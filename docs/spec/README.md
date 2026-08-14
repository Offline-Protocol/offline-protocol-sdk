# Offline Protocol Specification

This directory specifies the Offline Protocol as a wire and behaviour contract,
independent of the Rust implementation in this repository. A second
implementation written against these documents, in any language, should
interoperate with this one.

The Rust crates are the reference implementation, not the definition. Where a
document here and the code disagree, that is a bug in one of them, and the
document says which reading is normative for the wire.

## Documents

| Document | Scope |
|----------|-------|
| [Identity and addressing](identity.md) | Address derivation, canonical form, self-certification, session and group identifiers |
| [Message model and wire format](wire-format.md) | The abstract message, the JSON encoding floor, the binary v1 encoding, the extension TLV registry |
| [Control messages](control-messages.md) | The reserved prefix registry, control-plane signing, and the two exemption classes |
| [Encryption envelopes](encryption-envelopes.md) | The `__MLS_ENC__` envelope forms, the media chunk envelope, and the sealed rich payload |
| [Group protocol](group-protocol.md) | Group frames, membership commits, leaf identity binding, relay broadcast and the delivery report |
| [Capability negotiation](capability-negotiation.md) | What peers advertise, what each capability gates, and what happens on absence |

## Conformance language

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as defined in
RFC 2119 and RFC 8174, and appear in capitals only when used in that sense.

## Layering

The protocol has four independent layers. Each has its own versioning and its
own failure mode, and a reader should keep them apart:

1. **Transport framing.** How bytes reach the next hop. Bluetooth LE, Wi-Fi
   Direct, Reticulum, Nostr, or an internet relay. Out of scope here except
   where a transport constrains a payload size.
2. **Hop-local encoding.** How a `Message` becomes bytes for one hop. Either
   the JSON floor or the binary v1 codec. Negotiated per peer via
   `wire_versions`, and re-negotiated on every connection.
3. **End-to-end envelope.** How ciphertext and its addressing survive an
   arbitrary number of relay hops. The `__MLS_ENC__` and media envelopes,
   negotiated via `env_versions`.
4. **Sealed payload.** What travels inside the MLS AEAD boundary. Plain text,
   or a `__RICH_V1__` body, negotiated via `rich_versions`.

A change at one layer does not imply a change at another. The compact MLS
envelope and the binary wire codec ship independently and are gated by separate
switches, because a message can take the binary encoding on one hop and JSON on
the next while its envelope stays byte-identical end to end.

## Two invariants that outrank everything else

**JSON is the permanent floor.** Every conforming receiver MUST accept the JSON
encoding of a `Message`. A sender MAY emit a compact encoding only to a peer
that advertised it. Persistence and the internet relay path use JSON
unconditionally. No negotiation, capability, or configuration removes the
obligation to decode JSON.

**Frozen encodings never change in place.** The binary v1 DTO field order and
the numeric enum mappings are a wire contract. Additive data goes in the
extension TLV section. A change that cannot be expressed additively takes a new
version identifier and is negotiated, never assumed.
