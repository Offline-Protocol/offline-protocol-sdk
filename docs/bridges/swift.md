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

## S2. Four registration points per new Swift file

A new Swift source file in the React Native iOS package must be registered in
four places, and missing any one produces a different, unhelpful symptom:

1. The podspec's source file list, which enumerates top-level files one by one
   (only `ios/ble/**` and `ios/mesh/**` are globbed), so a new top-level file is
   invisible to CocoaPods until it is added. This one is not silent: the Rust
   guard `react_native_podspec_ships_every_hand_written_ios_source` fails
   `cargo test` on an unlisted top-level source. A file added under `ble/` or
   `mesh/` needs no podspec edit at all.
2. The Swift package manifest's target sources, for the typecheck harness.
3. Any exclusion list it must **not** be in.
4. The test target, if it has tests.

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

The Swift binding implements the storage interface against Keychain. Two rules:

- The provider's per-record size limit must stay **above** the core's, or a
  record the core accepts fails to persist.
- Adoption of an existing store is read-through plus claim, not copy. A copy
  leaves two sources of truth.

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

The bridge module itself and the Bluetooth manager are excluded from the package
manifest's typecheck target for the same reason, dependencies on CoreBluetooth
and the generated bindings. A separate harness with a symlink farm typechecks
those; if you touch them, run it, and negative-control it.
