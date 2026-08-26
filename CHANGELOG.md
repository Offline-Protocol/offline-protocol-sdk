# Changelog

All notable changes to the Offline Protocol SDK are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). This changelog covers
everything after the **v0.7.1** release.

This file holds unreleased changes and the current release. Older releases are
archived by series under [docs/changelog/](docs/changelog/); see the
[archive index](docs/changelog/README.md).

## [0.24.1] — 2026-08-26

> **Every fix here is the same defect found four ways: the iOS React Native
> bridge is declared twice, in two languages, and nothing compiles both
> halves.** `RCT_EXTERN_METHOD` records a selector and a parameter type as
> text; the Swift method it names is compiled separately, and
> `OfflineProtocolModule.swift` is the one bridge source no CI job builds. A
> selector that does not resolve is dropped at module load, a parameter type
> that disagrees is called through a function pointer cast to the wrong
> convention, and neither is reported. Eight methods were absent from
> JavaScript entirely, seven resolved but ran on argument bits that were never
> the number passed, fifteen conversions aborted the process instead of
> rejecting the call, and one promise never settled. **Android was never
> affected**, at any point, in any of the four: its dispatch is by method name
> and position, and the Kotlin side was correct throughout.
>
> **Settings your application already passes start taking effect on iOS for
> the first time, so read this before upgrading a fleet.** `updateRelayConfig`
> and `getRelayConfig` were never declared in the bridge, so since 0.22.0
> every relay setting given to `create()` was discarded behind a `console.warn`
> and the engine kept its own defaults, which are `allowRelay: true` and a 30%
> battery floor. The setting being ignored was therefore the one that turns
> something off: **an iOS device told not to relay has had its forwarding gate
> open the whole time, carrying other people's traffic whenever the mesh handed
> it any, and closes that gate on this release.** A stricter battery floor
> starts being honoured and a looser one starts permitting what it asked for,
> and the floor could not bite at all before this, because `setBatteryLevel`
> clamped its garbled argument to 100 on every call.
> `wipePersistedState` had been uncallable since 0.21.0, which means logging
> out could not erase the account it had just signed out of, and every prior
> account's MLS identity and sealed state is still on disk on such a device.
> `sendMessage`, `sendMessageRich` and `sendPresenceUpdate` stop pinning every
> priority and status to their `default:` arm, and both defaults are the
> innocuous value, which is why this went unnoticed: every iOS message went out
> `medium`, and **every presence update went out `online`, including the ones
> an application sent to say `away` or `offline`.** Nothing changed shape, so
> none of this breaks a build, which is exactly why it needs reading:
> [docs/UPGRADING.md §18](./docs/UPGRADING.md#18-settings-you-already-pass-start-applying-on-ios-v0241).
>
> **A remote peer could abort an iOS application, and that is why this is
> worth taking promptly.** Twelve conversions turned a `[NSNumber]` argument
> into bytes through a narrowing initializer that traps rather than returning
> a value the bridge could reject, and the array arguments have agreed across
> the bridge since the UniFFI migration, so these were live in every release
> that shipped the method. Any element outside `0...255` crashed the process,
> reachable from a malformed BLE fragment, a Wi-Fi Direct or internet frame,
> an MLS ciphertext or Welcome, a key package, or a file chunk. The remaining
> three aborted `create()` on an `initialTtl` above 255, and `create()` or
> `updateDorsConfig` on a negative DORS `historyWindowSize`.
>
> **The bridge now proves these cannot recur, by reading its own sources.**
> Three Rust guards parse the Objective-C shim, the Swift, and the TypeScript,
> and fail on any selector one side has and another does not, any parameter
> whose ABI class differs behind a shared selector, and any byte conversion
> that does not carry its own bound. The type table in `BRIDGE_MAINTENANCE.md`
> that recommended the mismatched pairing, and had done since v0.3.3, is
> corrected.

### Fixed

- **Eight React Native methods were unreachable on iOS, and the bridge now
  proves it cannot happen again.** `RCT_EXTERN_METHOD` does not declare a Swift
  method, it records a selector that React Native resolves against the class at
  module load; one it cannot find is dropped with a log line and the JS method
  is simply absent. Neither compiler sees both halves, and
  `OfflineProtocolModule.swift` is the one bridge source no CI job compiles, so
  three separate drifts shipped. `wipePersistedState` kept the pre-rename
  `userId:` label and had been uncallable since 0.21.0, which meant logging out
  could not erase the account it had just signed out of and every prior
  account's MLS identity and sealed state stayed on disk. `setBatteryState`,
  `getIsCharging`, `updateRelayConfig` and `getRelayConfig` were written in
  Swift, Kotlin and TypeScript and never declared in the bridge at all, so
  since 0.22.0 every relay setting an application passed to `create()` was
  discarded on iOS behind a `console.warn`: **applications that configure
  `allowRelay`, `minBatteryForRelay` or `relayPriority` will see those settings
  take effect on iOS for the first time on this release.** `dataListSpaces`,
  `dataFlushAll` and `dataWipeAll` took a labelled first parameter, which Swift
  exports as `dataListSpacesWithResolver:` rather than `dataListSpaces:`, and
  stopped resolving in 0.23.0. Android was never affected: its dispatch is by
  method name and position, and the Kotlin side was correct throughout.
  `react_native_ios_objc_shim_and_swift_agree_on_every_selector` now reads both
  bridge halves and the TypeScript, and fails on any selector one side has and
  another does not.

- **Seven more iOS methods resolved but ran on the wrong argument bits.** The
  bridge declares each parameter's type as text, and React Native picks the
  `RCTConvert` converter from that text and the calling convention from the
  Swift parameter's runtime encoding, then calls the one through a function
  pointer cast to the other. `nonnull NSNumber *` against a Swift `Int`
  therefore hands the method an object pointer read as a 64-bit integer, which
  is the pointer bits of a tagged `NSNumber` and never the number. The type
  table in `BRIDGE_MAINTENANCE.md` had recommended exactly that pairing since
  v0.3.3, the release that also introduced the first of these methods, so
  `sendMessage`, `sendMessageRich` and `sendPresenceUpdate` silently pinned
  every priority and status to their
  `default:` arm, `setBatteryLevel` and `setBatteryState` recorded a clamp bound
  rather than the level, and `processFileChunk` and `blePeerDiscovered` reached
  a narrowing conversion that traps, aborting the application. Nothing was
  logged in any of the seven cases. The type table is corrected, the two
  conversions that now receive real values reject or clamp out-of-range input
  instead of trapping, and the selector guard gained a third direction that
  compares the ABI class of every parameter behind a shared selector.

- **Fifteen iOS conversions aborted the app instead of rejecting the call.**
  A narrowing conversion like `UInt8(_:)` traps on out-of-range input rather
  than returning a value the bridge could reject, and every number reaching
  these conversions came straight from JavaScript. Twelve of them turned a
  `[NSNumber]` argument into bytes, so any array element outside 0...255
  crashed the application: reachable from a malformed BLE fragment, a Wi-Fi
  Direct or internet frame, an MLS ciphertext or Welcome, a key package, or a
  file chunk. The thirteenth was the `initialTtl` config field, which made
  `create()` abort on iOS for an application passing a value above 255, where
  Android truncated the same value and started normally. The last two narrowed
  the DORS `historyWindowSize` to `Int` before clamping it, which is too late
  to help: a negative number from JavaScript arrives at `uint64Value` as
  `UInt64.max`, so the conversion traps before the surrounding clamp can run,
  and both `create()` and `updateDorsConfig` aborted on a negative value. Byte
  arrays now convert through a helper that throws into the rejection each call
  site already had, `initialTtl` and `historyWindowSize` are clamped in the
  domain they arrive in, and
  `react_native_ios_bridge_bounds_every_byte_it_builds_from_javascript` fails
  on any byte conversion in the bridge that does not carry its own bound.

  Unlike the ABI mismatches above, these were never masked by anything. Array
  arguments cross as `NSArray *` against `[NSNumber]`, which has agreed since
  the UniFFI migration, so every one of these has been reachable in every
  release that shipped the method, and the transport ones are reachable by a
  remote peer rather than only by the application's own code.

- **`forwardMessage` hung forever on iOS debug builds instead of forwarding**
  ([#417](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/417)).
  React Native forces every `NSNumber` argument to non-null, because numbers
  are not nullable on Android, and refuses a null one before the Swift method
  is entered, so neither the resolver nor the rejecter ran and the promise
  never settled. The TypeScript passed `null` whenever a caller omitted the
  priority, which was the only nullable number in the bridge. No spelling of
  the declaration repairs that, so the nullability is gone instead: an omitted
  priority now resolves to `MessagePriority.Medium` in TypeScript, exactly as
  `sendMessage` has always done, and crosses to Swift and Kotlin as a required
  integer. No caller sees a behaviour change on either platform, because the
  core already resolved an absent priority to Medium and the null therefore
  carried no information. The check that refused it is compiled out of release
  builds, so only development was affected.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
| [0.24.x](docs/changelog/0.24.md) | 0.24.0 |
| [0.23.x](docs/changelog/0.23.md) | 0.23.0 |
| [0.22.x](docs/changelog/0.22.md) | 0.22.0 |
| [0.21.x](docs/changelog/0.21.md) | 0.21.0 |
| [0.20.x](docs/changelog/0.20.md) | 0.20.1, 0.20.0 |
| [0.19.x](docs/changelog/0.19.md) | 0.19.0 |
| [0.18.x](docs/changelog/0.18.md) | 0.18.3, 0.18.2, 0.18.1, 0.18.0 |
| [0.17.x](docs/changelog/0.17.md) | 0.17.0 |
| [0.16.x](docs/changelog/0.16.md) | 0.16.6, 0.16.5, 0.16.4, 0.16.3, 0.16.2, 0.16.1, 0.16.0 |
| [0.15.x](docs/changelog/0.15.md) | 0.15.0 |
| [0.14.x](docs/changelog/0.14.md) | 0.14.0 |
| [0.13.x](docs/changelog/0.13.md) | 0.13.1, 0.13.0 |
| [0.12.x](docs/changelog/0.12.md) | 0.12.0 |
| [0.11.x](docs/changelog/0.11.md) | 0.11.1, 0.11.0 |
| [0.10.x](docs/changelog/0.10.md) | 0.10.0 |
| [0.9.x](docs/changelog/0.9.md) | 0.9.4, 0.9.3, 0.9.2, 0.9.1, 0.9.0 |
| [0.8.x](docs/changelog/0.8.md) | 0.8.0 |
