// The dial code a native device's WebRTC Direct listener shows, read on the
// page (docs/spec/stream-framing.md, "The dial code"; W16).
//
// Invariants:
// - Every rule in the chapter's table is applied, and no other. The
//   listener's reader (`DialCode::parse` in offline-protocol-webrtc-direct)
//   is the other implementation, and both replay
//   crates/offline-protocol-webrtc-direct/tests/data/webrtc-direct-dial-v1.vectors.json,
//   so a rule one of them drops fails a vector rather than ship.
// - No value is ever percent-decoded. Every value comes from an alphabet
//   that needs no encoding, so a `%` anywhere refuses the code. A decoder
//   would make two spellings of one code, and a second spelling is how a
//   check on one is skipped on the other.
// - The host is an IP literal on the local network: private or link-local,
//   never loopback, never a name, and IPv6 only in its one RFC 5952
//   spelling. A name would be resolved by the browser and could point the
//   dial anywhere, and a public address would send the page's traffic off
//   the network it is on.
// - The address is only checked here, never trusted: it is the claim the
//   link's preamble must prove, and the engine parses it again when the link
//   opens (OfflineNode.openLink's `expect`). This check exists so that a
//   damaged code is refused when it is read, naming the field, rather than
//   when it is dialled.

const VERSION = "1";
const MAX_FRAGMENT_LEN = 512;
const CERTHASH_TEXT_LEN = 43;
const UFRAG_LEN = [4, 32];
const PASSWORD_LEN = [22, 64];
// Every key the format defines, in the order a listener writes them.
const KEYS = ["op-direct", "h", "p", "c", "u", "w", "a"];

const MESSAGES = {
  too_long: `the code is over ${MAX_FRAGMENT_LEN} characters`,
  percent_encoded: "the code contains a percent-encoded value",
  malformed: "the code has a part that is not key=value",
  unsupported_version: "the code's version is not one this dialler supports",
  host_not_literal: "the code's host is not an IP address literal",
  host_loopback: "the code's host is a loopback address",
  host_not_local: "the code's host is not a private or link-local address",
  port: "the code's port is not from 1 to 65535",
  certhash: "the code's certificate hash is not a 43-character base64url SHA-256",
  ufrag: "the code's ICE username is not 4 to 32 letters and digits",
  password: "the code's ICE password is not 22 to 64 letters and digits",
  address: "the code's address is not a canonical address",
};

/// Why a code was refused: `reason` names the rule (the vectors' names:
/// "too_long", "missing_key", "host_loopback", ...) and `key` the key, for a
/// missing or repeated one.
function refuse(reason, key) {
  const message =
    reason === "missing_key"
      ? `the code has no \`${key}\``
      : reason === "repeated_key"
        ? `the code names \`${key}\` twice`
        : MESSAGES[reason];
  const error = new Error(message);
  error.name = "DirectCodeError";
  error.reason = reason;
  if (key !== undefined) error.key = key;
  return error;
}

// What parseDirectCode returned, mapped to the values it checked, so a dial
// never uses a value a page edited after the check.
const checked = new WeakMap();

/// Reads a dial code from a whole URL, a `#fragment` or a bare fragment.
/// Everything after the first `#` is the fragment when there is one, and the
/// whole input otherwise.
///
/// Returns `{ host, port, certhash, ufrag, password, address }`: `host` as
/// the code writes it (an IPv4 or IPv6 literal, no brackets), `port` a
/// number, `certhash` the SHA-256 of the listener's certificate as a
/// 32-byte Uint8Array, and `address` the `off1…` address the link's
/// preamble must prove. Throws an Error whose `reason` names the rule the
/// code broke (and `key`, for a missing or repeated key).
export function parseDirectCode(input) {
  if (typeof input !== "string") throw new TypeError("a dial code is a string");
  const hash = input.indexOf("#");
  const fragment = hash === -1 ? input : input.slice(hash + 1);
  // In characters, as the chapter counts them; a code is ASCII, where this
  // is also its length in UTF-16 units.
  if ([...fragment].length > MAX_FRAGMENT_LEN) throw refuse("too_long");
  if (fragment.includes("%")) throw refuse("percent_encoded");

  const values = new Map();
  for (const part of fragment.split("&")) {
    const equals = part.indexOf("=");
    if (equals === -1) throw refuse("malformed");
    const key = part.slice(0, equals);
    if (!KEYS.includes(key)) continue;
    if (values.has(key)) throw refuse("repeated_key", key);
    values.set(key, part.slice(equals + 1));
  }
  for (const key of KEYS) if (!values.has(key)) throw refuse("missing_key", key);

  if (values.get("op-direct") !== VERSION) throw refuse("unsupported_version");
  const code = {
    host: checkHost(values.get("h")),
    port: checkPort(values.get("p")),
    certhash: checkCerthash(values.get("c")),
    ufrag: checkIce(values.get("u"), UFRAG_LEN, "ufrag"),
    password: checkIce(values.get("w"), PASSWORD_LEN, "password"),
    address: checkAddress(values.get("a")),
  };
  checked.set(code, { ...code, certhash: code.certhash.slice() });
  return code;
}

