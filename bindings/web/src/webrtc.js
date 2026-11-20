// Links between devices over WebRTC data channels, set up with no server.
//
// Invariants (docs/bridges/web.md, W9):
// - The carrier lives on the page. RTCPeerConnection exists only on the main
//   thread, so bytes cross to the node's worker by message, under a link id
//   the engine allocated (OfflineNode.openLink).
// - One reliable, ordered channel per peer. The engine's link is a byte
//   stream: the identity preamble must arrive before every frame, and a
//   frame's bytes in order. Bytes are cut into chunks of at most CHUNK_BYTES,
//   because every browser accepts a message that size and some refuse a
//   larger one, and the engine's stream reader joins them again. The
//   carrier never interprets a byte.
// - Signalling needs no network. Each side's description waits for ICE
//   gathering to finish, so one code carries every candidate, and the two
//   codes cross by whatever the people have: a QR code, a paste, a chat, or
//   a NostrSignaller room. With no ICE server (the default) the connection
//   is made on the local network or not at all; the application may name its
//   own STUN and TURN servers, and every candidate they yield goes in the
//   code.
// - A carrier address proves nothing. The connection only carries bytes; the
//   engine's preamble proves who is on the other end.
// - A connection closes on "failed" or "closed", or once it has stayed
//   "disconnected" for DISCONNECTED_MS. "disconnected" may recover on its
//   own, and closing at once would turn a Wi-Fi blip into two new codes
//   typed by hand. But nothing obliges a browser to move on to "failed":
//   Android's WebView was seen to sit in "disconnected" for good after the
//   other device restarted. The engine takes one link per address, so that
//   dead link refused every new link from the same device, and the two
//   never met again. The deadline is longer than Chrome's own (it fails 10 s
//   after "disconnected"), so it only ever ends a connection the browser
//   would have left hanging. The page hears every state as a `connection`
//   event too, so it can warn at "disconnected" (5 s after the last
//   answered check in Chrome) rather than look connected until the close.
// - A connection reports nothing until offer(), answer() or dial() has
//   handed the page its object. A call that fails, or whose connection
//   closed while it gathered, rejects and reports nothing: an event naming
//   an object the page never received is one it cannot match to anything.
// - A dial (W16) edits no description of the page's own: it builds the
//   listener's description from the code and sets the browser's offer
//   unchanged. libwebrtc is removing the edit to a local ICE username and
//   password, and a dial built on it would stop working the day a browser
//   refuses it. Everything after its channel opens is the same Connection
//   as a signalled one.

import { checkedDirectCode } from "./direct-code.js";

const CODE_PREFIX = "op1.";
const CHANNEL_ID = 0;
const CHUNK_BYTES = 16 * 1024;
const GATHER_MS = 5000;
// How long a connection may stay "disconnected" before it is closed.
const DISCONNECTED_MS = 15000;
// How long a dial waits for its channel to open (W16).
const DIAL_MS = 15000;
// The longest delay a browser timer honours; a longer one fires at once.
const MAX_TIMER_MS = 0x7fffffff;
// Above this many bytes buffered on the channel, sends wait for it to drain.
const HIGH_WATER_BYTES = 1024 * 1024;
// Above this many bytes waiting behind the channel, the peer is not keeping
// up and the connection is closed. The engine keeps every acknowledged
// message for a retry, so closing loses only what was never promised.
const MAX_BACKLOG_BYTES = 8 * 1024 * 1024;

async function compress(text) {
  const stream = new Blob([text]).stream().pipeThrough(new CompressionStream("deflate-raw"));
  const bytes = new Uint8Array(await new Response(stream).arrayBuffer());
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function decompress(code) {
  const base64 = code.replace(/-/g, "+").replace(/_/g, "/");
  const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
  return new Response(stream).text();
}

/// The description as a code: the SDP, every candidate included.
async function encode(description) {
  return CODE_PREFIX + (await compress(JSON.stringify({ t: description.type, s: description.sdp })));
}

async function decode(code, expected) {
  const text = code.trim();
  if (!text.startsWith(CODE_PREFIX)) throw new Error("not an Offline Protocol connection code");
  let parsed;
  try {
    parsed = JSON.parse(await decompress(text.slice(CODE_PREFIX.length)));
  } catch {
    throw new Error("the connection code is damaged or incomplete");
  }
  if (parsed?.t !== expected || typeof parsed.s !== "string") {
    throw new Error(`expected an ${expected} code`);
  }
  return { type: parsed.t, sdp: parsed.s };
}

function gathered(pc) {
  if (pc.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve) => {
    const timer = setTimeout(done, GATHER_MS);
    function done() {
      clearTimeout(timer);
      pc.removeEventListener("icegatheringstatechange", check);
      resolve();
    }
    function check() {
      if (pc.iceGatheringState === "complete") done();
    }
    pc.addEventListener("icegatheringstatechange", check);
  });
}

