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

An unrecognised value degrades to `Medium`.

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
rather than encoded as null where the field is absent.

Validation on decode is not optional. Identifier length caps and the Lamport
clock clamp are security checks, not conveniences, and the binary path is
required to enforce the identical set.

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

Three deliberate choices in that layout:

- **The id is 16 raw bytes, not a 36-character hyphenated string.** This is the
  single largest saving on short messages.
- **Rich structures ride as opaque JSON blobs.** Media metadata and forward
  attribution are rare and structurally complex. Carrying them as embedded JSON
  keeps the frozen surface small and lets those structures keep evolving through
  their own additive rules without touching the wire contract.
- **The metadata map is sorted by key.** A hash map iterates
  nondeterministically, which would make the encoding non-reproducible.

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

Chunk sizes are transport-dependent, because the constraint is the transport MTU
and duty cycle, not the protocol:

| Transport class | Chunk size | In-flight window |
|-----------------|-----------|------------------|
| Bluetooth LE | 4 KiB | 2 |
| Default | 32 KiB | 4 |
| Internet | 256 KiB | 8 |

The encoding choice interacts with this directly. An encrypted direct message
under the compact envelope and the binary codec is roughly 2.7 times smaller
than the same message under the JSON floor with the legacy envelope, which is
often the difference between one Bluetooth LE fragment and three.

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

Applications MUST NOT write these keys.
