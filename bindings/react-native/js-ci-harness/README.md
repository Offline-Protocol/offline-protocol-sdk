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

`test:js` runs every file below in turn; each is also runnable directly, e.g.
`node js-ci-harness/one-shot-hold.test.js`. The same `npm run test:js` runs in
the `React Native Typecheck` job in `.github/workflows/ci.yml`.

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
| `local-address.test.js` | The cache of this device's derived address (`src/index.ts`, `cachedLocalAddress`): populated eagerly by `start()` and by `identity_ready`, cleared by `destroy()` so it cannot outlive the identity it names, and the session attribution that depends on knowing which half of a pair is us. |
| `relay-config.test.js` | The relay and DORS config payloads this layer hands to native: the whole `relay` section crossing at create time (not just `relayPriority`), the legacy `low`/`medium`/`high` spelling mapping to the engine vocabulary, and a runtime update naming only the fields it was given — which is what makes the native-side merge a partial update rather than a full overwrite. Its other half is the Rust guard `react_native_bridges_merge_dors_updates_from_the_live_config`. |
