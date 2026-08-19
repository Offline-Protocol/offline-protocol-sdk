# Architecture decision records

Short records of decisions that are expensive to reverse or easy to undo by
accident. Each states the forces, the decision, and what it costs.

An ADR is written when a decision is **non-obvious**: when a reasonable engineer
would pick differently without knowing the context, or when a later change could
silently undo it. Decisions that follow from the obvious default do not need one.

## Index

| # | Decision | Status |
|---|----------|--------|
| [0001](0001-json-as-permanent-wire-floor.md) | JSON is the permanent wire floor; compact encodings are additive | Accepted |
| [0002](0002-frozen-dto-with-extension-tlv.md) | The binary encoding uses a frozen positional DTO with an extension TLV | Accepted |
| [0003](0003-self-certifying-addresses.md) | Identity is a self-certifying address, not a trust-on-first-use pin | Accepted |
| [0004](0004-control-plane-signature-gate.md) | Control frames are signature-gated with two documented exemption classes | Accepted |
| [0005](0005-defer-instead-of-drop-and-ack.md) | A receiver that cannot deliver withholds the acknowledgement | Accepted |
| [0006](0006-desync-classification-gates-rekey.md) | The desync classification gates the re-key, not the acknowledgement | Accepted |
| [0007](0007-reseal-on-resend.md) | Resends are re-sealed against the current session, never replayed | Accepted |
| [0008](0008-sealed-rich-payload.md) | Rich extras travel inside the MLS plaintext or not at all | Accepted |
| [0009](0009-report-membership-changes-by-default.md) | Unauthorized membership changes are reported; rejection is opt-in | Accepted |
| [0010](0010-unconditional-leaf-identity-binding.md) | Leaf identity binding is unconditional and checked at three seams | Accepted |
| [0011](0011-relay-broadcast-gated-on-delivery-report.md) | Relay broadcast defaults on, gated on a capability that guarantees a settled report | Accepted |
| [0012](0012-one-key-package-per-peer.md) | The push path assigns one MLS init key per peer | Accepted |
| [0013](0013-exhaustive-privacy-classifier.md) | Remote-influenced text is classified to a fixed vocabulary by an exhaustive match | Accepted |
| [0014](0014-dedicated-ffi-entry-points.md) | Security-relevant relay answers arrive through dedicated entry points | Accepted |
| [0015](0015-relay-hint-frames-unacked-and-pinned.md) | Relay hint frames are unacknowledged and transport-pinned | Accepted |
| [0016](0016-gateways-are-provisioned-not-emergent.md) | Gateways are provisioned, never emergent | Accepted |
| [0017](0017-nostr-is-a-carrier-not-a-gateway.md) | Nostr is a carrier, never a gateway | Accepted |
| [0018](0018-data-layer-engine-and-storage-seams.md) | The data layer has two seams: the engine and the backend | Accepted |

## Format

Keep them short. Context, decision, consequences, and where relevant an
explicit "what would undo this" note, because that is the part a future change
needs to see.

Status values: Proposed, Accepted, Superseded by NNNN, Deprecated.
