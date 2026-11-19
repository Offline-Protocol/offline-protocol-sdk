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

`RelayAnswerPrefixes.swift` holds one of the four copies of the relay-answer
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

The browser rebuilds its adverts from every record it holds after each
change, never removing one by its address. A peer back from the background
can be advertised by the record it just published and by its old one still in
our cache, and removing the old one by address took the live one's address
with it, so the peer was never dialed again. The instance name is a digest of
the address, as on Python, so a restarted listener replaces its record
instead of adding a second. A record whose dial is answered by a peer that
does not prove the address it advertises is left out until the browser
reports it again, so another record for the address is dialed, or none;
redialed, it failed the same way for as long as it lived and kept the real
record undialed. A dial gets ten seconds to become ready, whatever it
waits on (resolving the record, a path, the handshake), as on Python and
Android, before the redial ladder takes it over.

Writes follow Android's bounds: a body is dropped (the core retries it) when
more than 4 MiB is queued toward a peer whose oldest write has been
outstanding for two seconds, and a write outstanding for thirty seconds ends
the stream. Every stream carries TCP keepalive with the Python manager's
timers. At most sixteen streams are open, and at most twelve of them
inbound: the listener is open to the whole LAN, and the four left over are
what keeps this device able to dial whatever strangers' sockets hold the
rest. At most four inbound streams come from one remote address, the Python
manager's per-host bound scaled to this budget, so one address cannot hold
the whole inbound share. The bound counts addresses, not machines (one with
many IPv6 addresses is many), so it limits the easy case, and the reserve is
the guarantee. A local-network denial is reported
as a diagnostic naming `NSBonjourServices`.

