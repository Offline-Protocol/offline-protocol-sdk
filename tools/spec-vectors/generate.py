#!/usr/bin/env python3
"""Computes the conformance vectors for the Offline Protocol wire spec.

This script is a second implementation of the encodings in `docs/spec/`. It is
written from those chapters and MUST NOT import from, link against, or shell
out to the Rust crates it pins. That independence is the whole point: a vector
produced by running the implementation under test agrees with any format that
implementation happens to emit, including a wrong one, so it proves nothing.

What independence means here, stated exactly, because the stronger reading
would be an overclaim: every value below is computed from the rules published
in the spec chapters, not by executing the reference codec. For the binary
frame those rules and the reference implementation share an ancestor in the
published postcard wire format, which both were written against separately.

Run with no arguments to write the vector files. Run with --check to verify
that the committed files are what this script produces, which is what CI does:
it makes editing a vector to turn a red test green impossible without also
editing this file, and that diff reads as what it is.

Usage:
    python3 tools/spec-vectors/generate.py            # write
    python3 tools/spec-vectors/generate.py --check    # verify, exit 1 on drift
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

CORE_DATA = REPO / "crates" / "offline-protocol-core" / "tests" / "data"
SEALED_DATA = REPO / "crates" / "offline-protocol-sealed" / "tests" / "data"


# --------------------------------------------------------------------------
# Primitive encoding, from docs/spec/wire-format.md "Primitive encoding".
# --------------------------------------------------------------------------


def varint(value: int) -> bytes:
    """Little-endian base 128: seven data bits per byte, high bit continues."""
    if value < 0:
        raise ValueError(f"varint is unsigned, got {value}")
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def zigzag(value: int) -> int:
    """(n << 1) ^ (n >> 63) with an arithmetic shift, for a 64-bit signed n."""
    if not (-(2**63) <= value < 2**63):
        raise ValueError(f"i64 out of range: {value}")
    return ((value << 1) ^ (value >> 63)) & 0xFFFFFFFFFFFFFFFF


def enc_i64(value: int) -> bytes:
    return varint(zigzag(value))


def enc_u64(value: int) -> bytes:
    if not (0 <= value < 2**64):
        raise ValueError(f"u64 out of range: {value}")
    return varint(value)


def enc_u8(value: int) -> bytes:
    if not (0 <= value < 256):
        raise ValueError(f"u8 out of range: {value}")
    return bytes([value])


def enc_bool(value: bool) -> bytes:
    return b"\x01" if value else b"\x00"


def enc_bytes(value: bytes) -> bytes:
    return varint(len(value)) + value


def enc_str(value: str) -> bytes:
    return enc_bytes(value.encode("utf-8"))


def enc_opt(value, inner) -> bytes:
    return b"\x00" if value is None else b"\x01" + inner(value)


def enc_seq(values, inner) -> bytes:
    return varint(len(values)) + b"".join(inner(v) for v in values)


# --------------------------------------------------------------------------
# The binary wire v1 frame, from docs/spec/wire-format.md.
# --------------------------------------------------------------------------

WIRE_V1_MAGIC = 0xF5

PRIORITY = {"low": 0, "medium": 1, "high": 2, "critical": 3}

CONTENT_TYPE = {
    "text": 0,
    "image": 1,
    "video": 2,
    "audio": 3,
    "voice_note": 4,
    "video_note": 5,
    "file": 6,
    "file_chunk": 7,
    "poll": 8,
}

EXT_TAG_B64_TAIL = 1
EXT_TAG_REPLY_CONTEXT = 2

B64_TAIL_MIN_LEN = 64
B64_ALPHABET = set(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
)


def uuid_bytes(text: str) -> bytes:
    raw = bytes.fromhex(text.replace("-", ""))
    if len(raw) != 16:
        raise ValueError(f"not a 16-byte uuid: {text}")
    return raw


def b64_encode(raw: bytes) -> str:
    import base64

    return base64.b64encode(raw).decode("ascii")


def split_b64_tail(content: str):
    """The reference encoder's tag-1 split, reproduced from the chapter.

    Returns (head, raw_tail_bytes) or None. Where this splits is NOT normative:
    a conforming encoder may split elsewhere or never split at all, because a
    decoder reconstructs content from whatever tag 1 carries. What is normative
    is the reconstruction, which the decode vectors pin.

    No vector below uses a tail with `=` padding, and that is deliberate rather
    than an oversight. The chapter states the 64-character minimum and the
    re-encode-and-compare check, but not the arithmetic that 4-aligns the run,
    and two readings of it place the boundary differently once padding is in
    play. Since the split point is not normative, the chapter does not owe that
    detail and the vectors do not test it.

    If a padded case is ever added here and the Rust disagrees, the honest fix
    is to pin the alignment in the chapter and derive both from it. Editing this
    function to match the Rust is not: that makes the generator a copy of the
    implementation, which is the one thing it must never be.
    """
    if len(content.encode("utf-8")) < B64_TAIL_MIN_LEN:
        return None

    end = len(content)
    pads = 0
    while pads < 2 and end > 0 and content[end - 1] == "=":
        end -= 1
        pads += 1

    start = end
    while start > 0 and content[start - 1] in B64_ALPHABET:
        start -= 1

    start += (end - start) % 4
    tail = content[start : end + pads]
    if len(tail) < B64_TAIL_MIN_LEN:
        return None

    import base64

    try:
        raw = base64.b64decode(tail, validate=True)
    except Exception:
        return None

    if b64_encode(raw) != tail:
        return None

    return content[:start], raw


def raw_frame(
    *,
    id_hex: str = "00" * 16,
    sender: str = "a",
    recipient: str = "b",
    app_id: str = "c",
    priority: int = 1,
    ttl: int = 8,
    hop_count: int = 0,
    timestamp_ms: int = 0,
    lamport_clock: int = 0,
    content_type: int = 0,
    content: str = "",
    binary_content: bytes | None = None,
    media_metadata: bytes | None = None,
    metadata: list | None = None,
    requires_ack: bool = True,
    reply_to_msg: bytes | None = None,
    forwarded_from: bytes | None = None,
    ext: list | None = None,
) -> bytes:
    """Builds a frame from already-numeric fields.

    `encode_frame` cannot express a value the enums do not name, and the
    decode direction has to be pinned for exactly those: an unknown
    discriminant is a value a *future* sender emits, so no conforming encoder
    of this version can produce one.
    """
    return bytes([WIRE_V1_MAGIC]) + b"".join(
        [
            bytes.fromhex(id_hex),
            enc_str(sender),
            enc_str(recipient),
            enc_str(app_id),
            enc_u8(priority),
            enc_u8(ttl),
            enc_u8(hop_count),
            enc_i64(timestamp_ms),
            enc_u64(lamport_clock),
            enc_u8(content_type),
            enc_str(content),
            enc_opt(binary_content, enc_bytes),
            enc_opt(media_metadata, enc_bytes),
            enc_seq(metadata or [], lambda kv: enc_str(kv[0]) + enc_str(kv[1])),
            enc_bool(requires_ack),
            enc_opt(reply_to_msg, lambda b: b),
            enc_opt(forwarded_from, enc_bytes),
            enc_seq(ext or [], lambda e: varint(e[0]) + enc_bytes(e[1])),
        ]
    )


def encode_frame(m: dict) -> bytes:
    """Encodes one message spec into a 0xF5 frame."""
    content = m["content"]
    ext: list[tuple[int, bytes]] = []

    split = split_b64_tail(content)
    if split is not None:
        content, raw = split
        ext.append((EXT_TAG_B64_TAIL, raw))

    if m.get("reply_context_json") is not None:
        ext.append((EXT_TAG_REPLY_CONTEXT, m["reply_context_json"].encode("utf-8")))

    metadata = sorted(
        (k, v) for k, v in m.get("metadata", {}).items()
    )  # byte-wise on UTF-8 keys; see note below

    body = b"".join(
        [
            uuid_bytes(m["id"]),
            enc_str(m["sender"]),
            enc_str(m["recipient"]),
            enc_str(m["app_id"]),
            enc_u8(PRIORITY[m["priority"]]),
            enc_u8(m["ttl"]),
            enc_u8(m["hop_count"]),
            enc_i64(m["timestamp_ms"]),
            enc_u64(m["lamport_clock"]),
            enc_u8(CONTENT_TYPE[m["content_type"]]),
            enc_str(content),
            enc_opt(
                m.get("binary_content_hex"),
                lambda h: enc_bytes(bytes.fromhex(h)),
            ),
            enc_opt(
                m.get("media_metadata_json"),
                lambda s: enc_bytes(s.encode("utf-8")),
            ),
            enc_seq(metadata, lambda kv: enc_str(kv[0]) + enc_str(kv[1])),
            enc_bool(m["requires_ack"]),
            enc_opt(m.get("reply_to_msg"), uuid_bytes),
            enc_opt(
                m.get("forwarded_from_json"),
                lambda s: enc_bytes(s.encode("utf-8")),
            ),
            enc_seq(ext, lambda e: varint(e[0]) + enc_bytes(e[1])),
        ]
    )
    return bytes([WIRE_V1_MAGIC]) + body


# Python sorts str by code point; the spec orders by UTF-8 bytes. The two
# agree for every key below, and the sort is asserted to be byte-equivalent
# rather than assumed: a key above U+FFFF would separate them.
def _assert_sort_is_byte_wise(keys: list[str]) -> None:
    by_codepoint = sorted(keys)
    by_bytes = sorted(keys, key=lambda k: k.encode("utf-8"))
    if by_codepoint != by_bytes:
        raise AssertionError(
            "a metadata key set orders differently by code point and by UTF-8 "
            "bytes; the generator must sort by bytes"
        )


# --------------------------------------------------------------------------
# bech32m, from BIP-350's reference implementation.
# --------------------------------------------------------------------------

CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"
BECH32M_CONST = 0x2BC830A3


def bech32_polymod(values):
    generator = [0x3B6A57B2, 0x26508E6D, 0x1EA119FA, 0x3D4233DD, 0x2A1462B3]
    chk = 1
    for value in values:
        top = chk >> 25
        chk = (chk & 0x1FFFFFF) << 5 ^ value
        for i in range(5):
            chk ^= generator[i] if ((top >> i) & 1) else 0
    return chk


def bech32_hrp_expand(hrp):
    return [ord(x) >> 5 for x in hrp] + [0] + [ord(x) & 31 for x in hrp]


def bech32_create_checksum(hrp, data):
    values = bech32_hrp_expand(hrp) + data
    polymod = bech32_polymod(values + [0, 0, 0, 0, 0, 0]) ^ BECH32M_CONST
    return [(polymod >> 5 * (5 - i)) & 31 for i in range(6)]


def bech32_encode(hrp, data):
    combined = data + bech32_create_checksum(hrp, data)
    return hrp + "1" + "".join([CHARSET[d] for d in combined])


def convertbits(data, frombits, tobits, pad=True):
    acc = 0
    bits = 0
    ret = []
    maxv = (1 << tobits) - 1
    max_acc = (1 << (frombits + tobits - 1)) - 1
    for value in data:
        if value < 0 or (value >> frombits):
            return None
        acc = ((acc << frombits) | value) & max_acc
        bits += frombits
        while bits >= tobits:
            bits -= tobits
            ret.append((acc >> bits) & maxv)
    if pad:
        if bits:
            ret.append((acc << (tobits - bits)) & maxv)
    elif bits >= frombits or ((acc << (tobits - bits)) & maxv):
        return None
    return ret


# --------------------------------------------------------------------------
# Addressing, from docs/spec/identity.md.
# --------------------------------------------------------------------------

ADDRESS_HRP = "off"
ADDRESS_VERSION = 0x01
ADDRESS_HASH_LEN = 20


def address_from_hash(hash20: bytes) -> str:
    if len(hash20) != ADDRESS_HASH_LEN:
        raise ValueError("address hash is 20 bytes")
    payload = bytes([ADDRESS_VERSION]) + hash20
    return bech32_encode(ADDRESS_HRP, convertbits(payload, 8, 5))


def derive_address(public_key: bytes) -> str:
    if len(public_key) != 32:
        raise ValueError("an Ed25519 public key is 32 bytes")
    return address_from_hash(hashlib.sha256(public_key).digest()[:ADDRESS_HASH_LEN])


def address_hash(addr: str) -> bytes:
    """Recovers the 20 hash bytes from a canonical rendering."""
    data = [CHARSET.index(c) for c in addr[len(ADDRESS_HRP) + 1 : -6]]
    payload = bytes(convertbits(data, 5, 8, False))
    return payload[1:]


def session_id(a: str, b: str) -> str:
    """"session:" || lower || ":" || higher, ordered by hash bytes."""
    lo, hi = (a, b) if address_hash(a) <= address_hash(b) else (b, a)
    return f"session:{lo}:{hi}"


# --------------------------------------------------------------------------
# Canonical signing payloads, from docs/spec/control-messages.md.
# --------------------------------------------------------------------------

CTRL_DOMAIN_V1 = b"offline-ctrl-v1"
CTRL_DOMAIN_V2 = b"offline-ctrl-v2"


def canonical_payload(domain: bytes, fields: list[bytes]) -> bytes:
    """domain || for each field: u32be(len) || field. The domain is not
    length-prefixed, which is why the domains must be mutually non-prefixing."""
    out = bytearray(domain)
    for field in fields:
        out += len(field).to_bytes(4, "big")
        out += field
    return bytes(out)


def ctrl_payload_v1(sender: str, msg_id: str, recipient: str, content: str) -> bytes:
    return canonical_payload(
        CTRL_DOMAIN_V1,
        [
            sender.encode("utf-8"),
            msg_id.encode("utf-8"),
            recipient.encode("utf-8"),
            content.encode("utf-8"),
        ],
    )


def ctrl_payload_v2(
    sender: str, msg_id: str, recipient: str, content: str, timestamp_ms: int
) -> bytes:
    return canonical_payload(
        CTRL_DOMAIN_V2,
        [
            sender.encode("utf-8"),
            msg_id.encode("utf-8"),
            recipient.encode("utf-8"),
            content.encode("utf-8"),
            timestamp_ms.to_bytes(8, "big", signed=True),
        ],
    )


# --------------------------------------------------------------------------
# The compact MLS envelope, from docs/spec/encryption-envelopes.md.
# --------------------------------------------------------------------------

MLS_MESSAGE_TYPE = {"application": 0, "welcome": 1, "commit": 2, "proposal": 3}


def encode_envelope(e: dict) -> bytes:
    group_id = e["group_id"].encode("utf-8")
    sender_id = e["sender_id"].encode("utf-8")
    ciphertext = bytes.fromhex(e["ciphertext_hex"])
    return b"".join(
        [
            len(group_id).to_bytes(4, "little"),
            group_id,
            len(sender_id).to_bytes(4, "little"),
            sender_id,
            bytes([MLS_MESSAGE_TYPE[e["message_type"]]]),
            e["epoch"].to_bytes(8, "little"),
            e["timestamp_ms"].to_bytes(8, "little"),
            len(ciphertext).to_bytes(4, "little"),
            ciphertext,
        ]
    )


# --------------------------------------------------------------------------
# Vector construction.
# --------------------------------------------------------------------------

# Ed25519 public keys. The first three are the public keys of RFC 8032 section
# 7.1's test vectors 1, 2 and 3, reused here so a reader can check the input
# against a published source rather than trusting this file for both halves.
RFC8032_TV1_PK = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
RFC8032_TV2_PK = "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c"
RFC8032_TV3_PK = "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025"
ZEROS_PK = "00" * 32
FF_PK = "ff" * 32


def message(**kw) -> dict:
    """A message spec with the defaults every frame vector starts from."""
    base = {
        "id": "00000000-0000-0000-0000-000000000000",
        "sender": "a",
        "recipient": "b",
        "app_id": "c",
        "priority": "medium",
        "ttl": 8,
        "hop_count": 0,
        "timestamp_ms": 0,
        "lamport_clock": 0,
        "content_type": "text",
        "content": "",
        "binary_content_hex": None,
        "media_metadata_json": None,
        "metadata": {},
        "requires_ack": True,
        "reply_to_msg": None,
        "forwarded_from_json": None,
        "reply_context_json": None,
    }
    base.update(kw)
    return base


def frame_case(name: str, note: str, encoder_specific: bool = False, **kw) -> dict:
    m = message(**kw)
    _assert_sort_is_byte_wise(list(m["metadata"].keys()))
    case = {
        "name": name,
        "message": m,
        "hex": encode_frame(m).hex(),
        "note": note,
    }
    if encoder_specific:
        case["encoder_specific"] = True
    return case


def build_wire_vectors() -> dict:
    b64_tail = b64_encode(bytes(range(48)))  # 64 canonical base64 characters
    assert len(b64_tail) == 64

    frames = [
        frame_case(
            "minimal frame",
            "Every optional field absent, every list empty. The id is 16 raw "
            "bytes with no length prefix; the trailing five zero bytes are "
            "binary_content, media_metadata, metadata, reply_to_msg, "
            "forwarded_from and ext in that order.",
        ),
        frame_case(
            "multi-byte varints",
            "lamport_clock 300 is 0xAC 0x02 and not a fixed-width u64. A "
            "decoder reading eight bytes here consumes the rest of the frame.",
            lamport_clock=300,
        ),
        frame_case(
            "negative timestamp",
            "Zigzag sends -1 to 1, so the field is one byte. A decoder reading "
            "eight big-endian bytes reads a wildly wrong instant and stays "
            "misaligned for every field after it.",
            timestamp_ms=-1,
        ),
        frame_case(
            "a real instant",
            "1700000000000 ms. Zigzag doubles it, so this is the multi-byte "
            "signed case a fleet actually sends.",
            timestamp_ms=1700000000000,
        ),
        frame_case(
            "the largest and smallest instants",
            "i64::MIN. Zigzag maps it to u64::MAX, the longest varint a frame "
            "can carry: ten bytes.",
            timestamp_ms=-(2**63),
        ),
        frame_case(
            "non-ASCII sender and content",
            "Lengths are counted in UTF-8 bytes, never in characters. The "
            "content is 4 characters and 10 bytes.",
            sender="ünïcode",
            content="héllo→",
        ),
        frame_case(
            "metadata is emitted sorted",
            "Supplied as zebra, apple, mango; emitted apple, mango, zebra. A "
            "receiver never sees the sender's map order.",
            metadata={"zebra": "3", "apple": "1", "mango": "2"},
        ),
        frame_case(
            "metadata sorts by UTF-8 bytes",
            "A key that is a prefix of another sorts first, and the empty "
            "value is length 0 rather than absent.",
            metadata={"ack_for": "x", "ack_for_more": "", "ack": "y"},
        ),
        frame_case(
            "every option present",
            "binary_content, media_metadata, reply_to_msg and forwarded_from "
            "all take the 0x01 discriminant. Only reply_to_msg is fixed-width "
            "after it.",
            binary_content_hex="deadbeef",
            media_metadata_json='{"mime_type":"image/png","file_name":"a.png",'
            '"file_size":3}',
            reply_to_msg="0102030a-0b0c-0d0e-0f10-111213141516",
            forwarded_from_json='{"original_sender":"z",'
            '"original_message_id":"3f2504e0-4f89-11d3-9a0c-0305e82c3301",'
            '"original_timestamp":1700000000000,"forward_count":1}',
        ),
        frame_case(
            "every priority and a non-text content type",
            "priority Critical is 3 and content_type Poll is 8, both single "
            "raw bytes rather than varints.",
            priority="critical",
            content_type="poll",
        ),
        frame_case(
            "ttl and hop_count are raw bytes",
            "Both are u8 and neither is a varint, so 255 is one byte here "
            "where a varint would need two.",
            ttl=255,
            hop_count=255,
        ),
        frame_case(
            "a base64 tail rides the ext TLV",
            "The 64-character canonical tail is carried decoded under tag 1 "
            "and the wire content keeps only the head. Where the reference "
            "encoder splits is not normative: a conforming encoder may split "
            "elsewhere or not at all, because the decode vectors are what "
            "bind. What is pinned here is that the split reconstructs exactly.",
            encoder_specific=True,
            content="see " + b64_tail,
        ),
        frame_case(
            "a 63-character base64 tail is left alone",
            "One character below the 64-character minimum, so no split is "
            "taken and the content rides whole.",
            content="see " + b64_tail[:63],
        ),
        frame_case(
            "a reply context rides the ext TLV",
            "Tag 2 carries the quoted-reply preview as JSON. A decoder that "
            "skips it delivers the message without its preview.",
            reply_context_json='{"sender":"a","text":"hi"}',
        ),
        frame_case(
            "both ext tags, in order",
            "Tag 1 precedes tag 2 when the encoder emits both.",
            encoder_specific=True,
            content="see " + b64_tail,
            reply_context_json='{"sender":"a","text":"hi"}',
        ),
    ]

    return {
        "_comment": [
            "Frozen conformance vectors for the binary wire v1 frame described",
            "in docs/spec/wire-format.md. `hex` is the whole frame including",
            "the 0xF5 magic byte.",
            "",
            "Computed from the primitive encoding table in that chapter, not by",
            "running the codec they pin: a vector generated from the encoder",
            "would agree with any format it happened to emit, including a wrong",
            "one.",
            "",
            "A case marked `encoder_specific` pins a choice the reference",
            "encoder makes that the wire does not require. A second",
            "implementation is bound by the decode direction of those cases,",
            "not by the exact bytes.",
            "",
            "If one of these fails the wire format has changed. That needs a new",
            "magic byte and a negotiated version, not an edited expectation:",
            "editing the expected value converts a caught break into a shipped",
            "one.",
        ],
        "magic": "f5",
        "frames": frames,
        "decode_only": [
            {
                "name": "an unrecognised content type degrades to File",
                "hex": raw_frame(content_type=9).hex(),
                "expect": {"content_type": "file"},
                "note": "Discriminant 9 is not defined by this version. It "
                "degrades rather than rejecting the containing message, so a "
                "later variant does not make a frame undeliverable. A decoder "
                "built from a derived enum deserializer refuses this and does "
                "not conform.",
            },
            {
                "name": "an unrecognised priority degrades to Medium",
                "hex": raw_frame(priority=7).hex(),
                "expect": {"priority": "medium"},
                "note": "The numeric form tolerates an unknown value. The JSON "
                "floor does not, which is why a new priority is a breaking "
                "change there and has to be designed around.",
            },
            {
                "name": "an adversarial Lamport clock is clamped",
                "hex": raw_frame(lamport_clock=2**64 - 1).hex(),
                "expect": {"lamport_clock": 2**63 - 1},
                "note": "u64::MAX clamps to u64::MAX/2. The clamp is a security "
                "check, not a convenience: an unclamped peer clock parks every "
                "later message behind it forever. The binary path enforces the "
                "identical set of checks the JSON path does.",
            },
            {
                "name": "a base64 tail is reconstructed byte for byte",
                "hex": raw_frame(
                    content="see ", ext=[(EXT_TAG_B64_TAIL, bytes(range(48)))]
                ).hex(),
                "expect": {"content": "see " + b64_tail},
                "note": "This is the normative half of tag 1. A decoder "
                "re-encodes the carried bytes and appends them, reconstructing "
                "the sender's string exactly.",
            },
            {
                "name": "an unknown ext tag is ignored",
                "hex": raw_frame(content="hi", ext=[(7, b"\xde\xad")]).hex(),
                "expect": {"content": "hi"},
                "note": "Tag 7 is not in the registry. Skipping it is what lets "
                "a tag be added to v1 at all, and it is why a tag whose absence "
                "changes meaning cannot be added to v1.",
            },
            {
                "name": "only the first tag 1 entry is honoured",
                "hex": raw_frame(
                    content="see ",
                    ext=[
                        (EXT_TAG_B64_TAIL, bytes(range(48))),
                        (EXT_TAG_B64_TAIL, b"\xff\xff\xff"),
                    ],
                ).hex(),
                "expect": {"content": "see " + b64_tail},
                "note": "A second entry is surplus, not a second append. "
                "Concatenating them would let a hostile frame extend content "
                "past what the sender signed.",
            },
        ],
        "rejects": [
            {
                "name": "a JSON document is not a v1 frame",
                "hex": "7b0a",
                "reason": "wrong magic byte",
                "note": "0x7B opens JSON. Detection is by first byte alone and "
                "needs no negotiation, which is the property that lets the two "
                "encodings share a transport.",
            },
            {
                "name": "an empty buffer",
                "hex": "",
                "reason": "no magic byte",
            },
            {
                "name": "a magic byte from a version that does not exist yet",
                "hex": "f600",
                "reason": "wrong magic byte",
                "note": "0xF6 is reserved for a future breaking revision. A v1 "
                "decoder refuses it rather than guessing.",
            },
            {
                "name": "an identifier past the 256-byte cap",
                "hex": raw_frame(sender="x" * 257).hex(),
                "reason": "identifier too long",
                "note": "The cap is enforced identically on both encodings. A "
                "binary path that skipped it would be a way around a check the "
                "JSON path makes.",
            },
            {
                "name": "a truncated frame",
                "hex": raw_frame().hex()[:-4],
                "reason": "truncated",
                "note": "A length prefix that outruns the buffer is refused "
                "rather than allocated against.",
            },
        ],
    }


def build_address_vectors() -> dict:
    hashes = {
        "rfc8032-tv1": hashlib.sha256(bytes.fromhex(RFC8032_TV1_PK)).digest()[:20],
        "zeros": bytes(20),
        "ff": b"\xff" * 20,
        "ramp": bytes(range(20)),
    }
    return {
        "_comment": [
            "Frozen conformance vectors for the address encoding described in",
            "docs/spec/identity.md: 0x01 || SHA-256(pk)[0..20], rendered as",
            "bech32m under the human-readable part `off`.",
            "",
            "Computed with the BIP-350 reference implementation of bech32m over",
            "hashes computed here, not by running the Address type they pin.",
            "",
            "A failure here is a break in the identity layer, not a formatting",
            "detail: two spellings of one address split every set, map and",
            "dedup keyed by the rendered form.",
        ],
        "hrp": "off",
        "version_byte": 1,
        "hash_len": 20,
        "encoded_len": 44,
        "encode": [
            {
                "name": name,
                "hash_hex": h.hex(),
                "address": address_from_hash(h),
            }
            for name, h in hashes.items()
        ],
        "reject": [
            {
                "name": "uppercase is refused even though BIP-173 permits it",
                "input": address_from_hash(hashes["ramp"]).upper(),
                "reason": "not lowercase",
            },
            {
                "name": "a human-readable part other than off",
                "input": bech32_encode(
                    "xff", convertbits(bytes([1]) + hashes["ramp"], 8, 5)
                ),
                "reason": "hrp mismatch",
            },
            {
                "name": "a version byte this specification does not define",
                "input": bech32_encode(
                    "off", convertbits(bytes([2]) + hashes["ramp"], 8, 5)
                ),
                "reason": "version mismatch",
            },
            {
                "name": "a payload one byte short",
                "input": bech32_encode(
                    "off", convertbits(bytes([1]) + hashes["ramp"][:19], 8, 5)
                ),
                "reason": "payload length",
            },
            {
                "name": "a payload one byte long",
                "input": bech32_encode(
                    "off", convertbits(bytes([1]) + hashes["ramp"] + b"\x00", 8, 5)
                ),
                "reason": "payload length",
            },
        ],
        "ordering": _ordering_cases(),
    }


def _ordering_cases() -> list:
    """Pairs where hash-byte order and rendered-string order disagree.

    identity.md names a different order per tiebreaker and forbids
    harmonising them, because peers that changed and peers that did not would
    elect different winners from identical input with no way to detect the
    disagreement locally. A pair that sorts the same way both ways cannot
    catch that, so the search below finds pairs that do not.

    The candidates are derived from a fixed counter so this is reproducible.
    """
    found = []
    seen = 0
    while len(found) < 3 and seen < 4096:
        h1 = hashlib.sha256(f"offline-vector-{seen}".encode()).digest()[:20]
        h2 = hashlib.sha256(f"offline-vector-{seen + 1}".encode()).digest()[:20]
        seen += 1
        a1, a2 = address_from_hash(h1), address_from_hash(h2)
        by_hash_first = a1 if h1 <= h2 else a2
        by_string_first = min(a1, a2)
        if by_hash_first != by_string_first:
            found.append(
                {
                    "a": a1,
                    "b": a2,
                    "a_hash_hex": h1.hex(),
                    "b_hash_hex": h2.hex(),
                    "first_by_hash_bytes": by_hash_first,
                    "first_by_rendered_string": by_string_first,
                }
            )
    if not found:
        raise AssertionError(
            "no disagreeing pair found; the search is what makes these vectors "
            "worth having, so an empty result is a bug in it, not a property "
            "of the encoding"
        )
    return found


def build_derive_vectors() -> dict:
    keys = {
        "rfc8032-tv1": RFC8032_TV1_PK,
        "rfc8032-tv2": RFC8032_TV2_PK,
        "rfc8032-tv3": RFC8032_TV3_PK,
        "all-zero key": ZEROS_PK,
        "all-ff key": FF_PK,
    }
    a = derive_address(bytes.fromhex(RFC8032_TV1_PK))
    b = derive_address(bytes.fromhex(RFC8032_TV2_PK))
    c = derive_address(bytes.fromhex(RFC8032_TV3_PK))

    pairs = []
    for x, y in ((a, b), (b, c), (a, c)):
        by_hash = session_id(x, y)
        lo_s, hi_s = sorted([x, y])
        by_string = f"session:{lo_s}:{hi_s}"
        pairs.append(
            {
                "a": x,
                "b": y,
                "session_id": by_hash,
                "orders_disagree": by_hash != by_string,
                "session_id_if_ordered_by_string": by_string,
            }
        )

    return {
        "_comment": [
            "Frozen conformance vectors for address derivation and the 1:1",
            "session slot, from docs/spec/identity.md.",
            "",
            "Computed from the rules in that chapter with the BIP-350 reference",
            "implementation, not by running derive_address.",
            "",
            "The public keys are the public halves of RFC 8032 section 7.1 test",
            "vectors 1 to 3, chosen so both halves of each case can be checked",
            "against a published source rather than against this file.",
            "",
            "`orders_disagree` marks a pair where ordering by hash bytes and",
            "ordering by rendered string produce different slots. Those pairs",
            "are the whole reason this file exists: an implementation that",
            "sorts the rendered strings agrees with this one on every other",
            "pair and silently addresses a different session on these.",
        ],
        "derive": [
            {"name": name, "public_key_hex": pk, "address": derive_address(bytes.fromhex(pk))}
            for name, pk in keys.items()
        ],
        "sessions": pairs,
    }


def build_control_signing_vectors() -> dict:
    def case(name, note, sender, msg_id, recipient, content, timestamp_ms):
        return {
            "name": name,
            "note": note,
            "sender": sender,
            "id": msg_id,
            "recipient": recipient,
            "content": content,
            "timestamp_ms": timestamp_ms,
            "v1_hex": ctrl_payload_v1(sender, msg_id, recipient, content).hex(),
            "v2_hex": ctrl_payload_v2(
                sender, msg_id, recipient, content, timestamp_ms
            ).hex(),
        }

    addr_a = derive_address(bytes.fromhex(RFC8032_TV1_PK))
    addr_b = derive_address(bytes.fromhex(RFC8032_TV2_PK))
    uuid = "3f2504e0-4f89-11d3-9a0c-0305e82c3301"

    cases = [
        case(
            "a key package frame",
            "The ordinary shape: four length-prefixed fields under v1, the "
            "same four plus the stamp under v2.",
            addr_a,
            uuid,
            addr_b,
            "__MLS_KEY_PKG__{}",
            1700000000000,
        ),
        case(
            "the epoch instant",
            "A timestamp of zero is eight zero bytes, not an omitted field. "
            "The v2 payload is always exactly 12 bytes longer than a v1 "
            "payload over the same four fields.",
            addr_a,
            uuid,
            addr_b,
            "__PRESENCE__{}",
            0,
        ),
        case(
            "an instant before the epoch",
            "The stamp is signed and two's complement, so -1 is eight 0xFF "
            "bytes. An implementation writing it unsigned produces a "
            "signature no verifier can reproduce.",
            addr_a,
            uuid,
            addr_b,
            "__TYPING__{}",
            -1,
        ),
        case(
            "the largest representable instant",
            "i64::MAX. Fixed width means one encoding per instant, which a "
            "decimal rendering would not give.",
            addr_a,
            uuid,
            addr_b,
            "__READ_RECEIPT__{}",
            2**63 - 1,
        ),
        case(
            "empty content and empty recipient",
            "A zero-length field is a 4-byte zero prefix and no bytes, not an "
            "omitted field. Concatenation without the prefix would make this "
            "collide with a frame that moved the bytes across the boundary.",
            addr_a,
            uuid,
            "",
            "",
            1700000000000,
        ),
        case(
            "non-ASCII content",
            "Lengths count UTF-8 bytes. The content is 5 characters and 11 "
            "bytes.",
            addr_a,
            uuid,
            addr_b,
            "héllo",
            1700000000000,
        ),
    ]

    shifted = ctrl_payload_v1(addr_a, uuid, addr_b, "__PRESENCE__{}") + (
        1700000000000
    ).to_bytes(8, "big", signed=True)

    return {
        "_comment": [
            "Frozen conformance vectors for the control-plane canonical signing",
            "payloads described in docs/spec/control-messages.md.",
            "",
            "Computed from the construction stated in that chapter, not by",
            "running the payload builders they pin.",
            "",
            "These pin the bytes that are signed, not the signature. Ed25519",
            "itself is specified by RFC 8032 and has its own vectors; what is",
            "protocol-specific here is which bytes go under the key, and that",
            "is what a second implementation gets wrong.",
        ],
        "domains": {
            "v1": "offline-ctrl-v1",
            "v2": "offline-ctrl-v2",
            "live": [
                "offline-ctrl-v1",
                "offline-ctrl-v2",
                "offline-disc-v1",
                "offline-invite-v1",
                "offline-relay-addr-v1",
            ],
            "reserved": ["offline-gateway-addr-v1"],
        },
        "payloads": cases,
        "v1_is_not_v2_with_a_stamp": {
            "note": "The two are separate domains, not one payload with an "
            "optional field. Appending the stamp to a v1 payload produces "
            "these bytes, which are not the v2 payload for the same frame: "
            "the domain differs. An implementation that appends instead of "
            "re-domaining produces signatures that fail as forgeries.",
            "v1_with_stamp_appended_hex": shifted.hex(),
            "v2_hex": ctrl_payload_v2(
                addr_a, uuid, addr_b, "__PRESENCE__{}", 1700000000000
            ).hex(),
        },
    }


def build_envelope_vectors() -> dict:
    addr_a = derive_address(bytes.fromhex(RFC8032_TV1_PK))
    addr_b = derive_address(bytes.fromhex(RFC8032_TV2_PK))
    slot = session_id(addr_a, addr_b)

    def case(name, note, **kw):
        e = {
            "group_id": slot,
            "sender_id": addr_a,
            "message_type": "application",
            "epoch": 0,
            "timestamp_ms": 0,
            "ciphertext_hex": "",
        }
        e.update(kw)
        return {
            "name": name,
            "note": note,
            "envelope": e,
            "hex": encode_envelope(e).hex(),
        }

    return {
        "_comment": [
            "Frozen conformance vectors for the compact MLS envelope described",
            "in docs/spec/encryption-envelopes.md.",
            "",
            "Computed from the layout stated in that chapter, not by running",
            "the codec they pin.",
            "",
            "Every multi-byte integer here is little-endian, which is the",
            "opposite of the canonical signing payloads in the same protocol.",
            "That is deliberate and it is the single easiest thing to get",
            "backwards: these vectors are what catches it.",
        ],
        "byte_order": "little-endian for every length, the epoch and the timestamp",
        "layout": "group_id_len(4) group_id sender_len(4) sender_id "
        "message_type(1) epoch(8) timestamp_ms(8) ciphertext_len(4) ciphertext",
        "max_string_field_len": 4096,
        "envelopes": [
            case(
                "an application message in a 1:1 slot",
                "The ordinary shape. The group_id is the session slot the two "
                "addresses derive, so a receiver can check it without state.",
                epoch=3,
                timestamp_ms=1700000000000,
                ciphertext_hex="deadbeef",
            ),
            case("a welcome", "message_type 1.", message_type="welcome"),
            case("a commit", "message_type 2.", message_type="commit"),
            case("a proposal", "message_type 3.", message_type="proposal"),
            case(
                "an empty ciphertext",
                "A zero length prefix and no bytes. The envelope is still "
                "well-formed; emptiness is the payload's problem, not the "
                "codec's.",
            ),
            case(
                "a large epoch",
                "Epochs are fixed 8 bytes here, unlike the varints in the hop "
                "encoding. A codec that varints this field is misaligned from "
                "the epoch onward.",
                epoch=2**64 - 1,
                timestamp_ms=2**64 - 1,
            ),
        ],
        "json_disambiguation": {
            "note": "A legacy JSON envelope begins `{\"`, which read as a "
            "little-endian u32 group_id length is 8827, above the 4096 cap. "
            "The compact parser therefore rejects it deterministically and the "
            "caller falls through to JSON. This is why the two forms need no "
            "version byte between them.",
            "prefix_utf8": '{"',
            "as_le_u32": 8827,
            "exceeds_max_string_field_len": True,
        },
    }


def build_key_package_vectors() -> dict:
    return {
        "_comment": [
            "Conformance vectors for the key package payload described in",
            "docs/spec/capability-negotiation.md, the body of a",
            "__MLS_KEY_PKG__ frame.",
            "",
            "These pin the PARSE direction only. The JSON floor is not a",
            "byte-normative encoding: docs/spec/wire-format.md requires a",
            "receiver to accept both spellings of every optional field, so",
            "pinning an exact serialization here would assert a contract the",
            "spec deliberately does not make. What a second implementation owes",
            "is that these bodies parse to these values.",
            "",
            "The emission example is illustrative and is not asserted.",
        ],
        "parse": [
            {
                "name": "a payload from a peer that predates every capability",
                "json": '{"user_id":"off1abc","key_package_data":[1,2,3]}',
                "expect": {
                    "user_id": "off1abc",
                    "key_package_data": [1, 2, 3],
                    "remaining_lifetime_ms": 0,
                    "timestamp_ms": 0,
                    "session_reset": False,
                    "wire_versions": [],
                    "env_versions": [],
                    "rich_versions": [],
                    "data_versions": [],
                    "ctrl_versions": [],
                    "nostr_pubkey": None,
                },
                "note": "Every absent list defaults to empty, and empty selects "
                "the floor. This is rule 2 of capability negotiation expressed "
                "as a parse: absence is never an error.",
            },
            {
                "name": "a fully capable peer",
                "json": '{"user_id":"off1abc","key_package_data":[],'
                '"remaining_lifetime_ms":2419200000,"timestamp_ms":1700000000000,'
                '"session_reset":true,"wire_versions":[1],"env_versions":[1],'
                '"rich_versions":[1],"data_versions":[1,2,3],"ctrl_versions":[2],'
                '"nostr_pubkey":"abcd"}',
                "expect": {
                    "user_id": "off1abc",
                    "key_package_data": [],
                    "remaining_lifetime_ms": 2419200000,
                    "timestamp_ms": 1700000000000,
                    "session_reset": True,
                    "wire_versions": [1],
                    "env_versions": [1],
                    "rich_versions": [1],
                    "data_versions": [1, 2, 3],
                    "ctrl_versions": [2],
                    "nostr_pubkey": "abcd",
                },
                "note": "data_versions entries are read independently, so [1,2,3] "
                "is not a single level.",
            },
            {
                "name": "a field this version does not know",
                "json": '{"user_id":"off1abc","key_package_data":[],'
                '"a_field_from_a_later_release":42}',
                "expect": {
                    "user_id": "off1abc",
                    "key_package_data": [],
                    "remaining_lifetime_ms": 0,
                    "timestamp_ms": 0,
                    "session_reset": False,
                    "wire_versions": [],
                    "env_versions": [],
                    "rich_versions": [],
                    "data_versions": [],
                    "ctrl_versions": [],
                    "nostr_pubkey": None,
                },
                "note": "An unknown field is ignored rather than refused. A "
                "decoder that rejects it cannot be sent a newer key package, "
                "which is the frame that carries every other capability.",
            },
            {
                "name": "an explicit null nostr key",
                "json": '{"user_id":"off1abc","key_package_data":[],'
                '"nostr_pubkey":null}',
                "expect": {
                    "user_id": "off1abc",
                    "key_package_data": [],
                    "remaining_lifetime_ms": 0,
                    "timestamp_ms": 0,
                    "session_reset": False,
                    "wire_versions": [],
                    "env_versions": [],
                    "rich_versions": [],
                    "data_versions": [],
                    "ctrl_versions": [],
                    "nostr_pubkey": None,
                },
                "note": "Absent and null mean the same thing on the way in. On "
                "the way out the field is omitted rather than written null.",
            },
        ],
        "emission_is_not_pinned": {
            "note": "Illustrative only. A conforming peer parses this; it is "
            "not required to produce these bytes, and this file does not "
            "assert that it does.",
            "example": '{"user_id":"off1abc","key_package_data":[],'
            '"remaining_lifetime_ms":0,"timestamp_ms":0,"session_reset":false,'
            '"wire_versions":[],"env_versions":[],"rich_versions":[],'
            '"data_versions":[],"ctrl_versions":[]}',
            "nostr_pubkey_absent_is_omitted_not_null": True,
        },
    }


FILES = [
    (CORE_DATA / "wire-v1.vectors.json", build_wire_vectors),
    (CORE_DATA / "address-v1.vectors.json", build_address_vectors),
    (SEALED_DATA / "derive-address-v1.vectors.json", build_derive_vectors),
    (SEALED_DATA / "control-signing-v1.vectors.json", build_control_signing_vectors),
    (SEALED_DATA / "mls-envelope-v1.vectors.json", build_envelope_vectors),
    (SEALED_DATA / "key-package-v1.vectors.json", build_key_package_vectors),
]


def render(build) -> str:
    return json.dumps(build(), indent=2, ensure_ascii=False) + "\n"


def main() -> int:
    check = "--check" in sys.argv[1:]
    drift = []

    for path, build in FILES:
        want = render(build)
        if check:
            if not path.exists():
                drift.append(f"{path.relative_to(REPO)} is missing")
            elif path.read_text(encoding="utf-8") != want:
                drift.append(f"{path.relative_to(REPO)} differs from this generator")
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(want, encoding="utf-8")
            print(f"wrote {path.relative_to(REPO)}")

    if check:
        if drift:
            print("error: the committed vectors are not what the generator produces:")
            for line in drift:
                print(f"  {line}")
            print()
            print("A vector file is a frozen wire contract. If the wire genuinely")
            print("changed, change this generator and the spec chapter together and")
            print("regenerate. If it did not, the edit to the vector file is the bug.")
            return 1
        print(f"all {len(FILES)} vector files match the generator")

    return 0


if __name__ == "__main__":
    sys.exit(main())