/// The values parseDirectCode checked for `code`, which is either the text
/// of a code or an object parseDirectCode returned.
export function checkedDirectCode(code) {
  if (typeof code === "string") return checked.get(parseDirectCode(code));
  const values = code !== null && typeof code === "object" ? checked.get(code) : undefined;
  if (values === undefined) {
    throw new TypeError("dial takes a code's text, or what parseDirectCode returned for it");
  }
  return values;
}

// ---------------------------------------------------------------------------
// The host

function checkHost(text) {
  const v6 = text.includes(":");
  const bytes = v6 ? parseIpv6(text) : parseIpv4(text);
  if (bytes === null) throw refuse("host_not_literal");
  if (v6 && formatIpv6(bytes) !== text) throw refuse("host_not_literal");
  if (v6) {
    if (bytes.every((b, i) => b === (i === 15 ? 1 : 0))) throw refuse("host_loopback");
    // fc00::/7 (unique local) and fe80::/10 (link-local unicast).
    const local = (bytes[0] & 0xfe) === 0xfc || (bytes[0] === 0xfe && (bytes[1] & 0xc0) === 0x80);
    if (!local) throw refuse("host_not_local");
  } else {
    const [a, b] = bytes;
    if (a === 127) throw refuse("host_loopback");
    const local =
      a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 169 && b === 254);
    if (!local) throw refuse("host_not_local");
  }
  return text;
}

// Dotted decimal, four parts, no leading zeros (an octal reading of
// 192.168.001.1 differs between parsers, so it is no one's address).
function parseIpv4(text) {
  const parts = text.split(".");
  if (parts.length !== 4) return null;
  const bytes = [];
  for (const part of parts) {
    if (!/^(0|[1-9][0-9]{0,2})$/.test(part)) return null;
    const value = Number(part);
    if (value > 255) return null;
    bytes.push(value);
  }
  return bytes;
}

// Any RFC 4291 text form, IPv4 tail included; the canonical comparison that
// follows refuses every spelling but one.
function parseIpv6(text) {
  const halves = text.split("::");
  if (halves.length > 2) return null;
  const groups = (half) => (half === "" ? [] : half.split(":"));
  const head = groups(halves[0]);
  const tail = halves.length === 2 ? groups(halves[1]) : [];
  const words = [];
  const all = [...head, ...tail];
  for (let i = 0; i < all.length; i++) {
    const group = all[i];
    if (i === all.length - 1 && group.includes(".")) {
      const v4 = parseIpv4(group);
      if (v4 === null) return null;
      words.push((v4[0] << 8) | v4[1], (v4[2] << 8) | v4[3]);
    } else {
      if (!/^[0-9a-fA-F]{1,4}$/.test(group)) return null;
      words.push(parseInt(group, 16));
    }
  }
  const headWords = wordsIn(head);
  let full;
  if (halves.length === 2) {
    if (words.length > 7) return null;
    full = [...words.slice(0, headWords), ...new Array(8 - words.length).fill(0), ...words.slice(headWords)];
  } else {
    if (words.length !== 8) return null;
    full = words;
  }
  return full.flatMap((w) => [w >> 8, w & 0xff]);
}

function wordsIn(groups) {
  return groups.reduce((n, g) => n + (g.includes(".") ? 2 : 1), 0);
}