/// The listener's description, built from its code alone
/// (docs/spec/stream-framing.md, "The page's side"). `mid` is the offer's.
function listenerAnswer(code, mid) {
  const family = code.host.includes(":") ? "IP6" : "IP4";
  const fingerprint = Array.from(code.certhash, (b) => b.toString(16).padStart(2, "0").toUpperCase()).join(":");
  return [
    "v=0",
    `o=- 0 0 IN ${family} ${code.host}`,
    "s=-",
    "t=0 0",
    "a=ice-lite",
    `m=application ${code.port} UDP/DTLS/SCTP webrtc-datachannel`,
    `c=IN ${family} ${code.host}`,
    `a=mid:${mid}`,
    "a=ice-options:ice2",
    `a=ice-ufrag:${code.ufrag}`,
    `a=ice-pwd:${code.password}`,
    `a=fingerprint:sha-256 ${fingerprint}`,
    "a=setup:passive",
    "a=sctp-port:5000",
    "a=max-message-size:16384",
    `a=candidate:1467250027 1 UDP 1467250027 ${code.host} ${code.port} typ host`,
    "a=end-of-candidates",
    "",
  ].join("\r\n");
}

/// One connection toward one other device.
class Connection {
  #node;
  #pc;
  #channel;
  #link = null;
  #inbound = []; // bytes that arrived before the link id
  #outbound = Promise.resolve();
  // Bytes handed to #send and not yet on the channel.
  #backlog = 0;
  // Sends waiting for the channel to drain; close() releases them, as a
  // closed channel never fires bufferedamountlow.
  #draining = new Set();
  #closed = false;
  #onClose;
  #onState = null;
  #reported = null;
  #disconnectedMs;
  // Closes the connection once it has been "disconnected" too long.
  #stale = null;
  // The address the other end's preamble must prove, for a dial (W16).
  #expect;
  // Called once the channel opens, before the link is opened, for a dial.
  #onOpen;
  // { resolve, reject } while a dial waits for its channel to open.
  #dialling = null;
  // Rejects once the connection closes; see #step.
  #ended;
  #end;

