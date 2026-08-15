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
| `Int` | `nonnull NSNumber *` |
| `Bool` | `BOOL` |
| Promise | `RCTPromiseResolveBlock` / `RCTPromiseRejectBlock` |

A method present in Swift and absent from the bridge is simply not callable from
JavaScript. There is no error at build time.

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
