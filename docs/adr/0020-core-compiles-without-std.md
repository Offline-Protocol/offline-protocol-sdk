# 0020. Core compiles without `std`, and `std` is a default-on feature

**Status:** Accepted

## Context

The SDK's smallest deployable unit was a phone. Every crate assumed `std`, an
async runtime and an operating system, which is correct for the engine and
wrong for the class of device the protocol most wants to reach: a battery door
lock, a sensor, a mains-powered bulb acting as a relay. Those are Cortex-M
parts with roughly 1.5 MB of flash and 256 KB of RAM, no operating system, and
no Rust `std` at all, because bare-metal targets ship `core` and `alloc` only.

The published mobile artifact is 4.45 MB, which reads as "does not fit" next to
those numbers and ends the conversation. That figure is a `cdylib` containing
the engine, every transport, MLS and the CRDT data layer. None of it is what a
leaf node runs.

A leaf node does not mint messages. It receives a frame someone else minted,
validates it, and answers. That half of the protocol is `offline-protocol-core`
and nothing else, and core turned out to depend on `std` in only three places:
constructors that mint state from the platform, one module of lock helpers, and
two `HashMap` field types.

## Decision

`offline-protocol-core` compiles for bare-metal targets with
`--no-default-features`. `std` is a feature, on by default, so the published API
and every existing consumer are unchanged.

`std` gates exactly what the platform must supply and a bare-metal target does
not: a wall clock (`Timestamp::now`, `WallClockTimestamp::now`), a monotonic
clock (`LocalInstant`), entropy (`MessageId::new`, and `Message::new` and
`MessageBuilder` with it), and threads (the `sync` module). Everything that
parses, validates, re-encodes or compares is present in both configurations.

No `alloc` feature exists. Every type here is built from `String` and `Vec`, so
`alloc` is mandatory in both configurations, and a feature that can never be
turned off is dead configuration.

Three consequences that will look like clutter to someone tidying up later:

**Seven dependencies are declared locally instead of inherited.** A member
crate cannot drop default features from a `{ workspace = true }` dependency.
Cargo accepts `default-features = false` beside it and silently ignores it, so
an inherited `serde` keeps pulling `serde/std` and the bare-metal build fails
with errors pointing at the dependency rather than at the cause. Re-inheriting
them re-breaks the build in a way that does not look related to the change that
caused it. A guard test fails if the local versions drift from the workspace
table, because dropping inheritance also drops the single point of update.

**`uuid/v4` lives in the `std` feature, not the base dependency.** v4 pulls
`getrandom`, which has no backend on bare metal.

**`MetadataMap` replaces `HashMap<String, String>` in two field types.** Under
`std` it is `HashMap`, so nothing downstream moves; without it, `BTreeMap`,
because `HashMap`'s default hasher seeds from entropy that is not there. This
cannot reach the wire in either direction: JSON objects are unordered by
definition, and the binary v1 codec carries metadata as an ordered
`Vec<(String, String)>`.

Test modules are gated on `std`. The no_std configuration is verified by
building it, not by running tests, which is the only option on a target with no
test runner. A CI job builds and lints it, plus builds core standalone with
default features, because a missing `std` re-add is invisible inside the
workspace where another member's features paper over it through feature
unification, and would otherwise surface only during `cargo publish`.

## Consequences

A leaf node links about 95 KB of flash and no static RAM beyond its own heap,
measured by `tools/embedded-footprint` and reported on every CI run. That is
roughly 6% of an xG24's flash, which makes the question "does it fit" answerable
with a number rather than an argument.

This ADR covers the protocol layer only. It does not make MLS run on a leaf
node, and it should not be read as promising that: OpenMLS is not a no_std
crate, and MLS key-schedule and ratchet-tree state does not fit alongside a
vendor radio stack in 256 KB.

A leaf node's payload cryptography was settled separately, in
[ADR 0021](0021-a-leaf-node-speaks-mls.md). The paragraph above holds for
OpenMLS and for large groups, and turned out not to hold for the shape the
problem has: a phone paired with one device is a two-member group whose ratchet
tree is three nodes, and a second RFC 9420 implementation that does build
without `std` fits the part. A leaf runs real MLS.

## What would undo this

Adding a `use std::` to any file in core that is not behind
`#[cfg(feature = "std")]`. The CI job is what turns that from a discovery on
someone's hardware into a failed check, and it is the only thing that does.
