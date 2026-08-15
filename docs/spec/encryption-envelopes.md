# Encryption envelopes

Three envelope formats carry ciphertext, at three different layers. A message
may take any combination of them.

| Envelope | Layer | Carried in | Negotiated by |
|----------|-------|-----------|---------------|
| `__MLS_ENC__` | End to end, 1:1 and group | `content` | `env_versions` |
| Media chunk envelope | End to end, media | `binary_content` | `rich_versions` for the v2 form |
| `__RICH_V1__` sealed body | Inside the MLS plaintext | The decrypted plaintext | `rich_versions` |

They are not independently negotiated. Only `__MLS_ENC__` has a capability of
its own; the media envelope's v2 form and the sealed rich body **share
`rich_versions`**, so advertising it enables both, and a change to what that
token means moves two envelopes at once.

## The MLS encrypted message

All three envelope forms ultimately carry the same structure: an MLS ciphertext
plus the routing information a receiver needs before it can decrypt.

| Field | Purpose |
|-------|---------|
| `group_id` | Which MLS group or 1:1 session slot this belongs to |
| `sender_id` | Claimed sender |
| `message_type` | Application message, commit, welcome, proposal |
| `epoch` | MLS epoch the ciphertext was sealed at |
| `timestamp_ms` | Wall clock, display only |
| `ciphertext` | The MLS message |

### Compact binary encoding

```
u32le len(group_id)   || group_id  (UTF-8)
u32le len(sender_id)  || sender_id (UTF-8)
u8    message_type
u64le epoch
u64le timestamp_ms
u32le len(ciphertext) || ciphertext
```

All integers are little-endian. Decoders MUST bounds-check every length against
the remaining buffer before reading, and MUST NOT trust a length prefix to be
consistent with the buffer size.

## The `__MLS_ENC__` envelope

The prefix is followed by one of three forms. A receiver distinguishes the first
by the **byte immediately after the prefix**, and the other two by attempting
them in order:

| First byte after prefix | Form |
|-------------------------|------|
| `{` | Legacy JSON, parsed directly |
| anything else | Base64. Decode, then try the compact binary encoding, and fall back to JSON inside the base64 |

Base64 output never begins with `{`, so separating the first row is total. The
two base64 forms are separated by trying the compact encoding first, which is
safe rather than merely conventional: base64-wrapped JSON decodes to bytes
starting `{"`, which read as a group identifier length far above the compact
decoder's 4 KB cap, so it is rejected deterministically and falls through.

A conforming receiver MUST accept all three. Emitting the base64-wrapped JSON
form is not required.

**Parsing is unconditional.** A receiver accepts every historical form
regardless of what it advertised. Capability negotiation governs only what a
sender emits. An implementation that gates parsing on its own advertisement will
drop messages from peers that legitimately believed it capable.

The compact form is roughly 2.7 times smaller than the JSON form, because the
JSON form renders the ciphertext as a decimal integer array at about 3.6 bytes
per byte.

### Envelope slot binding

Before attempting decryption of a 1:1 envelope, a receiver MUST check that the
envelope's `group_id` is the session slot it shares with the **claimed sender**,
computed as described in [Identity](identity.md).

An envelope naming any other slot is refused without decryption. Without this
check, one derivable session identifier could be aimed at arbitrary peers.

## The media chunk envelope

File chunks ride in `binary_content`. Encrypted chunks are wrapped so a receiver
can tell them from legacy plaintext chunks and apply policy.

```
[magic: 2 bytes = "ML"][version: 1 byte][compact EncryptedMessage bytes]
```

| Version | Meaning |
|---------|---------|
| `0x01` | Payload is an MLS encrypted message; plaintext carries metadata and original content type |
| `0x02` | Same payload; the plaintext may additionally carry rich extras |

### Disambiguation from legacy plaintext chunks

Legacy plaintext chunks are raw serialized file chunks, which begin with a file
identifier length as a little-endian `u32` capped at 4096. Their second byte is
therefore the high byte of that length, at most `0x10`, which can never equal
`0x4C` (`L`). The two formats are unambiguous without a version negotiation.

### The sealed plaintext layout

```
[flags: 1 byte]
[if flags & 0x01: u32le meta_len][media metadata JSON]
[if flags & 0x02: u8 oct_len][original content type string]
[if flags & 0x04: u32le rich_len][media rich extras JSON]
[chunk bytes: remainder]
```

Flag bits are **not additively safe on their own.** A decoder that ignored an
unknown bit would slurp that field's bytes as chunk data and silently corrupt
the file. Therefore:

