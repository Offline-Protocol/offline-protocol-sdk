# Conformance

This chapter says what it means to implement this protocol, which of the other
chapters are mandatory, and how an implementation demonstrates that it got them
right.

The short version: **conformance is decided by the vectors, not by reading.**
Every other chapter describes a format in prose, and prose is where two readers
disagree without noticing. The vector files are the same formats as bytes.

## The two profiles

An implementation conforms as one of two profiles. They differ almost entirely
in what they must *emit*. The exception is the control payload, which is two
rows of the table below (`ctrl_versions` and the v1 payload on other control
frames) because it is one rule with two halves: a leaf verifies only
`offline-ctrl-v2`, so it MUST advertise `2` and MUST refuse the older payload
where a full node may accept it. That pair is the only place in this
specification where being smaller makes a requirement stricter, and
[the leaf profile](capability-negotiation.md#the-leaf-profile) explains why it
is affordable there and nowhere else.

| | Full node | Leaf |
|---|---|---|
| Runs the engine, relays, joins groups | Yes | No |
| Accepts the JSON encoding of a `Message` | MUST | MUST |
| Accepts all three inbound `__MLS_ENC__` forms | MUST | MUST |
| Decodes binary wire v1 | MAY | MAY |
| Emits binary wire v1 | Only to a peer advertising `wire_versions` `1` | Same |
| Emits the compact envelope | Only to a peer advertising `env_versions` `1` | Same |
| Advertises `ctrl_versions` containing `2` | SHOULD | MUST |
| Accepts `__MLS_KEY_PKG__` signed under `offline-ctrl-v1` | MUST | MUST |
| Accepts the v1 payload on other control frames | MAY, until that peer proves v2 | MUST NOT |
| Replicates documents | MAY | MAY |

Two rows are easy to get backwards, and both are stated where they belong
rather than inferred here:

- **Decoding binary wire v1 is optional; accepting the compact envelope is
  not.** They look like the same kind of capability and are not. An
  implementation that does not decode the hop encoding advertises an empty
  `wire_versions` and is sent JSON, which costs it nothing but frame size. The
  envelope has no such fallback on the receive side: a receiver
  [MUST accept all three forms](encryption-envelopes.md#the-__mls_enc__-envelope)
  regardless of what it advertised, because a peer that legitimately believed it
  capable will send one.
- **Emission is what negotiation governs, never parsing.** Every "emits" row
  above is a restriction on the sender. No row anywhere in this table lets an
  implementation refuse a form on the grounds that it did not advertise it.

Two more, on the control payload rows, because the qualifiers there are doing
work:

- **A leaf refuses the v1 payload unconditionally, not after some first
  exchange.** First contact is already covered by the row above it: a
  `__MLS_KEY_PKG__` frame under the v1 payload MUST be accepted by everyone,
  which is what lets a peer that has never seen this device's `ctrl_versions`
  reach it at all. Every *other* control frame a leaf receives MUST carry the v2
  payload from the first one onward. A first-contact exemption on those frames
  would be unauthenticatable, since nothing in a frame proves that it is the
  first, so any sender could claim it indefinitely.
- **A full node's MAY expires per peer.** It is not a standing permission: a
  verifier that has once verified a v2 signature from a peer MUST record that
  durably and refuse that peer's v1 control frames from then on, `__MLS_KEY_PKG__`
  excepted
  ([the downgrade ratchet](control-messages.md#2-negotiation-and-the-downgrade-ratchet)).
  Without the record, an attacker replays a capture made before the peer
  upgraded and the freshness binding is side-stepped.

The leaf profile is described in full in
[Capability negotiation](capability-negotiation.md#the-leaf-profile) and
[Leaf node provisioning](leaf-provisioning.md), which states why `ctrl_versions`
is the one capability whose floor a leaf cannot accept.

## What every implementation owes

These hold for both profiles, and each is a chapter in itself:

1. **Accept the JSON encoding of a `Message`, unconditionally**
   ([wire format](wire-format.md)). No negotiation, capability or configuration
   removes this obligation.
2. **Distinguish the encodings by the first byte alone.** `0x7B` opens JSON,
   `0xF5` opens binary wire v1. An implementation that does not decode binary v1
   still has to tell the two apart, so that it refuses a frame it cannot read
   rather than handing the bytes to a JSON parser and reporting a malformed
   message from a peer that sent a well-formed one.
3. **Derive and check addresses** ([identity](identity.md)). A claimed sender is
   checked against the key presented with it, by derivation. There is no
   trust-on-first-use store to fall back on.
4. **Refuse non-canonical address spellings.** Re-encode and compare, or refuse
   every non-canonical form explicitly.
5. **Gate the control plane on a signature**
   ([control messages](control-messages.md)), including the address-derivation
   step, which is what makes the gate mean anything.
6. **Reserve every prefix in the registry** on every public send surface.
   Without it, application text is a control-frame injection vector.
7. **Parse unconditionally and emit by capability**
   ([capability negotiation](capability-negotiation.md)). An absent capability
   selects the floor and is never an error.
8. **Drop a gated payload rather than weaken it.** A downgrade loses the
   feature, never the confidentiality.

## The vectors

Each vector file is the conformance surface for one chapter. They live in the
crate whose code they pin, so a packaged build carries its own vectors, and they
are computed independently of that code.

| Vectors | Chapter | Direction |
|---------|---------|-----------|
| `crates/offline-protocol-core/tests/data/wire-v1.vectors.json` | [Message model and wire format](wire-format.md) | Both |
| `crates/offline-protocol-core/tests/data/address-v1.vectors.json` | [Identity and addressing](identity.md) | Both |
| `crates/offline-protocol-sealed/tests/data/derive-address-v1.vectors.json` | [Identity and addressing](identity.md) | Encode |
| `crates/offline-protocol-sealed/tests/data/control-signing-v1.vectors.json` | [Control messages](control-messages.md) | Encode |
| `crates/offline-protocol-sealed/tests/data/gateway-address-proof-v1.vectors.json` | [The gateway contract](gateway-contract.md) | Encode |
| `crates/offline-protocol-sealed/tests/data/mls-envelope-v1.vectors.json` | [Encryption envelopes](encryption-envelopes.md) | Both |
| `crates/offline-protocol-sealed/tests/data/key-package-v1.vectors.json` | [Capability negotiation](capability-negotiation.md) | Parse |
| `crates/offline-protocol/tests/data/data-sync-v1.vectors.json` | [Document replication](data-sync.md) | Both |
| `crates/offline-protocol-transport/tests/data/ble-framing-v1.vectors.json` | [Bluetooth LE framing](ble-framing.md) | Both |

### How they are computed

`tools/spec-vectors/generate.py` is a second implementation of these encodings,
written from the chapters and forbidden from importing, linking against or
shelling out to the Rust crates it pins. Running it with `--check` regenerates
the seven files it owns (every row above except document replication and Bluetooth
LE framing) and fails on any difference, which is what CI does.

Those two remaining files predate the generator and were computed by hand from
their chapters, as each says in its own header. They carry the same independence
claim and the same rule below about what a failure means; what they do not have
is a job that reproduces them, so the claim rests on review.

The claim that buys is narrower than "independent", and it is worth stating
exactly rather than letting a reader assume the stronger version: **the values
are computed from the published rules, not by executing the code under test.**
For the binary frame, those rules and the reference implementation share an
ancestor in the published postcard wire format, which each was written against
separately.

What this rules out is the failure that matters. A vector generated by running
the encoder agrees with whatever that encoder emits, including a wrong format,
so it can never report a break. These can.

### When a vector fails

**The wire format changed.** That is the only thing a failure means.

The fix is a new version identifier and a negotiated capability, never an
edited expectation. Editing the expected value to make a test pass converts a
caught break into a shipped one, and every install already in the field still
speaks the old bytes.

If the vector is genuinely the thing that is wrong, the correction lands in
`tools/spec-vectors/generate.py` and in the chapter together, and the diff shows
a rule changing rather than a number changing. A change to a vector file alone
is not a legitimate diff. For the two hand-computed files there is no generator
to change, so the chapter edit is the whole of the justification and the review
is where it has to hold up.

### What the vectors deliberately do not cover

Stating this is part of the contract. A suite that silently omits things is
worse than one that says what it omits, because the gaps are invisible to
exactly the reader who most needs to know about them.

- **Signatures.** The control-signing vectors pin the bytes that go under the
  key, not any signature over them. Ed25519 is specified by RFC 8032 and carries
  its own vectors; what is specific to this protocol is which bytes are signed,
  and that is the half a second implementation gets wrong.
- **The JSON floor's exact serialization.** The floor is not byte-normative:
  [wire format](wire-format.md#json-encoding) requires a receiver to accept both
  spellings of every optional field. The key package vectors therefore pin the
  parse direction only. Pinning bytes there would assert a contract this
  specification deliberately declines to make.
- **Where the reference encoder splits a base64 tail.** A case marked
  `encoder_specific` pins a choice the wire does not require. A conforming
  encoder may split elsewhere or never split; what binds is that a decoder
  reconstructs the content exactly, which the decode cases pin.
- **The rich structures carried as embedded JSON.** Media metadata, forward
  attribution and reply context ride as opaque blobs by design, and evolve under
  their own additive rules rather than under the frozen wire contract.
- **Size bounds too large to express.** The Bluetooth LE 1 MiB ceiling needs
  seventeen full frames to reach; a vector file carrying them would be megabytes
  of hex pinning arithmetic a unit test pins for free. See
  [Bluetooth LE framing](ble-framing.md#conformance-vectors).

## Self-testing a second implementation

An implementation that has not run these against itself has not been tested,
only reviewed.

1. Load the vector file for the chapter you implemented.
2. For each `frames` or `envelopes` case, encode the described value and compare
   bytes.
3. For each case, decode the bytes and compare against the described value.
   Encoding alone passes for a codec whose decoder agrees with its own encoder
   and with nothing else, which is the precise way a positional format fails.
4. For each `decode_only` case, decode and check the stated expectation. These
   are the values a *future* sender emits, so no conforming encoder of this
   version can produce them and they can only be pinned from the wire inward.
5. For each `rejects` or `reject` case, confirm the input is refused.
6. Check the case counts before iterating. A loop over an array that a bad merge
   emptied passes by not running, which is the one failure mode a vector suite
   cannot otherwise detect.

## Chapters that are not yet conformance surfaces

Some chapters specify behaviour that has no vector file:

- [Group protocol](group-protocol.md) and [Username discovery and
  invites](username-discovery.md) are specified in prose and pinned by the
  reference implementation's own tests.
- [The gateway contract](gateway-contract.md) is pinned only where it is
  bytes. The attach proof has vectors, because it is the one place in that
  chapter where a wrong encoding is silent: a payload built with the length
  little-endian, or with the challenge length-prefixed as though it were a
  second field, produces a signature that verifies against nothing, which
  reads as a key problem rather than an encoding one. The rest of the chapter
  is a JSON message vocabulary whose mistakes surface as a rejected message,
  and it stays prose.