`PeerStreamSessionTests` drives the session with string handles, a fake
carrier and a manual clock, `PeerStreamReaderTests` the reader with every
chunking, `PeerStreamDialPolicyTests` when to dial and redial (the ladder
climbs only on a redial actually scheduled, a dial that finds no free slot
waits on it, and a stale record never takes a live one's address), and `PeerStreamFramingTests`
replays the chapter's vectors and pins the advert's TXT bytes and instance
name. The manager's own use of Network framework
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

## S10. A stream a page dialled is a row in the one stream table, in the foreground only

**Invariants:** a stream a page dialled to this device's WebRTC Direct
listener ([ADR 0039](../adr/0039-a-browser-dials-a-native-device-from-a-code.md),
[stream framing](../spec/stream-framing.md#dialling-a-listener-from-a-browser))
is held in the same table as the TCP streams of S8, under the same caps and
the same one-announced-stream-per-address rule, so the core hears one
announcement and, later, one loss per address whichever kind carries it. Of
two dialled streams proving one address the newer is kept; against a TCP
stream the existing one stays, in both directions. A dialled stream counts as
inbound, from the address its page's first check came from. A stream proving
this device's own address is refused. The dial code is returned to the
application and never logged. The listener's socket is bound to the Wi-Fi
interface and to its one private IPv4 address, never a wildcard. The listener
lives only while the application is in the foreground.

`WifiDirectManager.openWebrtcDirect()` (React Native `openWebrtcDirect()`)
opens the listener while the transport runs and returns the code;
`closeWebrtcDirect()` closes it, and so do `stop()`, `pause()`, the
application leaving the foreground, and the Wi-Fi interface going away or
losing the bound address, since its code then names an address no page can
reach. Opening again closes the previous listener first, its streams reported
lost: the socket, certificate and credentials are fresh on every open.

The application hears every close it did not ask for, once, as the React
Native `webrtc_direct_closed` event with a reason: `background` (it left the
foreground), `paused`, `stopped`, `network` (the Wi-Fi interface went or lost
the bound address) or `socket` (the socket failed). A close of its own
(`closeWebrtcDirect()`) or by the next open reports nothing. Before the event
a diagnostic was the only signal, so an application that came back to the
foreground kept showing a code whose listener was gone, and every page that
dialled it timed out with nothing to say why. The report is the listener's
own (`openWebrtcDirect(onClosed:)`, which `WebrtcDirectListener` calls once,
after its first close that names a reason), so a listener closed twice, or
one an open replaced, cannot report twice or late.

The listener itself (admission by authenticated check only, DTLS, the channel,
the preamble and the framing) is the engine's `webrtc_direct_*`.
`WebrtcDirectListener` moves only what the UDL contract gives it: every
datagram the socket receives, unfiltered; every datagram and event the engine
queued, after every call; and a tick on a timer re-armed from the tick's own
return. The socket's read source, the timer and every call into the engine
run on `linkQueue`, the queue the stream table already lives on (S8), so the
engine's listener is never entered from two threads at once, and a handler
that calls back in (a refusal, a supersede) only marks the pump to go round
again, so events are handled in order and never inside one another.

- **One table.** `PeerStreamSession.dialProved` puts the stream in the same
  `PeerStreamLinks` as the TCP streams, through the same announcement path,
  and `dialReceived` and `ended` deliver and report through the same calls.
  The manager's rows (`Held`, with `Stream` and `DialledStream` under it)
  share one set, which is what the caps count. A table per kind would let a
  page and a TCP peer each announce the same address, and the first close
  would tell the core a reachable peer is gone, which a copied preamble
  ([R16](../security/threat-model.md#r16-the-identity-assertion-is-static-and-replayable-on-every-carrier))
  is enough to cause.
- **The newer dialled stream wins.** `PeerStreamLinks.announce` decides it
  ahead of the lower-address rule of S8. Only the page dials, so that rule
  has no race to settle here, and under it a device whose address is the
  lower would keep the older dialled stream: a page that reloaded, or came
  back after a Wi-Fi drop, would be refused until the dead stream's consent
  ran out, up to 15 s, while the page, which closed its old link before
  dialling again (W3, W16), waits on a stream this device will not hold. The superseded stream is closed with `webrtc_direct_close_stream`,
  out of the table first, so its `Closed` reports nothing. Against a TCP
  stream the existing one stays, so neither kind can take an address from
  the other. `every_peer_stream_manager_keeps_the_newer_dialled_stream` pins
  the lines, beside Android's (K10) and Python's (P18).
- **The caps cover both kinds**: 16 streams, 12 inbound and 4 from one
  remote address, the chapter's mobile values, through
  `PeerStreamDialPolicy.admitsDialled`. A dial past a cap is closed and never
  announced. A dialled stream it would supersede is not counted, so a page at
  the cap can still reload. Counted as outbound, dialled streams would let
  one machine hold the per-address bound twice over. The TCP listener and the
  dials count the dialled rows too.
- **A BSD datagram socket, not Network framework.** The engine names a
  destination for every datagram, to whichever page sent the check, from the
  one port the code names. An `NWListener` over UDP hands over one
  `NWConnection` per remote and cannot send to a remote it has made none
  for; a second connection binds a port of its own unless reuse is forced,
  and an answer from any port but the code's is not the candidate the page
  checked, so its ICE agent drops it and the dial never completes. The
  socket is bound with `IP_BOUND_IF` to the interface Network framework
  reports as Wi-Fi and to the address `WebrtcDirectListener.listenAddress`
  picks from it (private IPv4, else IPv4 link-local). Unbound, its answers
  could follow the default route, which on a Wi-Fi with no internet is
  cellular, and a wildcard address cannot go in a code and would admit
  checks from networks the code was never shown on.
- **Foreground only** (ADR 0039 decision 8). A suspended application's socket
  answers no page, and its streams would wait out their 15 s consent instead
  of ending, so the manager closes the listener on
  `UIApplication.didEnterBackgroundNotification` and in `pause()`, whichever
  comes first, each held stream reported lost once. Nothing reopens it: the
  application opens it again on return and shows the new code.
- **Local network permission.** Answering a page on the LAN is local network
  traffic, so on iOS 14 and later the application needs
  `NSLocalNetworkUsageDescription` and the user's consent, as for S8. No
  Bonjour service is involved, so nothing is added to `NSBonjourServices`.
  Without consent iOS refuses the listener's answers to the page and the
  dial times out with nothing said on the page's side, so the first send
  the socket refuses is reported as a diagnostic naming
  `NSLocalNetworkUsageDescription`.
- **The code never reaches a log.** It carries the listener's ICE password
  and admits whoever holds it while the listener is open
  ([R35](../security/threat-model.md#r35-a-dial-code-admits-whoever-holds-it-to-an-open-listener)).
  Diagnostics carry the host, port and address only, and
  `WebrtcDirectDialCode`'s description, debug description and mirror leave
  the fragment out, so a stray interpolation or `dump` cannot leak it. The
  generated `WebrtcDirectCode`, which prints every field, never leaves the
  manager's adapter. This is the fast link's passphrase rule (ADR 0038)
  applied to a code.

`WebrtcDirectDialledStreamsTests` pins the table's rules and the caps, and
`WebrtcDirectListenerTests` the socket, the pump, the clock, the close and
the code, on a fake engine over loopback sockets. A page dialling a phone is
covered only by a device run (C9).

## Testing

```bash
cd bindings/react-native/ios
swift test
```

Roughly one second. There is no reason to skip it.

The test target covers the policy and translation types that carry real logic,
37 suites at the time of writing, including config reading, relay control-op
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
-emit-sil` over the sources the package manifest leaves out, including
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
