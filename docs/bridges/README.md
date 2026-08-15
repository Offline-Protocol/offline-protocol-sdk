# Bridge contracts

The Rust core is exposed to Swift, Kotlin, Python, and TypeScript. This
directory states what each binding owes the core and what the core owes each
binding.

| Document | Scope |
|----------|-------|
| This file | The contract every binding shares |
| [Swift](swift.md) | iOS native and the React Native iOS bridge |
| [Kotlin](kotlin.md) | Android native and the React Native Android bridge |
| [Python](python.md) | Desktop and tooling |
| [TypeScript](typescript.md) | The React Native JavaScript surface |

Integration **guides** live elsewhere: [iOS](../ios-integration.md),
[Android](../android-integration.md),
[React Native](../react-native-integration.md). This directory is the contract,
not the tutorial. It is what you read before changing the boundary, not before
using it.

## Why this needs writing down

Every rule here shares one property: **violating it fails silently.**

Nothing in the toolchain catches a partial binding regeneration, a config field
dropped in a bridge parser, an error variant inserted rather than appended, or a
constant list that disagrees across three languages. A green `cargo clippy` and a
green `tsc` say nothing about any of them.

## C1. Regenerate every binding together

The Swift, Kotlin, and Python bindings are **one artifact set**, not three
independent ones. They come from one bindgen run over one interface definition
and carry the FFI checksums of the library they were generated against.

```bash
./scripts/generate-bindings.sh
```

Regenerating a subset leaves the others describing a different ABI. **That fails
no build.** It fails the app, at the first call, with a checksum mismatch.

Every path that generates bindings delegates to that one script: the React
Native wrapper, both platform build scripts, the Python desktop build, and both
CI workflows.

The release workflow matters most and is the least obvious: the Kotlin it
generates is downloaded **over** the committed Android bindings in the publish
job, so it, not the committed file, is what ships. Anything that generates there
must come through the same script, or the released Kotlin and the released Swift
were built by different bindgens.

## C2. The error enum is append-only

Generated bindings decode the error enum by **positional discriminant**.
Inserting or removing a variant, or reordering, breaks every committed binding
in every language, silently, by shifting every variant after the edit.

New variants are appended. Never inserted, never removed, never reordered.

The core maps its internal errors to this enum by classifying every variant
explicitly. That match is **no longer compiler-enforced**: the engine error
types are `#[non_exhaustive]`, which forces a wildcard arm, so a newly added
internal variant compiles cleanly and reaches the boundary as `Other`, logging a
warning at runtime.

Treat the wildcard as a tripwire, not a destination. After adding an internal
error variant, classify it here as well, and watch for that warning: the compile
error that used to catch the omission is gone.

## C3. Events cross as opaque JSON

Events are serialized to JSON in the core and cross the boundary as a single
string. Nothing about an event's field set appears in the interface definition.

Consequences, in both directions:

**Adding or changing an event field needs no interface regeneration.** This is
deliberate. Events change far more often than the API surface, and coupling them
would mean a full four-language regeneration for every field.

**The bindings' event types are therefore unchecked by the compiler.** A
TypeScript event interface that has drifted from what the core emits compiles
perfectly and fails at runtime. The core carries tests that pin the event tag
strings against the TypeScript definitions for exactly this reason, and those
tests are the only mechanism holding the two in step.

**A field added to an event is invisible to a binding until someone adds it
there.** There is no warning.

## C4. Security-relevant answers get dedicated entry points

Anything whose contents drive a delivery or security decision arrives through
its own function, never by injecting a synthesized message frame into the
generic receive path.

The group delivery report is the reference case. Bridges may still pass the raw
frame through as an opaque server message for observability, but that path
drives nothing.

See [ADR 0014](../adr/0014-dedicated-ffi-entry-points.md).

## C5. Hand-mirrored constants must be pinned in every language

Some constants exist in several places no single compiler sees together. Two
sets do today, and they are pinned by **different** mechanisms, so knowing which
one you are touching matters.

