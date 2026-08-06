# JS CI harness

Behavioral tests for the SDK's **TypeScript layer** (`../src`), run against the
real compiled class with a stubbed native module. No React Native runtime, no
device, no test framework.

## Why this exists

The RN package has no other JS test setup, and the TypeScript layer is the one
part of this bridge that nothing else covers:

- `tsc` (the `React Native Typecheck` CI job) proves it compiles, not that it
  behaves.
- The Rust text-guards in `crates/offline-protocol-uniffi/src/lib.rs`
  (`react_native_*`) pin that a mechanism's pieces are still *present* and that
  event tags agree across the TypeScript, Kotlin and Swift definitions. They
  read source as text — by construction they cannot tell you the pieces still
  add up to correct behavior.
- The Android (`../android-ci-harness`) and iOS (`../ios/Package.swift`)
  harnesses cover the native halves only.

That gap is not academic. Every failure mode in the one-shot event hold —
an event held but never replayed, replayed twice, replayed stale, replayed
before the `on(...)` that registered it returns — compiles, typechecks, and
ships silently.

## Run it

```bash
cd bindings/react-native
npm ci          # first time only
npm run test:js
```

Or directly: `node js-ci-harness/one-shot-hold.test.js`. The same command runs
in the `React Native Typecheck` job in `.github/workflows/ci.yml`.

Each file compiles `../src` into a scratch directory of its own rather than
using the package's `lib/` — that directory is gitignored and routinely stale,
and a harness that silently tests last month's build is worse than none.

## What is stubbed

`require('react-native')` is intercepted (via `Module._load`) and answered with:

- `NativeModules.OfflineProtocolModule` — a Proxy returning `Promise.resolve()`
  for every method, overridable per test.
- `NativeEventEmitter` — a stub that records subscriptions, can emit events on
  demand, and **flushes "sticky" events from inside `addListener`**. That last
  detail is the point: it reproduces Android's redelivery timing, which fires
  from the native `addListener` that the SDK's own constructor calls — the one
  moment where the app provably has no listener registered yet.

## Writing tests here

Two traps, both of which produce a test that passes while proving nothing:

- **Never assert inside an event listener.** `emitEvent` wraps every listener
  call in try/catch, so a failing assertion in a handler is swallowed and
  logged. Record what the handler saw and assert after it returns.
- **Always mutation-test a new case.** Patch out the mechanism it claims to
  cover, confirm it fails, restore. Do this with the source *staged*, so the
  restore (`git checkout -- <file>`) cannot take an uncommitted fix with it.

## Files

| File | Covers |
| --- | --- |
| `one-shot-hold.test.js` | The JS-layer one-shot event hold: hold, replay, and the `start()` / `enableTransport('internet')` / `destroy()` staleness transitions (`src/index.ts`, `ONE_SHOT_EVENT_TYPES`). See `docs/react-native-integration.md` §6.1. |