  constructor(node, iceServers, disconnectedMs, onClose, { expect, onOpen } = {}) {
    this.#node = node;
    this.#disconnectedMs = disconnectedMs;
    this.#onClose = onClose;
    this.#expect = expect;
    this.#onOpen = onOpen;
    this.#ended = new Promise((_, reject) => {
      this.#end = reject;
    });
    // Nothing need be waiting on it when the connection closes.
    this.#ended.catch(() => {});
    this.#pc = new RTCPeerConnection({ iceServers });
    this.#channel = this.#pc.createDataChannel("offline-protocol", {
      negotiated: true,
      id: CHANNEL_ID,
      ordered: true,
    });
    this.#channel.binaryType = "arraybuffer";
    this.#channel.bufferedAmountLowThreshold = HIGH_WATER_BYTES / 2;
    this.#channel.onopen = () => this.#opened();
    this.#channel.onmessage = ({ data }) => this.#received(new Uint8Array(data));
    this.#channel.onclose = () => this.close();
    this.#pc.onconnectionstatechange = () => {
      const state = this.#pc.connectionState;
      if (state === "failed" || state === "closed") return this.close();
      if (state === "disconnected") this.#expire();
      else this.#recovered();
      if (state === "connected" || state === "disconnected") this.#report(state);
    };
  }

  #expire() {
    if (this.#stale !== null || this.#disconnectedMs === Infinity) return;
    // The state is read again at the deadline: a recovery whose change event
    // is still queued behind this timer must not be closed.
    this.#stale = setTimeout(() => {
      this.#stale = null;
      if (this.#pc.connectionState === "disconnected") this.close();
    }, Math.min(this.#disconnectedMs, MAX_TIMER_MS));
  }

  #recovered() {
    clearTimeout(this.#stale);
    this.#stale = null;
  }

  /// Starts reporting to `onState`, beginning with the state the connection
  /// is already in, so a change before this call is not lost.
  listen(onState) {
    this.#onState = onState;
    const state = this.#pc.connectionState;
    if (state === "connected" || state === "disconnected") this.#report(state);
  }

  // Once per change: "connected", "disconnected", then "closed" exactly once.
  // Nothing before listen(). dispatchEvent never throws for a listener (the
  // browser reports the exception), so a throwing listener cannot stop this.
  #report(state) {
    if (this.#onState === null || this.#reported === state || this.#reported === "closed") return;
    this.#reported = state;
    this.#onState(state);
  }

  get peerConnection() {
    return this.#pc;
  }

  get closed() {
    return this.#closed;
  }

  // `promise`, or a rejection once the connection closes. A browser
  // abandons a description step whose connection closed while it ran, and
  // never settles it ("if [[IsClosed]], abort these steps"), so a call that
  // awaited the step alone would never settle either: a page that dialled
  // one code twice, or closed its carrier, during a dial's setup was left
  // with a dial() that never resolved or rejected.
  #step(promise) {
    return Promise.race([promise, this.#ended]);
  }

  async offer() {
    await this.#step(this.#pc.setLocalDescription(await this.#step(this.#pc.createOffer())));
    await this.#step(gathered(this.#pc));
    return encode(this.#pc.localDescription);
  }

  async answer(offerCode) {
    await this.#step(this.#pc.setRemoteDescription(await decode(offerCode, "offer")));
    await this.#step(this.#pc.setLocalDescription(await this.#step(this.#pc.createAnswer())));
    await this.#step(gathered(this.#pc));
    return encode(this.#pc.localDescription);
  }

  async accept(answerCode) {
    await this.#step(this.#pc.setRemoteDescription(await decode(answerCode, "answer")));
  }

  /// Dials a listener from its checked code. Resolves once the channel is
  /// open; rejects, closing the connection, when it closes first or has not
  /// opened within `timeoutMs`.
  async dial(code, timeoutMs) {
    const opened = new Promise((resolve, reject) => {
      this.#dialling = { resolve, reject };
    });
    const timer = setTimeout(() => {
      this.#settleDial(dialError(`the dial was not answered within ${timeoutMs} ms`, "timeout"));
      this.close();
    }, Math.min(timeoutMs, MAX_TIMER_MS));
    try {
      const offer = await this.#step(this.#pc.createOffer());
      // Exactly as the browser made it: the page edits no local description.
      await this.#step(this.#pc.setLocalDescription(offer));
      const mid = /^a=mid:(\S+)/m.exec(offer.sdp)?.[1];
      if (mid === undefined) throw new Error("the browser's offer has no data channel section");
      await this.#step(this.#pc.setRemoteDescription({ type: "answer", sdp: listenerAnswer(code, mid) }));
    } catch (error) {
      this.#settleDial(error);
      this.close();
    }
    try {
      await opened;
    } finally {
      clearTimeout(timer);
    }
  }

  // Settles a waiting dial once: resolved with no error, else rejected.
  #settleDial(error) {
    const dialling = this.#dialling;
    if (dialling === null) return;
    this.#dialling = null;
    if (error) dialling.reject(error);
    else dialling.resolve();
  }

  async #opened() {
    this.#settleDial(null);
    // Ahead of openLink: the node answers calls in the order they were
    // made, so a link this closes is gone before this one is opened.
    this.#onOpen?.(this);
    try {
      this.#link = await this.#node.openLink({
        send: (bytes) => this.#send(bytes),
        close: () => this.close({ engine: true }),
        expect: this.#expect,
      });
    } catch {
      // The engine's link ceiling, or a node that ended.
      return this.close();
    }
    if (this.#closed) {
      this.#node.closeLink(this.#link);
      return;
    }
    for (const bytes of this.#inbound.splice(0)) this.#deliver(bytes);
  }

  #received(bytes) {
    if (this.#link === null) this.#inbound.push(bytes);
    else this.#deliver(bytes);
  }

  #deliver(bytes) {
    // The node's calls are answered in the order they were made, so bytes
    // reach the engine in the order they arrived.
    this.#node.linkReceived(this.#link, bytes).catch(() => this.close());
  }

  #send(bytes) {
    if (this.#closed) return;
    this.#backlog += bytes.length;
    if (this.#backlog > MAX_BACKLOG_BYTES) return this.close();
    // Chained, so a wait for the buffer to drain never reorders bytes.
    this.#outbound = this.#outbound.then(async () => {
      try {
        await this.#transmit(bytes);
      } finally {
        this.#backlog -= bytes.length;
      }
    });
  }

  async #transmit(bytes) {
    for (let at = 0; at < bytes.length && !this.#closed; at += CHUNK_BYTES) {
      if (this.#channel.bufferedAmount > HIGH_WATER_BYTES) {
        await new Promise((resolve) => {
          const done = () => {
            this.#draining.delete(done);
            resolve();
          };
          this.#draining.add(done);
          this.#channel.addEventListener("bufferedamountlow", done, { once: true });
        });
      }
      if (this.#channel.readyState !== "open") return;
      this.#channel.send(bytes.subarray(at, at + CHUNK_BYTES));
    }
  }

  /// Closes the connection. The other end sees its channel close, which is
  /// its signal that this side refused or left.
  close({ engine = false } = {}) {
    if (this.#closed) return;
    this.#closed = true;
    // Read before the close below makes every connection "closed". A
    // certificate that does not match a dial's code fails DTLS, which the
    // browser reports as "failed".
    const failed = this.#pc.connectionState === "failed";
    this.#settleDial(
      failed
        ? dialError("the connection failed (a certificate that does not match the code fails DTLS)", "failed")
        : dialError("the connection closed before its channel opened", "closed"),
    );
    this.#recovered();
    this.#end(new Error("the connection closed"));
    for (const done of [...this.#draining]) done();
    if (this.#link !== null && !engine) this.#node.closeLink(this.#link);
    this.#channel.close();
    this.#pc.close();
    this.#onClose(this);
    this.#report("closed");
  }
}

// A rejected dial's error: `state` is "failed" (DTLS or ICE failed),
// "closed" (closed before its channel opened) or "timeout".
function dialError(message, state) {
  const error = new Error(`the dial did not open: ${message}`);
  error.state = state;
  return error;
}

/// WebRTC links for one node. Each connection takes two codes: the side that
/// offers shows an offer code and reads back an answer code; the other side
/// reads the offer and shows its answer. A native device's WebRTC Direct
/// listener takes one code, which `dial()` reads (W16).
///
/// Every change of a connection's state is a `connection` event with
/// `{ state, connection }` as its detail: `state` is "connected",
/// "disconnected" (no answer to the last checks; it may recover, and is
/// closed if it has not within `disconnectedMs`) or "closed" (once, at the
/// end), and `connection` is the object `offer()`, `answer()` or `dial()`
/// resolved to, so a page with several devices can tell them apart. A call
/// that rejects reports nothing.
export class WebRtcCarrier extends EventTarget {
  #node;
  #iceServers;
  #disconnectedMs;
  #connections = new Set();
  // address -> the dialled Connection linked toward it (W16).
  #dialled = new Map();
  // address -> a dialled Connection toward it whose channel has not opened.
  #dialling = new Map();

  /// `iceServers` is RTCPeerConnection's list, for networks where devices
  /// cannot reach each other directly: a STUN server for addresses behind a
  /// NAT, a TURN server to relay. The default is none, so links stay on the
  /// local network and no server learns of them (W9).
  ///
  /// `disconnectedMs` is how long a connection may stay "disconnected"
  /// before it is closed and the engine reports the peer lost; `Infinity`
  /// leaves it to the browser, which may never decide.
  constructor(node, { iceServers = [], disconnectedMs = DISCONNECTED_MS } = {}) {
    super();
    if (typeof disconnectedMs !== "number" || !(disconnectedMs > 0)) {
      throw new Error("disconnectedMs must be a positive number of milliseconds");
    }
    this.#node = node;
    this.#iceServers = iceServers;
    this.#disconnectedMs = disconnectedMs;
  }

  /// Starts a connection. Resolves to `{ code, accept(answerCode), close() }`:
  /// show `code` to the other device, then pass its answer to `accept`.
  async offer() {
    const connection = this.#track(this.#connection());
    const code = await this.#code(connection, () => connection.offer());
    return this.#hand(connection, {
      code,
      accept: (answerCode) => connection.accept(answerCode),
      close: () => connection.close(),
    });
  }

  /// Answers another device's offer. Resolves to `{ code, close() }`: show
  /// `code` to the device that offered.
  async answer(offerCode) {
    const connection = this.#track(this.#connection());
    const code = await this.#code(connection, () => connection.answer(offerCode));
    return this.#hand(connection, { code, close: () => connection.close() });
  }

  /// Dials a native device's WebRTC Direct listener from the code it shows
  /// (W16): the code's text (a whole URL, a `#fragment` or a bare
  /// fragment) or what `parseDirectCode` returned for it. Resolves to
  /// `{ address, close() }` once the data channel is open, where `address`
  /// is the code's `a`; from there the connection is the same as one
  /// `offer()` makes, with the same `connection` events.
  ///
  /// The link's preamble must prove the code's address: the engine closes a
  /// link that proves anyone else and announces nothing. A dial closes this
  /// carrier's earlier dialled connection to the same address first. The
  /// engine keeps its first link per address (W3) and the listener its
  /// newest dialled stream, so a page that kept the old link would leave
  /// each end closing the link the other kept. No ICE server is used,
  /// whatever the carrier was given: the listener is on the local network
  /// or nowhere.
  ///
  /// Rejects, closing the connection, when the code is refused (see
  /// `parseDirectCode`), when the connection fails (a certificate that does
  /// not match the code fails DTLS; the error's `state` is "failed"), or
  /// when the channel has not opened within `timeoutMs` (default 15000;
  /// `state` "timeout"). A listener that does not admit the dial (a wrong
  /// password, a closed listener, a browser that withheld the candidate)
  /// answers nothing, so that is how such a dial ends.
  async dial(code, { timeoutMs = DIAL_MS } = {}) {
    if (typeof timeoutMs !== "number" || !(timeoutMs > 0)) {
      throw new Error("timeoutMs must be a positive number of milliseconds");
    }
    const checked = checkedDirectCode(code);
    const { address } = checked;
    // A dial of this address still setting up is abandoned: it links
    // nothing yet. A linked one is kept until this dial's channel opens, so
    // a dial that fails (a stale code, a listener gone) costs nothing that
    // was working.
    this.#dialling.get(address)?.close();
    const connection = this.#track(
      new Connection(
        this.#node,
        [],
        this.#disconnectedMs,
        (c) => {
          this.#connections.delete(c);
          if (this.#dialling.get(address) === c) this.#dialling.delete(address);
          if (this.#dialled.get(address) === c) this.#dialled.delete(address);
        },
        {
          expect: address,
          // The engine keeps its first link per address (W3) and the
          // listener its newest dialled stream, so the earlier link is
          // closed before this one opens: each end then keeps this one.
          onOpen: (c) => {
            if (this.#dialling.get(address) === c) this.#dialling.delete(address);
            const held = this.#dialled.get(address);
            if (held !== undefined && held !== c) held.close();
            this.#dialled.set(address, c);
          },
        },
      ),
    );
    this.#dialling.set(address, connection);
    try {
      await connection.dial(checked, timeoutMs);
    } catch (error) {
      connection.close();
      throw error;
    }
    if (connection.closed) throw dialError("the connection closed before its channel opened", "closed");
    return this.#hand(connection, { address, close: () => connection.close() });
  }

  /// Closes every connection.
  close() {
    for (const connection of [...this.#connections]) connection.close();
  }

  #connection() {
    return new Connection(this.#node, this.#iceServers, this.#disconnectedMs, (c) => this.#connections.delete(c));
  }

  // The connection's code, or a rejection that closes it. A connection that
  // closed while it gathered (carrier.close(), or the page leaving) rejects
  // too, rather than resolve to a code nobody can use.
  async #code(connection, make) {
    let code;
    try {
      code = await make();
    } catch (error) {
      connection.close();
      throw error;
    }
    if (connection.closed) throw new Error("the connection closed before its code was ready");
    return code;
  }

  // Hands `handle` to the page: from here every change of state is an event
  // naming it.
  #hand(connection, handle) {
    connection.listen((state) =>
      this.dispatchEvent(new CustomEvent("connection", { detail: { state, connection: handle } })),
    );
    return handle;
  }

  #track(connection) {
    this.#connections.add(connection);
    return connection;
  }
}