**The relay-answer prefix exemption list** is the canonical example: the core,
the Swift bridge, and the Kotlin bridge each hold a copy. A prefix present in one
copy and absent from another **fails silently**: the bridge injects the answer
unattributed, the core's gate declines to exempt it, and the frame is dropped as
unsigned with no peer at fault. The visible symptom is a relay feature quietly
not working.

Each copy of that list is pinned against **literals** in its own language's test
suite. A test that recomputes the list from the constant it is checking agrees
with any edit, which is precisely the failure mode.

**The protocol-state record ceiling** is the second set, and it is wider: four
sites across three binding languages, Python included
(`ProtocolStateStorage.swift`, `ProtocolStateStorage.kt`, `state_storage.py`,
and the Rust constant they mirror). It is pinned the other way round, by a
single **Rust** guard that reads all three binding sources and asserts the
literal `8 * 1024 * 1024` in each. There is no per-language test for it, so a
binding edited alone fails the Rust suite rather than its own. See
[S6](swift.md#s6-secure-storage).

The one-shot event tag list and the mesh wake task key are pinned the same way,
by Rust guards that read the binding sources.

## C6. Config parsers must not default to literals

A bridge parsing a config object must distinguish "the caller did not supply this
field" from "the caller supplied the default value".

A parser that reads each field with a literal fallback turns every **partial**
config update into a silent reset of every field the caller did not mention.
This has shipped as a bug more than once.

Where a binding accepts both a nested section and flat keys for the same setting,
**nested wins over flat**, and both spellings must be pinned in the bridge's own
parser tests.

## C7. Ordering constraints across the boundary

Some sequences are constrained and the constraint is invisible on either side
alone.

| Constraint | Why |
|------------|-----|
| Relay capabilities injected **before** the internet-available transition | The flush that transition triggers must already see them |
| Relay capabilities cleared **on** internet drop | Otherwise a stale capability keeps the broadcast gate open |
| Per-peer end-to-end capabilities restored **before** queued sends flush | Otherwise the startup flush emits downgraded envelopes to every established peer |

## C8. The identifier the bridge reports must match the namespace it is asked for

A bridge holds identity in more than one namespace: the protocol address, and
whatever the relay or directory keys by.

Handing back the wrong one is a silent failure, because both are non-empty
strings that look plausible. Every bridge function that returns or compares an
identity must state which namespace it is in.

## C9. Bridge behaviour is not covered by the Rust test suite

`cargo test` proves nothing about the bridges. Each binding needs its own tests,
and they are fast enough that there is no excuse:

| Binding | Test entry point | Rough cost |
|---------|-----------------|------------|
| Swift | `swift test` in the React Native iOS package | seconds |
| Kotlin | Gradle unit tests, Robolectric where a platform type is needed | tens of seconds |
| Python | pytest against the built desktop library | seconds |
| TypeScript | `tsc` plus the JS harness | seconds |

**After a rename, grep for the old identifier across every binding.** The
compiler will not find it in the three languages that do not share a type system
with the one you changed.

## C10. Lifecycle rules for event emission

An event delivered to a binding that has no live host instance is **lost**. The
core does not retain it, and the core cannot know the host is gone.

Two shapes solve this and they are not interchangeable:

- **Restatement** for state that has a current value: derive the current state
  from a latch and re-emit it on subscribe. Correct for presence, connection
  status, transport state.
- **A held one-shot** for events that fire once and matter: hold the event until
  a subscriber acknowledges it, then clear. Correct for one-time results.

Which one applies depends on **where the event fires** relative to subscription,
not on what the event means. A replay-on-subscribe mechanism alone does not fix
a one-shot that fires before any subscriber exists.

## What each binding owes

| Binding | Owes |
|---------|------|
| Swift | The manual Objective-C bridge kept in step with every `@objc` method; secure storage backed by Keychain; a live-instance check before emitting |
| Kotlin | Secure storage backed by Keystore; no blocking work on the main looper; awareness that platform callbacks arrive on binder threads |
| Python | Nothing platform-specific; it is the thinnest binding and therefore the best place to smoke-test an ABI change |
| TypeScript | Config normalization, event typing kept in step with the core, and no assumption that a native method exists in an older binary |