// RFC 5952: lower-case hex with no leading zeros, the longest run of two or
// more zero groups (the first, on a tie) written `::`, and an IPv4-mapped
// address with its IPv4 tail in dotted decimal.
function formatIpv6(bytes) {
  const words = [];
  for (let i = 0; i < 16; i += 2) words.push((bytes[i] << 8) | bytes[i + 1]);
  if (words.slice(0, 5).every((w) => w === 0) && words[5] === 0xffff) {
    return `::ffff:${bytes.slice(12).join(".")}`;
  }
  let best = { at: -1, length: 0 };
  for (let i = 0; i < 8; ) {
    if (words[i] !== 0) {
      i++;
      continue;
    }
    let j = i;
    while (j < 8 && words[j] === 0) j++;
    if (j - i > best.length) best = { at: i, length: j - i };
    i = j;
  }
  const hex = (list) => list.map((w) => w.toString(16)).join(":");
  if (best.length < 2) return hex(words);
  return `${hex(words.slice(0, best.at))}::${hex(words.slice(best.at + best.length))}`;
}

// ---------------------------------------------------------------------------
// The other values

function checkPort(text) {
  // No leading zero: one port, one spelling.
  if (!/^[1-9][0-9]{0,4}$/.test(text)) throw refuse("port");
  const port = Number(text);
  if (port < 1 || port > 65535) throw refuse("port");
  return port;
}

const BASE64URL = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

// 43 characters carry 258 bits for 256, so the last character's two low
// bits are spare. They must be zero: one certificate, one code.
function checkCerthash(text) {
  if (text.length !== CERTHASH_TEXT_LEN) throw refuse("certhash");
  const sextets = [];
  for (const char of text) {
    const value = BASE64URL.indexOf(char);
    if (value === -1) throw refuse("certhash");
    sextets.push(value);
  }
  if ((sextets[sextets.length - 1] & 0b11) !== 0) throw refuse("certhash");
  const bytes = new Uint8Array(32);
  let bits = 0;
  let acc = 0;
  let at = 0;
  for (const value of sextets) {
    acc = (acc << 6) | value;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      bytes[at++] = (acc >> bits) & 0xff;
    }
  }
  return bytes;
}

function checkIce(text, [min, max], reason) {
  if (text.length < min || text.length > max || !/^[A-Za-z0-9]+$/.test(text)) throw refuse(reason);
  return text;
}

// The address's text form: `off1`, 34 data characters and a 6-character
// bech32m checksum, all lower case, whose data is the version byte 0x01 and
// a 20-byte hash with its 2 padding bits zero. Anything else is not the one
// spelling of an address. This reads and checks; it derives nothing.
const BECH32_CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const BECH32M_CONST = 0x2bc830a3;
const ADDRESS_HRP = "off";
const ADDRESS_LEN = 44;
const ADDRESS_VERSION = 0x01;

function checkAddress(text) {
  if (text.length !== ADDRESS_LEN || !text.startsWith(`${ADDRESS_HRP}1`)) throw refuse("address");
  const data = [];
  for (const char of text.slice(ADDRESS_HRP.length + 1)) {
    const value = BECH32_CHARSET.indexOf(char);
    if (value === -1) throw refuse("address");
    data.push(value);
  }
  const expanded = [];
  for (const char of ADDRESS_HRP) expanded.push(char.charCodeAt(0) >> 5);
  expanded.push(0);
  for (const char of ADDRESS_HRP) expanded.push(char.charCodeAt(0) & 31);
  if (polymod([...expanded, ...data]) !== BECH32M_CONST) throw refuse("address");
  // 34 five-bit groups: 168 bits of payload and 2 of padding.
  const groups = data.slice(0, -6);
  let acc = 0;
  let bits = 0;
  const payload = [];
  for (const value of groups) {
    acc = ((acc << 5) | value) & 0xfff;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      payload.push((acc >> bits) & 0xff);
    }
  }
  if (payload.length !== 21 || bits !== 2 || (acc & 0b11) !== 0 || payload[0] !== ADDRESS_VERSION) {
    throw refuse("address");
  }
  return text;
}

function polymod(values) {
  const generator = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
  let chk = 1;
  for (const value of values) {
    const top = chk >>> 25;
    chk = (((chk & 0x1ffffff) << 5) ^ value) >>> 0;
    for (let i = 0; i < 5; i++) if ((top >>> i) & 1) chk = (chk ^ generator[i]) >>> 0;
  }
  return chk;
}
