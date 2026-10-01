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
bounded where they are written, or behind a `guard` that rejects. A clamp
*around* a narrowing conversion does not count, because the conversion runs
first and traps before the clamp applies; that reaches unsigned values too,
since a negative JavaScript number arrives at `uint64Value` as `UInt64.max`.
Twelve array conversions, the `initialTtl` config field and two DORS config
paths were unbounded until this release, which made a malformed BLE fragment,
an `initialTtl: 300` and a `historyWindowSize: -1` all fatal on iOS and
harmless on Android.

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

**A nullable number is not something this bridge can express**, and the guard
above cannot say so. React Native forces every `NSNumber` argument to non-null
whatever the declaration says, because numbers are not nullable on Android, and
it refuses a null one before entering the Swift method, so neither the resolver
nor the rejecter runs and the promise never settles. The check lives inside
`#if RCT_DEBUG`, so the release build passes the null through and only
developers meet the hang. `forwardMessage`'s priority was declared that way
until this release. The repair is not a spelling of the declaration but the
removal of the nullability: TypeScript resolves the documented default, and the
shim, Swift and Kotlin all take a required integer. That costs nothing, because
the core already turns an absent priority into Medium. The guard misses this class
because a nullable number and a nullable object share an ABI class, so both
halves agree while React Native rejects the call anyway.

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
2. The target sources in `bindings/react-native/ios/Package.swift`, the
   manifest of the test harness. Not the Swift package that ships, which
   takes a new source without being told ([below](#the-swift-package)).
3. Any exclusion list it must **not** be in.
4. The test target, if it has tests.
5. **The `.github/workflows/ci.yml` "iOS bridge typecheck" file list**, if the
   file is one the harness manifest excludes. This is the one most often
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

## S8. A peer stream's verdict is its preamble, not its connect

`WifiDirectManager` fills the peer-stream slot with TCP streams over Network
framework, framed as [the chapter](../spec/stream-framing.md) specifies, and
found through DNS-SD `_offlineprotocol._tcp` on the network or over AWDL
([ADR 0027](../adr/0027-ios-peer-streams-ride-network-framework.md)). A
connected peer is announced to the core only under the address
`verifyIdentityAssertion` derived from its first frame, and only then: never
on connect, and never from the `addr` in its TXT record, which is only a
claim the preamble must match. A record carrying `sid` is a service
instance under the [DNS-SD mapping](../spec/dns-sd-mapping.md), not a peer,
and the browser ignores it as the stream chapter requires.

The rules live in two Foundation-only pieces. `PeerStreamReader` cuts the
stream into whole frames and refuses a prefix out of bounds before it
buffers a byte of the body. `PeerStreamSession`, generic over the stream
handle, owns the preamble, the frame rules and every call to the core. The
manager starts its listener, its browser and every connection on one serial
queue, and forwards every per-stream event from there. That queue is the
invariant, for two reasons: a stream's first frame must not overtake its
connect, and a frame must never be delivered after the peer's loss report,
because the core re-adds a neighbour on any inbound body. Every callback
checks that its listener, browser or stream is still the manager's, and
`stop()` forgets all three before it reports every peer lost.

Both ends of a pair browse, so both can dial. Of two streams for one
address, the one the lower address opened is kept, and the newer of two
such, the Python manager's rule, so an iPhone and a host agree on the same
stream ([P9](python.md#p9-a-socket-clients-verdict-is-the-preamble-not-the-connect)). The lower address dials at once, and the
higher waits five seconds for the lower one's stream to arrive. A refused
duplicate closes with no loss report. A stream handle is a new object per
connection, never an id a later stream could share: the Multipeer manager
this replaced keyed its peers by an id that survived a restart, and a
reconnect met state that said its preamble was already sent.

Writes follow Android's bounds: a body is dropped (the core retries it) when
more than 4 MiB is queued toward a peer whose oldest write has been
outstanding for two seconds, and a write outstanding for thirty seconds ends
the stream. Every stream carries TCP keepalive with the Python manager's
timers. At most sixteen streams are open. A local-network denial is reported
as a diagnostic naming `NSBonjourServices`.

`PeerStreamSessionTests` drives the session with string handles, a fake
carrier and a manual clock, `PeerStreamReaderTests` the reader with every
chunking, `PeerStreamDialPolicyTests` when to dial and redial (the ladder
climbs only on a redial actually scheduled), and `PeerStreamFramingTests`
replays the chapter's vectors. The manager's own use of Network framework
(advertise, browse, connect) is not covered in CI (C9).

## S9. Every object is built for the pod's deployment target

No object in the iOS library declares a minimum OS newer than the one the
podspec declares. The arm64 simulator slice is held to iOS 14.0 where the
pod's target is below that, because no arm64 simulator exists before 14.0 and
the toolchain raises the slice to it.

Two compilers read `IPHONEOS_DEPLOYMENT_TARGET`, and they fall back
differently when it is unset. The Rust compiler falls back to the target's own
floor, which is below anything the pod would declare. The C compiler behind
`cc-rs`, which builds the C and assembly in `ring` and `oslog`, falls back to
the version of the installed SDK. So with the variable unset those objects
were stamped for whichever Xcode built the release, and the linker of every
application using the pod warned about each of them.

Three things hold the invariant:

- `bindings/react-native/scripts/build-uniffi-ios.sh` reads the target out of
  the podspec and exports it before it builds. The number is written once, in
  the podspec. The SDK's own Rust code is compiled for it too.
- `package_xcframework` reads every object in every slice before it packages
  anything, and refuses an archive that holds one too new. An archive it
  could not read fails the same way: one with no stamp in it, and one
  `otool` read part of before giving up. The check is inside the packaging
  function, not beside the call, so the pod's XCFramework cannot be packaged
  without it. The Swift package's XCFramework is built by
  `scripts/package-swiftpm-xcframework.sh`, which does not check; the
  `Swift Package` job runs the check on its archive as a step of its own.
- The gate needs `otool`, so in CI it runs where the library is built on a
  Mac: in a release, on all three archives, and in the `Swift Package` job,
  on the simulator archive of every pull request.
  `scripts/tests/test-ios-min-os.sh`, at the repository root, runs on every
  pull request on Linux. It covers what can go wrong without `otool`: the
  parser, the podspec reader, a failing `otool`, and the packaging function
  itself, driven with stand-ins for `otool`, `lipo` and `xcodebuild`: each
  archive is held to its own ceiling, and a refused one packages nothing.

The podspec has one reader, `ios_deployment_target` in
`bindings/react-native/scripts/shared/xcframework.sh`.
`scripts/ios-deployment-target.sh` calls it for the Swift package and its CI
job. It takes the line only when it is there once, with a version of two or
three components. Two readers with two rules disagreed on some podspecs, and
the release would have built for a number the package refused.

A target directory built for one deployment target keeps its Rust objects
when the target changes, because cargo does not rebuild a crate for a new
value of the variable. Only the C compiled through `cc-rs` is redone. A
lowered target therefore fails the gate on Rust objects until `cargo clean`.
Both CI caches that hold iOS objects, the `Swift Package` job's and the
release's, add the podspec's hash to their key, so a changed target starts
from a cold cache there.

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
harness manifest. The same holds for the mesh controller and the Bluetooth
discovery bootstrap policy: suites exist, `swift test` does not run them. The
`Swift Package` job does, as described at the end of this section.

**Excluded from `swift test` does not mean unchecked.** A separate CI step,
"iOS bridge typecheck (files excluded from the SwiftPM harness)", runs `swiftc
-typecheck` over the sources the package manifest leaves out, including
`BleManager.swift`, the Wi-Fi Direct and Reticulum managers, and their `mesh/`
and `ble/` collaborators. They are typechecked on every run, just not
unit-tested.

**Two files need more than that step supplies**, and a second step covers
both: "iOS bridge typecheck (OfflineProtocolModule.swift with React)" installs
the React Native headers from the pinned devDependency, builds the symlink
farm `ios/BRIDGE_MAINTENANCE.md` describes, and typechecks every hand-written
source in `ios/` by glob, so a new file is covered without being listed.

- `OfflineProtocolModule.swift` needs real React headers. It had no compile
  coverage before that step, and a parameter named like the method it called
  broke every iOS app build while CI stayed green. If you touch it, run the
  local harness too, and negative-control it: a shell slip produces a clean
  exit that proves nothing.
- `ProtocolErrorBridge.swift` depends on the generated UniFFI module and is
  absent from the first step's enumerated list, so the glob is what reaches
  it. Its suite is still excluded from `swift test`.

The exclusion list in `Package.swift` remains the source of truth for what
`swift test` skips.

**The `Swift Package` job runs every suite, the excluded three included.** It
builds the Rust library for the simulator, assembles the package with
`--bridge-tests` and runs it with `xcodebuild test`
([bindings/swift](../../bindings/swift/README.md#build-and-test-it)). That is
the only place the mesh controller, the BLE discovery bootstrap policy and
the error mapping suites execute, and the only place anything in the bridge
is linked against the library: `LinkTests` calls into Rust, and runs the
storage conformance suite against the built-in state store (C11). The job
then builds `bindings/swift/consumer-check`, which depends on the package as
an application would, with a plain import, and calls into the library from
outside the module.

## The Swift package

The Swift package is these sources, assembled. It is not a second copy of
the bridge. `scripts/assemble-swift-package.sh`
writes a directory Swift Package Manager can build, from the sources in
`bindings/react-native/ios` and the generated bindings, beside a manifest
rendered from `bindings/swift/Package.swift.template`
([C13](README.md#c13-a-shared-bridge-source-compiles-without-react),
[ADR 0025](../adr/0025-native-packages-are-assembled-in-place.md)).

Four things about it fail in ways that do not point at their cause:

- **The Swift module is `OfflineProtocolSDK`, not `OfflineProtocol`.** The
  generated bindings declare a class called `OfflineProtocol`, and a module
  that shares a name with one of its own types cannot be used to qualify
  anything in it.
- **The test harness's stand-ins are behind `OFFLINE_PROTOCOL_HARNESS_SHIMS`,
  not `SWIFT_PACKAGE`.** Swift Package Manager defines `SWIFT_PACKAGE` for
  every package it builds. Under that switch the package declared
  `MlsStorageError`, `MlsStorageProvider` and `ProtocolStateStorageProvider`
  twice, once as a stand-in and once generated.
- **The header sits in `Headers/offline_protocolFFI/` inside each slice, not
  in `Headers/`.** Xcode copies the headers of every binary an application
  links into one directory, and a module map is always called
  `module.modulemap`, so two libraries that ship one at the top overwrite
  each other. Every library UniFFI generates ships one.
- **The manifest stays at tools version 5.9.** It selects the Swift 5
  language mode. Under Swift 6 the storage providers do not compile.

The XCFramework the pod ships has no headers in it, because the podspec
supplies search paths. `scripts/package-swiftpm-xcframework.sh` builds the
package's own from the same archives.

The package takes a source at the top level of `ios/`, or under `ble/` or
`mesh/`, without being told, which is what the podspec takes. A source in a
new directory is in neither until it is named in both.
