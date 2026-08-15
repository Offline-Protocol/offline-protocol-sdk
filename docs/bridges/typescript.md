# TypeScript bridge contract

Covers the React Native JavaScript surface.

Read [the shared contract](README.md) first. This document covers what is
specific to TypeScript.

## T1. TypeScript is not generated, and the compiler proves less than it appears to

UniFFI does not produce TypeScript. The JavaScript surface is hand-written over
the two native modules.

`tsc` passing means the TypeScript is internally consistent. It says nothing
about whether:

- the native method being called exists,
- the event shape being destructured is what the core emits,
- a config key being passed is one the bridge parser reads.

All three are runtime failures that compile cleanly.

## T2. Event types are pinned from the Rust side

Events cross as opaque JSON ([C3](README.md#c3-events-cross-as-opaque-json)), so
the TypeScript event interfaces have no compile-time link to the core.

The core holds guard tests that read `bindings/react-native/src/types.ts` and
pin the event JSON shape against the declarations there. That test, not the
TypeScript compiler, is what keeps them in step.

Adding an event field means updating `types.ts` in the same change. When
`types.ts` lags, nothing fails loudly: the event simply arrives untyped.

## T3. Cross-language constant sets are pinned by Rust guards

Two constant sets in `constants.ts` are mirrored by hand into other languages,
up to four definitions across up to three languages, and drift fails silently
while everything still compiles:

| Constant | Definitions | Also defined in | Guard |
|----------|-------------|-----------------|-------|
| `ONE_SHOT_EVENT_TYPES` | 4, across 3 languages | Kotlin module event constant, Kotlin and Swift superseded-latch policy | `react_native_one_shot_event_set_matches_native` |
| `MESH_WAKE_TASK_KEY` | 2, across 2 languages | Kotlin mesh wake policy | `react_native_mesh_wake_wiring_is_present` |

The wake task key is the worse of the two to break: React Native logs "No task
registered for key" to the device log, the app sees an opt-in that does nothing,
and both sides compile.

## T4. One-shot membership is a decision, not a filter

An event belongs in the one-shot set only when **redelivering it late is better
than losing it**.

A held periodic event replayed after the fact reports a state that has since
changed, which is worse than the drop it replaced. That is why the set is two
entries and not twenty.

Both current members are events emitted after the thing that would restate them
is already down, so there is no later event carrying the same news.

## T5. The two gaps are covered by different mechanisms

| Gap | Mechanism | Lives in |
|-----|-----------|----------|
| native to JavaScript | Sticky buffer | Kotlin |
| JavaScript to app listener | Held one-shot | TypeScript |

They are not interchangeable and neither covers the other's gap. The JS-side
hold must be cleared **behind a microtask yield**, or a listener registered
synchronously in the same tick misses it.

**Never assert inside a listener** in a test for this. An assertion that throws
inside a listener is swallowed by the emitter and the test passes.

## T6. Config normalization

The JavaScript surface accepts both a nested config section and flat keys.
**Nested wins.** It forwards both spellings down to the native parsers, which
apply the same precedence (see [C6](README.md#c6-config-parsers-must-not-default-to-literals)).

A config section added on the JS side that no native parser reads is silently
inert. Add the parser case and its test in the same change.

## T7. Over-the-air JavaScript updates can outrun the native binary

A JavaScript-only update that starts calling a **new** native method against an
older native binary fails those calls with a method-not-found error. Existing
methods are unaffected.

Any new native method reachable from JavaScript is therefore a native-version
dependency, and a JS-only deployment channel needs to guard it.

## T8. The published package's build output is not the local one

The `lib/` directory is gitignored and goes stale. Never read it to determine
what shipped. Check the published tarball, or do a clean rebuild.

A gitignore rule for a bare `lib/` matches at any depth, which has already eaten
a `scripts/lib` directory once. Note that the rule here is **still** unanchored:
that collision was resolved by renaming the victim to `scripts/shared`, not by
fixing the pattern. Anchor it as `/lib/` before adding any nested `lib/`
directory, or expect the same silent disappearance.

## Testing

```bash
cd bindings/react-native
npx tsc --noEmit
```

Plus the JavaScript harness under `js-ci-harness/`.

The example app under `examples/react-native-app/` is **not** typechecked by any
CI job. Changes there are unverified unless you check them by hand.