- Any chunk carrying a flag beyond a receiver's known set MUST ship under a
  bumped envelope version, so an old decoder rejects it cleanly at the version
  check instead of misparsing.
- A decoder MUST also reject unknown flag bits outright, as a backstop.

Rich extras (caption, reply threading, quoted-reply context, forward
attribution) ship only on chunk 0, only under envelope v2, and only toward
recipients that advertised rich payload support. Additive fields go inside the
rich extras structure itself, which is self-describing JSON and needs no new
flag bit or envelope version.

### What the envelope moved inside the AEAD boundary

Before this envelope existed, the chunk-0 message leaked media metadata in
cleartext, including the file name and the preview thumbnail, plus the original
content type. Those now travel in the sealed plaintext. An implementation that
also writes them to the outer message reopens the leak.

## The sealed rich payload

Rich message extras travel inside the MLS AEAD boundary, wrapped around the text
before encryption:

```
__RICH_V1__ + JSON of:
  {
    text:           string,
    reply_context:  optional,
    media_metadata: optional,
    forward_info:   optional,
    content_type:   optional
  }
```

### Why it exists

The outer message fields are visible to every relay and every forwarding hop.
The rich fields include quoted-reply previews (which quote another message's
content), media metadata (which for cloud media includes the **encryption key
and initialization vector**), and forward attribution (which names the original
sender). None of that belongs on a relay-visible field.

### Restore rules

On receipt, a sealed body is **authoritative** over the outer copies.
Specifically:

1. Strip the outer reply context unconditionally, sealed body or not. A
   relay-visible quoted-reply preview is never trusted and never rendered.
2. If the plaintext carries a `__RICH_V1__` body, parse it and replace the rich
   fields **wholesale**, absent values included, so a sealed body that omits a
   field clears the outer copy rather than letting it show through.
3. Absent a sealed body, the remaining outer fields (media metadata, forward
   attribution) **survive**. That is the deliberate fallback for senders that
   predate sealing, and it is why rule 2 is scoped to the sealed branch rather
   than applied as an unconditional wipe.
4. The sealed `content_type` hint, when present, overrides the outer value.

Rule 4 closes a specific attack: without it a relay could restamp the rendering
hint in transit, and restamping it to `FileChunk` routes the decrypted message
into the file-transfer manager where it is dropped. A sealed `FileChunk` claim
is therefore **refused** on restore, mirroring the send boundary which refuses
it too.

The `content_type` field is additive. Bodies sealed by older senders omit it, in
which case the outer value stands.

### Parsing is unconditional, sealing is gated

As with the envelope, a receiver tries to parse whatever a peer chose to seal. A
parse failure surfaces the raw text plus a warning rather than dropping an
authenticated message.

Sealing is gated on the recipient having advertised rich payload support. Toward
a non-capable recipient the extras are **silently dropped, never sent in
cleartext**. That is the whole point: a downgrade must lose the feature, not the
confidentiality.

### Hint-only bodies

A fresh send with a non-`Text` content type seals a body carrying only the hint,
even with no extras present. In groups this is mandatory rather than
belt-and-braces: the group payload has no outer content-type carrier, so an
unsealed hint is lost entirely, not merely unprotected.

### Forwards

Forwarding seals the attribution and the original media metadata as extras
toward capable recipients. That is the only way a forwarded cloud media message
keeps its encryption key and initialization vector.

Cleartext outer copies are kept as the legacy fallback for non-capable
recipients, with the secrets stripped at the wire boundary. A sealed restore
overwrites them wholesale.

### Size bound

Serialized rich extras are capped (32 KiB in the reference implementation).

The cap MUST be enforced at the **API boundary**, not at seal time. A message
queued behind session establishment re-makes the seal decision when it flushes,
and a seal-time failure there would re-queue the message forever. Bounding at
the boundary means every queued blob is already known to seal.

## Group sealing gate

In a group, the sealed body is used only when **every** other member is known to
support it. A single unknown member fails the gate closed and the extras drop.

Capability is established two ways:

1. **Directly**, from a member's own advertised capability.
2. **By inviter attestation.** The Add commit carries the affected member's
   capability to existing members, and the Welcome carries a capability map to
   the joiner. Attestation chains across successive adds; direct exchange always
   overrides it.

Attestation entries are bounded to the joined MLS roster and admin-gated on the
commit. Attestation feeds **only** the group sealing gate, never 1:1 sealing and
never envelope selection.

When the gate fails, the implementation reports which members were unknown and
backfills by pushing a key package to them once. Their automatic reply reopens
the gate, which is the recovery path for groups formed before attestation
existed.
