# 0001. JSON is the permanent wire floor; compact encodings are additive

**Status:** Accepted
**Shipped in:** 0.14.0 (binary wire codec), 0.14.0 (compact MLS envelope)

## Context

JSON message framing is expensive on a Bluetooth LE link. A 36-character
hyphenated message identifier, verbose field names, and a ciphertext rendered as
a decimal integer array cost roughly 3 to 4 times what a compact encoding does,
which translates directly into fragment counts and airtime.

The obvious move is to replace JSON with a compact encoding. Two things make
that wrong here:

1. The fleet upgrades gradually and mesh peers meet arbitrary strangers. There
   is no coordinated flag day.
2. The internet relay and every persisted record are on the same code path.
   Changing the encoding changes what old records deserialize as.

## Decision

Compact encodings are **additive**. JSON remains a permanent obligation:

- every receiver decodes JSON, unconditionally and forever,
- a compact encoding is emitted only to a peer that advertised it,
- persistence and the internet relay transport stay JSON unconditionally (the
  Nostr transport is relay-mediated but uses the negotiated codec like any other
  peer-to-peer path),
- decoding of compact encodings is always on, independent of whether emitting
  them is.

Three layers carry this full shape, each with its own switch and its own
capability, because they are independent: the hop-local wire codec
(`binary_wire_enabled` / `wire_versions`), the end-to-end MLS envelope
(`compact_envelope_enabled` / `env_versions`), and the sealed rich payload
(`rich_payload_enabled` / `rich_versions`).

The media chunk envelope is **not** a fourth instance and should not be
described as one. It has no JSON form to fall back to and no switch: its payload
is always the compact encoding, and only the choice between its v1 and v2 forms
is negotiated, riding the rich-payload capability rather than one of its own.

## Consequences

**Good.** A mixed fleet works with no coordination. Rollback is a configuration
change, not a migration. A peer that mis-advertises costs a delivery failure to
itself, not a fleet-wide outage.

**Cost.** Two encoders and two decoders per layer, forever. The JSON path can
never be deleted, so it must stay tested. Size wins only materialize once both
ends have upgraded.

**Cost.** Detection is by first byte, which constrains the magic byte to a range
that cannot begin valid JSON or valid UTF-8. That range holds eleven values, of
which v1 spends one, leaving ten for future versions. That is plenty, but it is
finite.

## What would undo this

Making decode of a compact form conditional on local configuration. That turns a
kill switch into a compatibility break: peers that were told we are capable
start sending a form we then refuse.

Adding a compact form to persistence "since we already have the codec". Stored
records outlive every negotiation.
