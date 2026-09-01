# Message model and wire format

## The abstract message

Every frame the protocol puts on a transport is one message. The abstract
message is the same regardless of which encoding carries it.

| Field | Type | Notes |
|-------|------|-------|
| `id` | 128-bit UUID | Identity for deduplication, acknowledgement, and outbox tracking |
| `sender` | user identifier | Canonically an address; see [Identity](identity.md) |
| `recipient` | user identifier | Canonically an address |
| `app_id` | string | Namespaces traffic between applications sharing a mesh |
| `priority` | enum | `Low`, `Medium`, `High`, `Critical` |
| `ttl` | unsigned | Hops remaining; a message with 1 or fewer is not forwarded |
| `hop_count` | unsigned | Hops traversed so far |
| `timestamp` | signed integer, milliseconds | Wall clock, for display only, never for ordering decisions |
| `lamport_clock` | unsigned | Logical clock for causal ordering across devices |
| `content_type` | enum | Rendering hint; see the mapping table below |
| `content` | string | Text, a control prefix plus its body, or an envelope |
| `binary_content` | optional bytes | File chunk payloads and media envelopes |
| `media_metadata` | optional structure | Present for non-text content |
| `metadata` | string map | Application use plus a small set of reserved keys |
| `requires_ack` | boolean | Default true; see [Delivery and ACKs](../state-machines/delivery-and-acks.md) |
| `reply_to_msg` | optional UUID | Threading |
| `forwarded_from` | optional structure | Forwarding attribution |
| `reply_context` | optional structure | Quoted-reply preview |

Two further fields exist in the reference implementation and are deliberately
**not** part of the wire model:

- **transport peer identity.** Stamped by the transport layer on receipt with
  the physically verified peer that delivered the frame. It exists in process
  only, is never serialized, and is what the control-plane gate compares the
  claimed `sender` against. An implementation MUST NOT accept this value from
  the wire, because doing so hands the spoofing check to the spoofer.
- **wire codec selection.** Which encoding to use on the next hop. Stamped from
  per-peer capability just before send.

### Content type mapping

The numeric mapping is a frozen wire contract:

| Value | Type |
|-------|------|
| 0 | Text |
| 1 | Image |
| 2 | Video |
| 3 | Audio |
| 4 | VoiceNote |
| 5 | VideoNote |
| 6 | File |
| 7 | FileChunk |
| 8 | Poll |

Adding a variant is an additive change. Every decode path MUST degrade an
unrecognised value to `File` rather than rejecting the containing message. That
applies to the string form, the binary numeric form, and the JSON form alike. A
JSON decoder built from a derived enum deserializer typically rejects unknown
variants and therefore does not conform; the fallback has to be written by hand.

### Priority mapping

| Value | Priority |
|-------|----------|
| 0 | Low |
| 1 | Medium |
| 2 | High |
| 3 | Critical |

An unrecognised **numeric** value degrades to `Medium`. Unlike content type,
that tolerance does not extend to the JSON encoding: priority is carried there
as a string against a closed set, with no fallback and no default, so an unknown
or absent value rejects the whole message. A new priority is therefore a
breaking change on the JSON floor, which is where a future addition has to be
designed around.

On the wire the value is always **lowercase**, so a wire decoder built to the
four lowercase names alone is conforming. The reference implementation's decoder
additionally accepts the **capitalized** spelling of each name (`Low`, `Medium`,
`High`, `Critical`), but that tolerance exists for its **FFI boundary** rather
than for the wire: the UniFFI `receive_message` JSON renders priority
Debug-cased, and that JSON has to round-trip back in through `forward_message`.
Anything re-parsing SDK-emitted FFI JSON needs the capitalized spellings; a peer
implementation does not.

## Encodings

A receiver distinguishes the two encodings by the **first byte alone**:

| First byte | Encoding |
|------------|----------|
| `0x7B` (`{`) | JSON |
| `0xF5` | Binary wire v1 |

The magic byte is drawn from `0xF5..=0xFF`. Those are invalid UTF-8 leading
bytes and cannot begin a JSON document, so detection needs no negotiation and no
out-of-band state. Future breaking revisions take the next value (`0xF6` = v2),
leaving room for eleven wire versions before the range is exhausted.

Detection is unconditional. Negotiation governs only what a sender **emits**.

### JSON encoding

The permanent floor. Every conforming receiver MUST accept it.

It is the sole encoding used for:

- persistence (outbox, pending queues, stored state),
- the internet relay path.

Field names are the abstract field names above. Optional fields are omitted
rather than encoded as null where the field is absent, with one exception:
`reply_to_msg` is always present and is `null` when there is no reply. A
receiver MUST accept both spellings for every optional field, since the
distinction carries no meaning.

