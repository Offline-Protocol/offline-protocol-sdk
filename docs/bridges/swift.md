# Swift bridge contract

Covers the generated Swift bindings, the native iOS surface, and the React
Native iOS bridge.

Read [the shared contract](README.md) first. This document covers what is
specific to Swift.

## The generated layer

UniFFI produces Swift from the interface definition. It is one third of the
artifact set described in [C1](README.md#c1-regenerate-every-binding-together)
and is never regenerated alone.

The generated Swift lives under `ios/Generated/` and is committed. It carries
FFI checksums that must match the compiled library shipped alongside it.

## S1. The Objective-C bridge is hand-written and must be kept in step

React Native reaches Swift through an Objective-C bridge file declaring each
method with `RCT_EXTERN_METHOD`. **UniFFI does not generate it and React Native
does not generate it.**

Update it whenever you add an `@objc` method, change a parameter list, or change
a return type.

Type mapping:

| Swift | Objective-C |
|-------|-------------|
| `String` | `NSString *` |
| `String?` | `NSString *` (nullable) |
| `Int` | `NSInteger` |
| `Double` | `double` |
| `Bool` | `BOOL` |
| `NSNumber` | `nonnull NSNumber *` |
| Promise | `RCTPromiseResolveBlock` / `RCTPromiseRejectBlock` |

A method present in Swift and absent from the bridge is simply not callable from
JavaScript. There is no error at build time.

**A primitive and an object are not interchangeable, and mixing them does not
fail, it lies.** React Native chooses the `RCTConvert` converter from the
bridge's type text and the calling convention from the Swift parameter's runtime
encoding, then calls the one through a function pointer cast to the other. Pair
`nonnull NSNumber *` with a Swift `Int` and the returned object pointer is read
as a 64-bit integer, so the method runs on the pointer bits of a tagged
`NSNumber` rather than on the number; pair it with a `Double` and an integer
register is read as a floating-point one. The selector still resolves, the
method still runs, and nothing is logged. This table said `Int` to
`nonnull NSNumber *` from v0.3.3 until this release, and seven methods
followed it: message and
presence priorities were silently pinned to their `default:` arm, the battery
level to a clamp bound, and the three file-transfer scalars aborted the app on
a trapping conversion.

**The two halves must agree on the whole selector, not just the method name.**
React Native resolves each declared selector against the class when it parses
the module, drops any it cannot find, and logs that the JS method will not be
available. A renamed parameter label is therefore as fatal as a missing
declaration, and it is the easier of the two to ship: the `userId` to `profile`
rename reached Swift, Kotlin and TypeScript and missed this file, which left
`wipePersistedState` uncallable on iOS from 0.21.0 through 0.24.0.

**The first parameter must be unlabelled (`_`).** Swift exports
`f(resolver:rejecter:)` as `fWithResolver:rejecter:`, not as `f:rejecter:`, so a
labelled first parameter silently changes the selector. Fix that shape by
dropping the label in Swift, never by spelling the `With` form here: React
Native takes the JS method name from the selector text before its first colon,
so writing `fWithResolver:` in the bridge renames the JS method instead of
repairing it.

**An argument that arrives is still not a value you can narrow.** `UInt8(_:)`
and its siblings trap on out-of-range input: they abort the process rather than
returning something the bridge could reject. Every number crossing here came
from JavaScript, so out-of-range is a caller mistake, and a caller mistake that
aborts is a crash any caller can reach. Byte arrays go through the `jsBytes`
helper, which throws into the rejection the call site already has; scalars are
bounded where they are written, or behind a `guard` that rejects. Twelve array
conversions and the `initialTtl` config field were unbounded until this
release, which made a malformed BLE fragment and an `initialTtl: 300` both fatal on iOS
and harmless on Android.

All of these are pinned in `offline-protocol-uniffi`, in `cargo test`, because
neither compiler sees both halves and this file's Swift counterpart is the one
bridge source no CI job compiles.
`react_native_ios_objc_shim_and_swift_agree_on_every_selector` reads both files
and compares them as sets: the selectors in both directions, and then, behind
each shared selector, the ABI class of every parameter. It also checks that the
TypeScript only calls methods the bridge exports, and refuses to pass when its
own scan finds nothing to check.
`react_native_ios_bridge_bounds_every_byte_it_builds_from_javascript` fails on
any byte conversion whose argument does not carry its own bound.

One gap is known and unfixable here: React Native forces every `NSNumber`
argument to non-null, so `forwardMessage`'s optional priority is refused before
the Swift method runs and its promise never settles in a debug build. It needs
a contract change across all three languages, tracked in
[#417](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/417).

## S2. Five registration points per new Swift file

A new Swift source file in the React Native iOS package must be registered in
five places, and missing any one produces a different, unhelpful symptom:

1. The podspec's source file list, which enumerates top-level files one by one
   (`ios/ble/**`, `ios/mesh/**` and `ios/Generated/*.h` are the only globs), so
   a new top-level file is invisible to CocoaPods until it is added. This one is
   not silent: the Rust guard
   `react_native_podspec_ships_every_hand_written_ios_source` fails `cargo test`
   on an unlisted top-level source. A file added under `ble/` or `mesh/` needs
   no podspec edit at all.
2. The Swift package manifest's target sources, for the typecheck harness.
3. Any exclusion list it must **not** be in.
4. The test target, if it has tests.
5. **The `.github/workflows/ci.yml` "iOS bridge typecheck" file list**, if the
   file is one the package manifest excludes. This is the one most often
   forgotten, because the local recipe globs the directory while CI enumerates
   explicitly: the local harness stays green and CI fails with `cannot find
   'YourNewType' in scope`.

The typecheck harness is only meaningful if the file is actually in it. Verify by
negative control: break the file deliberately and confirm the harness fails.

## S3. Emit requires a live instance, and the check is subtler than it looks

An event emitted when no React instance exists is lost. The core cannot know
that, so the bridge must check.

**An optional `@objc` protocol member accessed through an existential is a double
optional.** Testing it against nil is therefore always true, and the guard
silently passes. This has shipped as a bug. Unwrap both levels explicitly.

The precondition is pinned by the Rust guard
`react_native_ios_emit_gate_has_live_instance_precondition`, which reads the
bridge source, so removing the check fails `cargo test` rather than only failing
on a device.

## S4. Never hold a strong reference during deallocation

Taking a weak reference to an object that is already deallocating is a **hard
abort**, not a nil. Teardown paths must not capture self weakly and then
resurrect it.

A device reproduction of this class needs the app to fully initialize first; a
harness that tears down immediately after construction does not reach the state
where it fires.

## S5. Threading

- Status-plane FFI calls run off the main thread, on the module's own queue.
- Blocking socket I/O must **not** share a thread that a stop or teardown path
  waits on. Mixing latency classes on one confinement thread turns a slow socket
  into a hang.
- CoreBluetooth resolves cached versus dynamic characteristic behaviour at
  service registration time, not at read time. A characteristic whose value
  changes must be declared accordingly when the service is added.

## S6. Secure storage

There are two storage surfaces and they have different rules.

**Secure storage** (`MlsSecureStorage.swift`, backed by Keychain) holds MLS key
material. Adoption of an existing store is read-through plus claim, not copy. A
copy leaves two sources of truth.

**Protocol-state storage** (`ProtocolStateStorage.swift`, file-backed) holds
restartable protocol state and carries a per-record ceiling of `8 * 1024 * 1024`
that it must spell **exactly**. That value is not a local choice: it mirrors
`MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES`, and
`built_in_providers_mirror_the_transfer_ceiling` reads this source and asserts
the literal, so drift in **either** direction fails the Rust suite.

The relationship is easy to state backwards. The ceiling is a deliberate
superset of the core's own record cap plus its seal envelope, so a provider
enforcing it never rejects a record the SDK legitimately wrote. That
superset relation is what "above" refers to, and it is pinned separately by
`bounded_load_ceiling_is_a_superset_of_the_record_cap`. The provider's job is
not to stay above anything, it is to match the ceiling exactly.

This ceiling is a hand-mirrored constant across **four** sites in three binding
languages, Python included. See
[C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language).

## S7. Pinned constant lists

`RelayAnswerPrefixes.swift` holds one of the three copies of the relay-answer
exemption list. It is pinned against literals in `RelayAnswerPrefixesTests.swift`.

See [C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language).

## Testing

```bash
cd bindings/react-native/ios
swift test
```

Roughly one second. There is no reason to skip it.

The test target covers the policy and translation types that carry real logic,
21 suites at the time of writing, including config reading, relay control-op
translation, fragment buffering, rate limiting, presence policy, identity
binding, address declaration, legacy store adoption, the write-stall watchdog,
the superseded latch, and the pinned prefix list. Read `Package.swift` for the
current set rather than trusting this sentence.

**Error mapping is not in that list.** `ProtocolErrorBridge` depends on the
generated UniFFI module, so both it and its test suite are excluded from the
package manifest; they ride the app build only. The same holds for the mesh
controller and the Bluetooth discovery bootstrap policy: suites exist, `swift
test` does not run them.

**Excluded from `swift test` does not mean unchecked.** A separate CI step,
"iOS bridge typecheck (files excluded from the SwiftPM harness)", runs `swiftc
-typecheck` over the sources the package manifest leaves out, including
`BleManager.swift`, the Wi-Fi Direct and Reticulum managers, and their `mesh/`
and `ble/` collaborators. They are typechecked on every run, just not
unit-tested.

**Two files are covered by neither**, and both ride the app build alone:

- `OfflineProtocolModule.swift`, which needs real React headers. The
  symlink-farm harness in `ios/BRIDGE_MAINTENANCE.md` exists for this one. If
  you touch it, run the harness, and negative-control it: a shell slip produces
  a clean exit that proves nothing.
- `ProtocolErrorBridge.swift`, which depends on the generated UniFFI module. It
  is on the package manifest's exclusion list and absent from the CI typecheck
  list, and its suite is excluded too, so nothing in CI compiles it.

Note that the comment above the CI step claims `OfflineProtocolModule.swift` is
the only uncovered file. That comment is stale; the exclusion list in
`Package.swift` is the source of truth.
