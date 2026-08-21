# 0022. One sealed layer, shared with the leaf

**Status:** Accepted

## Context

[ADR 0021](0021-a-leaf-node-speaks-mls.md) decided that a leaf node speaks
MLS, through a second implementation sized for the part. It also said what
makes that affordable: the protocol does not change, and the phone does not
learn a second way to seal anything. The two ends run different MLS libraries
and agree on everything outside them.

"Everything outside them" is four things, and all four were in crates a leaf
node cannot link:

| Piece | Where it was | Why a leaf needs it |
|---|---|---|
| `EncryptedMessage` and its compact codec | `offline-protocol-mls` | It is the wire form of every sealed payload, in both directions |
| `derive_address` | `offline-protocol-mls` | Every trust gate is `derive(presented_key) == claimed_address`, including the leaf's own credential |
| The canonical signing payload | `offline-protocol` | A leaf signs and verifies control frames |
| The sender-ratchet bounds | `offline-protocol-mls` | Both ends of a session must configure the same two numbers |

`offline-protocol-mls` is built on OpenMLS and `offline-protocol` is the
engine; neither compiles without `std`. So a leaf implementation had exactly
two options: copy them, or move them somewhere both sides can reach.

The cost of copying is not hypothetical. `tools/mls-interop` already carried
two of these copies, because it needed them before this layer existed, and its
manifest said so in a comment: nothing pinned the copies to the originals, so a
change in the SDK would leave the harness green while it quietly stopped
testing the SDK's configuration.

## Decision

**These four pieces live in one crate, `offline-protocol-sealed`, which sits
between `offline-protocol-core` and everything else and compiles for bare
metal.** Every existing route to them is a re-export or a delegation, so no
call site, wire byte, error string or FFI signature changes.

The rule is *move, never copy*. `MlsManager::derive_address` is now a
one-line delegation; `offline_protocol_mls::types` re-exports the envelope
types; `group.rs` re-exports the ratchet constants; the engine's
`build_canonical_payload` calls
`offline_protocol_sealed::control_signing_payload`; and `tools/mls-interop`
imports all of it instead of restating it.

### Why a new crate rather than putting them in core

Core is deliberately empty of two things this needs.

**Core links no cryptography.** Address derivation is a SHA-256, and putting
`sha2` in core would push a hash implementation into every consumer of the
protocol's base types, including a relay-only leaf image that parses and
forwards frames and never derives an address at all. `address.rs` says this in
its module docs already: it never touches key material, and hashing happens
upstream.

**Core knows nothing about MLS.** `EncryptedMessage`, `GroupId` and
`MlsMessageType` are MLS vocabulary. Core's published API is the message
envelope, addressing and the wire codec, which is what makes the "about 95 KB
for the protocol layer" figure in ADR 0020 mean something specific.

A crate boundary also buys the thing prose cannot: the bare-metal CI job now
builds and lints `offline-protocol-sealed` without `std`, so a `use std::`
added here fails a check rather than a device.

### What the split does not change

The engine still has one sealing path, one prefix registry and one threat
model. This ADR moves code between crates; ADR 0021 is still the decision
about what a leaf runs, and this is the plumbing that lets it run it.

## Consequences

There is now exactly one implementation of each of these in the workspace, and
the interop harness tests the SDK's configuration rather than its own copy of
it. `the_interop_harness_uses_this_crate_rather_than_its_own_copies` in
`offline-protocol-sealed` reads the harness source and fails if the copies come
back, which is the only thing in `cargo test --workspace` that can notice: the
harness is its own cargo workspace, so no type check connects the two.

A ninth publishable crate joins the release, with the obligations every other
one carries: a `LICENSE` copy, a README, and local dependency declarations kept
in lockstep with the workspace table by a guard test.

Methods that moved now return `SealedError` where they returned `MlsError`.
`From<SealedError>` exists for both `MlsError` and the engine's `Error`, and
passes the inner string through rather than the rendered `Display`, so error
text is byte-identical. The change is visible only to an external consumer
matching on the error type of `EncryptedMessage::from_bytes` or
`GroupId::new`, and the workspace moves every crate's version together.

`SealedError` is deliberately not `#[non_exhaustive]`, unlike its sibling
`MlsError`. The mapping into `MlsError` has to be exhaustive, because a
wildcard arm is how a new variant silently starts rendering as some other
error's text (the failure [ADR 0013](0013-exhaustive-privacy-classifier.md)
names for classifiers). Adding a variant here is a compile error in the MLS
crate until someone decides what it means.

### What joined the layer afterwards

The four pieces above are what the decision was taken over. The rule it set
(anything both ends must agree on lives here, once) admits more, and two more
arrived when the leaf crate needed them:

| Piece | Came from | Why both ends need it |
|---|---|---|
| The six 1:1 control-frame prefixes | `offline-protocol`, a private module | A frame's type is the prefix its content begins with, so two ends that disagree about one do not have a conversation |
| `KeyPackagePayload` and the compact envelope version | `offline-protocol`, a private module | It is the only channel by which capabilities are advertised, and a leaf builds it with the other MLS implementation |

Neither changes what this ADR decided, and both were pure relocations: the
prefix registry that reserves a prefix and refuses application content
beginning with one stays in the engine, because reservation is engine
machinery rather than something the two ends agree on.

## What would undo this

Reintroducing a second implementation of any of the four. The likely route is
not a deliberate fork but a convenience: a harness, a benchmark, a fixture or
an example that needs a derivation or a codec and writes three lines rather
than taking the dependency. That is exactly how the interop harness acquired
its copies, and the harness is now the worked example of what closing it looks
like.

The other route is moving these into core after all, to be tidy. The reason not
to is in the decision above: it puts a hash into an image that does not derive
addresses, and MLS vocabulary into a crate whose whole claim is that it is the
protocol layer and nothing else.