Validation on decode is not optional. Identifier length caps and the Lamport
clock clamp are security checks, not conveniences, and the binary path is
required to enforce the identical set:

| Check | Bound | On breach |
|-------|-------|-----------|
| `sender`, `recipient`, `app_id` length | `256` bytes | Reject the message |
| Identifier contents | Non-empty, and never `.`, `..`, a control character, `/`, `\` or `:` | Reject the message |
| `lamport_clock` | 2^63 - 1 | Clamp, do not reject |

The Lamport clock clamps rather than rejecting because the value is a peer's
claim about ordering, not a structural error: a frame carrying `u64::MAX` is
well formed, and refusing it would let any peer make its own messages
undeliverable. Clamping bounds the damage instead, which matters because an
unclamped peer clock parks every later message behind it permanently.

### Binary wire v1

A compact positional encoding for hop-local use. It is emitted only to a peer
that advertised `wire_versions` containing `1`.

The frame is `0xF5` followed by a positionally encoded structure with this
**frozen** field order:

```
id                   16 raw bytes
sender               string
recipient            string
app_id               string
priority             u8
ttl                  u8
hop_count            u8
timestamp            i64
lamport_clock        u64
content_type         u8
content              string
binary_content       optional bytes
media_metadata_json  optional bytes   (media metadata serialized as JSON)
metadata             list of (string, string), sorted by key
requires_ack         bool
reply_to_msg         optional 16 raw bytes
forwarded_from_json  optional bytes   (forward attribution serialized as JSON)
ext                  list of (u16 tag, bytes)
```

#### Primitive encoding

The field order above is not sufficient to produce a frame. Each primitive in it
has exactly one encoding, and an implementation that writes fixed-width fields
produces frames this protocol cannot decode.

| Primitive | Encoding |
|-----------|----------|
| `u8` | One raw byte |
| `u16`, `u64`, and every length prefix | Varint |
| `i64` | Zigzag, then varint |
| `bool` | `0x00` false, `0x01` true |
| `string` | Varint byte length, then the UTF-8 bytes |
| `bytes` | Varint byte length, then the raw bytes |
| 16 raw bytes | Exactly 16 bytes, **no length prefix** |
| `optional T` | `0x00` alone when absent; `0x01` followed by `T` when present |
| `list of T` | Varint element count, then each element in order |
| A tuple, such as a metadata pair or an `ext` entry | Its members concatenated in order, with no count and no framing of their own |

A **varint** is little-endian base 128: seven data bits per byte, with the high
bit set on every byte except the last. Zero is `0x00`, 127 is `0x7F`, 128 is
`0x80 0x01`, and 300 is `0xAC 0x02`.

**Zigzag** maps a signed integer onto an unsigned one before the varint is
taken, so a small magnitude stays short in both directions. The mapping is
`(n << 1) ^ (n >> 63)` with an arithmetic right shift, giving 0 to 0, -1 to 1,
1 to 2, and -2 to 3. A timestamp of zero is therefore one byte, not eight.

Two consequences are worth stating, because each is a way a plausible
implementation goes wrong silently:

- **The 16-byte fields are the exception, not the rule.** `id` and
  `reply_to_msg` are fixed width and carry no length prefix, while every other
  variable-length field carries one. A decoder that prefixes the id, or that
  omits the prefix on `sender`, is misaligned from that point on and reads every
  later field as garbage rather than failing where the mistake was made.
- **The `ext` tag is a varint, not two fixed bytes.** Tags 1 and 2 each occupy a
  single byte on the wire. The registry below calls the tag a `u16` to state its
  value range, not its width.

A length prefix is the varint of a pointer-sized unsigned integer, but the
encoding is value-identical on every platform: nothing in this protocol emits a
length a 32-bit implementation cannot represent. A receiver MUST reject a length
prefix that exceeds the bytes remaining in the buffer, and MUST NOT allocate on
the strength of one before checking it.

These rules coincide with version 1 of
[the postcard wire format](https://postcard.jamesmunns.com/wire-format), which
is the encoding the reference implementation uses. That document is where these
rules come from; it is not what makes them binding. The table above is normative
here, and a second implementation conforms to it rather than to any particular
library.

Three deliberate choices in that layout:

- **The id is 16 raw bytes, not a 36-character hyphenated string.** This is the
  single largest saving on short messages.
- **Rich structures ride as opaque JSON blobs.** Media metadata and forward
  attribution are rare and structurally complex. Carrying them as embedded JSON
  keeps the frozen surface small and lets those structures keep evolving through
  their own additive rules without touching the wire contract.
- **The metadata map is sorted by key.** A hash map iterates
  nondeterministically, which would make the encoding non-reproducible. The
  order is a byte-wise comparison of the UTF-8 keys, so it depends on neither a
  locale nor a Unicode collation table. Sorting decoded code points instead
  agrees for ASCII keys and diverges above it, which is the shape of bug that
  ships because every test key was ASCII.

#### Why a separate DTO

The abstract message carries defaulting rules, skip-if-absent rules, and
validation-on-deserialize behaviour that a non-self-describing positional format
cannot honour field for field. Conforming implementations therefore encode
through a flat fixed-order intermediate structure and convert back through the
**validating** constructors, so the checks the JSON path enforces stay intact on
the binary path.

#### Evolution contract

A positional format silently corrupts decoding on peers running the previous
layout if a field is reordered, removed, retyped, or inserted. Therefore:

1. Existing fields MUST NOT change, in order or in type.
2. Additive, backward-compatible data goes into `ext`, a trailing tagged list
   that old decoders read and ignore.
3. A change that cannot be expressed as an `ext` entry requires a new magic
   byte and out-of-band version negotiation.

### The `ext` TLV registry

| Tag | Meaning | Absence-safe? |
|-----|---------|---------------|
| 1 | Trailing base64 run of `content`, carried decoded | No, see below |
| 2 | Quoted-reply context serialized as JSON | Yes |

**Tag 1** exists because envelopes are base64 and base64 inflates by 4/3. When
`content` ends in a long canonical base64 run, the wire `content` keeps only the
head and the decoded tail rides in the TLV; the decoder re-encodes and appends,
reconstructing the original string byte for byte.

The split is taken only when the encoder has verified that reconstruction
property by re-encoding and comparing. That makes it correct by construction for
arbitrary input: non-canonical padding, foreign alphabets, and lookalike text
simply fail the comparison and the content rides as plain text. The minimum
tail length before the split is worth taking is 64 base64 characters (48 raw
bytes).

Tag 1 constrains the registry. It shipped in wire v1's **first** release, so
advertising `wire_versions` containing `1` implies understanding it. A decoder
that ignored tag 1 would reconstruct a truncated `content`, which is only safe
because no v1 decoder without tag-1 support ever shipped.

The rule that follows, and it is the important one: **a future tag whose absence
changes meaning cannot piggyback on v1.** It needs a new wire version. Tags may
only be added to v1 when ignoring them costs efficiency or optional context,
never correctness.

**Tag 2** is a correct additive tag under that rule. A decoder that skips it
delivers the message without its reply preview, which is exactly the degradation
a legacy JSON receiver applies by ignoring an unknown field. Decoders honour
only the first tag-2 entry and reject a frame whose payload is not valid reply
context, matching the JSON path where a malformed value rejects the message.

## Size and fragmentation

Two different things are called splitting here and they belong to different
layers. **Chunks** are how a large payload is divided before it becomes
messages at all, and the sizes below are theirs. **Fragments** are how one
message is sliced to fit a single radio write, which is transport framing and
is specified per carrier: for Bluetooth LE, in
[Bluetooth LE framing](ble-framing.md). A 4 KiB chunk on a Bluetooth LE link
still becomes many fragments.

Chunk sizes are transport-dependent, because the constraint is the transport MTU
and duty cycle, not the protocol:

| Transport class | Chunk size | In-flight window |
|-----------------|-----------|------------------|
| Bluetooth LE | 4 KiB | 2 |
| Default | 32 KiB | 4 |
| Internet | 256 KiB | 8 |

The encoding choice interacts with this directly. Measured on one encrypted
direct message, the JSON floor with the legacy envelope takes 1342 bytes; the
compact envelope alone brings that to 808 (the envelope is responsible for
roughly a 2.7 times reduction on the payload it replaces); adding the binary
codec brings it to 472, **about 2.8 times smaller** end to end. At the 185-byte
Bluetooth LE fragment size that is the difference between ten fragments and
four.

## Reserved metadata keys

The metadata map is application space with a small reserved set. Reserved keys
observed on the wire:

| Key | Meaning |
|-----|---------|
| `ack_for` | This message acknowledges the named message id |
| `ack_hop_count` | Hop count observed by the acknowledging party |
| `ack_transport` | Transport the acknowledged message arrived on |
| `transport_preference` | Requested transport for this message |
| `original_content_type` | Pre-chunking content type of a file transfer |
| `__ctrl_sig` | Base64 Ed25519 signature over the control-message canonical payload |
| `__ctrl_pk` | Base64 Ed25519 public key of the signer, 32 raw bytes |

Applications MUST NOT write these keys.

The last two are the control-plane signature and its verification key, described
in [Control messages](control-messages.md#the-control-plane-signature-gate). They are
security-relevant rather than merely reserved: an implementation that lets
application input reach them lets an application forge the control plane.
