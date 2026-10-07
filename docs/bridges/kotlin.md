# Kotlin bridge contract

Covers the generated Kotlin bindings, the native Android surface, and the React
Native Android bridge.

Read [the shared contract](README.md) first. This document covers what is
specific to Kotlin.

## The generated layer

UniFFI produces Kotlin from the interface definition. It is one third of the
artifact set described in [C1](README.md#c1-regenerate-every-binding-together)
and is never regenerated alone.

**The release workflow regenerates the Kotlin and downloads it over the committed
file in the publish job and in the job that builds the Android library.** The
generated artifact, not the committed one, is what ships. Any generation step in CI must therefore go through the same script, or
the released Kotlin and the released Swift were built by different bindgens.

## K1. Platform callbacks arrive on binder threads

Bluetooth GATT callbacks and most Android system callbacks are delivered on
binder threads, not on the main looper and not on your own executor.

Anything they touch is concurrent. Fields read across such a boundary need
explicit visibility guarantees; a `@Volatile` on a status field is load-bearing,
not decorative.

## K2. Nothing blocking on the main looper

FFI calls and socket work must not run on the main looper. A blocking call there
is an application-not-responding report, and the SDK gets the blame for the
freeze regardless of which layer blocked.

A scheduled executor with a fixed-rate schedule runs **gapless on overrun**: if
one tick takes longer than the period, the next fires immediately. For work whose
duration varies, prefer a fixed delay.

## K3. Foreground service lifecycle

A sticky foreground service restart is exempt from the API 31 and later
restriction on starting a foreground service from the background. That exemption
is what makes mesh wake work.

The wake service is referenced by fully-qualified class name string, so a rename
or a package move is not caught by the compiler. Grep for the string.

Invalidation of the service handle is deliberately unsynchronized; the design
tolerates a lost race rather than holding a lock across a platform call.

## K4. Secure storage

The same split as Swift applies, with the same two rules. `MlsSecureStorage.kt`
implements secure storage against Keystore, where adoption of an existing store
is read-through plus claim, never a copy. `ProtocolStateStorage.kt` is the
file-backed protocol-state provider, and its `MAX_VALUE_BYTES` must spell
`8 * 1024 * 1024` exactly, matching
`MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES` rather than merely exceeding
anything. See [S6](swift.md#s6-secure-storage) for why that distinction matters
and which Rust guard reads this file.

A logout wipe must clear every namespace the SDK wrote, which is a
bindings-level concern because only the binding knows the platform store layout.

## K5. Config parsing

The React Native Android bridge parses the config object handed down from
JavaScript. It must:

- distinguish an absent field from a field set to the default value, or every
  partial update silently resets what it did not mention (see
  [C6](README.md#c6-config-parsers-must-not-default-to-literals)),
- accept both a nested section and flat keys, with **nested winning**,
- accept both camelCase and snake_case spellings where the surface historically
  did.

The last two are pinned in `ProtocolConfigParserTest.kt`. Add a case there for
every new field rather than trusting the parser's shape.

The first is pinned elsewhere, and the split is not arbitrary. Preserving an
absent field is a property of the **update** path, which reads the live config
and merges, so it cannot be exercised against the parser alone: the module that
performs the merge cannot be instantiated in a plain unit test (see
[Running them locally](#running-them-locally)). The Rust guard
`react_native_bridges_merge_dors_updates_from_the_live_config` pins it instead,
by reading the bridge source. Note the consequence: `ProtocolConfigParserTest.kt`
pins literal defaults at **initial** parse, which is the opposite mechanism, so a
green run there says nothing about partial updates.

## K6. Sticky and one-shot events

Events that fire before JavaScript subscribes are lost. The Android bridge
carries a sticky buffer and a dispatcher for this, covered by
`StickyEventBufferTest.kt` and `StickyEventDispatcherTest.kt`.

Which mechanism applies depends on where the event fires relative to
subscription, not on what it means. See
[C10](README.md#c10-lifecycle-rules-for-event-emission).

## K7. Pinned constant lists

`RelayAnswerPrefixes.kt` holds one of the four copies of the relay-answer
exemption list, pinned in `RelayAnswerPrefixesTest.kt`.

See [C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language).

## K8. A peer stream's verdict is its preamble, not its connect

`WifiDirectManager` fills the peer-stream slot with Wi-Fi Direct group
sockets and, through `LanPeerDiscovery`, with TCP sockets on the Wi-Fi
network the device is on, framed as [the chapter](../spec/stream-framing.md)
specifies. A
socket's peer is announced to the core only under the address
`verifyIdentityAssertion` derived from its first frame: never on connect, and
never under the socket endpoint or `"go:<ip>"`, which is what this manager
once passed and which put strings into the core's bounded `known_peers`.

Everything between the socket and the core lives in `PeerStreamSockets`,
behind a small host interface, so it runs in CI on loopback sockets. It owes
the shapes P9 describes for Python, with the reasons given there: one
deadline over the whole preamble frame, a per-stream ordered writer (a shared
pool could write two frames to one socket at once and splice them), a write
deadline on progress rather than per frame, and a queue bound that drops only
while the writer is stalled on the peer. It also owes one thing Python gets
from asyncio's single thread: a stream's announcement, deliveries and loss
report run under that stream's lock, so no body reaches the core after its
loss report.

A peer that vanishes without a FIN stays announced until keepalive notices:
15 seconds idle, 5 between probes, 3 probes, the iOS and Python timers, set
through `Os.setsockoptInt` because `java.net.Socket` cannot set the
interval. The core's acknowledgements are still the real signal that a peer
is gone, as P9 says for the same reason.

`NsdManager`'s `DiscoveryListener` reports a record found and lost, never
changed. A peer that restarts and publishes the same instance name on a new
port, with no goodbye in between, is an update the listener never delivers,
where iOS hears a changed record and dials. So the LAN carrier dials back any
peer whose announced stream ends while its record is still advertised, on the
usual policy, and a dial that nothing answers resolves the record again
before the redial, which returns the cache's current port. Without both, an
Android phone left the restarted peer to dial it, and dialed the dead port
itself (seen on devices against an advertise-only Python host).

Of two streams for one address, the manager keeps the one the lower address
opened, and the newer of two such: P9's rule, compared by UTF-8 bytes, and
`every_peer_stream_manager_keeps_the_same_stream` pins it to the iOS and
Python copies. Inside a group only the client dials, so the rule reduces to
"newer wins" there, except that a client that is the higher address cannot
supersede its own half-open stream: its reconnect is refused until keepalive
ends the stale one, about thirty seconds. That is the price of the rule being
one rule; a pair that shares a group and a LAN has two dialers on one link
table (ADR 0028). A client whose stream ends while the group is up reconnects on a doubling
delay, always to the owner the group has at that moment: on a group switch
the new owner's first dial can lose to the old stream still closing, and the
redial is then the only one left. The manager joins a group the system formed, including one that
formed before it started: since Android 10 the connection broadcast is not sticky, so the manager asks
for the group once it is running. With `formGroups` (`wifiDirect.autoAccept`, Android 10 and later)
it also forms one: `WifiDirectGroupFormation` holds the rules (who creates, who joins, the network
name and passphrase, both derived from the application id) and `WifiDirectGroupFormationTest` pins
them. Three behaviours of the radio shaped them and are worth knowing before changing them. A DNS-SD
query by service type alone returns only the PTR record, so the TXT record that carries the address
arrives only for a query that names the instance. A device that owns a group, or is in the middle of
joining one, answers no service discovery query, so two phones often hear each other minutes apart or
not at all; that is why the group is named after the application and not its owner (a joiner needs
nothing from the owner), why a join attempt is cancelled after fifteen seconds (the joiner is
discoverable again; eight seconds cut off the supplicant's retry of a rejected association), why a
device that hears a lower peer creates the group itself after three joins found none (a join the
framework refuses outright counts), and why a device that heard no record still probes a group
owner it sees, at most once a minute. And two groups can form at once, so an owner whose group
stays empty for a randomised 30 to 60 seconds dissolves it and joins before creating again.

A device alone keeps looking, but less often: service discovery backs off from 15 to 60 seconds
while it hears no record, and the owner probe from one to four minutes while probes find no group
(a Wi-Fi Direct printer or television is an owner no probe joins). Both start over when a record,
a new owner or a new device appears. A device that is a client of any group, the application's or
another (Wi-Fi Direct printing, a screen cast), forms nothing while it lasts.

The group can outlive the process: the framework removes it only when no application on the phone
still holds Wi-Fi P2P. An owner whose application died leaves a group with no listener that
answers no discovery query, and every device that probes it joins a group that carries nothing, then
never runs formation again because it is in a group. Two rules close that: a group under the
application's name that this device owns is removed by `stop()` even when it was adopted at start
rather than created by the run, and a client of such a group leaves it after three dials at the top of
the redial ladder prove nothing (`GroupOwnerRedial.shouldLeave`). Leaving is not enough on its own:
every group of the application has one name, so a join by name lands on whichever owner of it the
supplicant picks (by signal), and the one in sight is the owner just left. The first version rejoined
it ten seconds later, every time. A join cannot be steered away from it: `setDeviceAddress` on a join
by credentials becomes the supplicant's `go_bssid`, which must equal the owner's P2P interface address,
and an application only ever sees device addresses, so a join named that way matches no network. So
the owner left is remembered for five minutes by device address (longer than an owner whose application
comes back takes to dissolve its empty group): it no longer counts as an owner nearby, a join that
lands in its group leaves at once and counts as one that found no group
(`WifiDirectGroupFormation.joinedLeftOwner`, `failedJoinsAfterGroupJoined`; the counters are settled
once the owner is known, never on `CONNECTION_CHANGED`, or each capture would zero them), so a device
that heard a peer creates after three, and its record heard again forgives it. Beside a dead owner the
merge is bounded by that takeover, not guaranteed. The redial goes on after a leave is asked for, so a leave the framework refuses is asked again. A
group paired in the system settings is never removed or left.

`formGroups` is set while stopped; `stop()` undoes what the run did (the service request, the
record, an application group it created or adopted) whatever the flag says by then. `enableTransport('wifiDirect')` with no
configuration keeps the current setting rather than turning formation off.

`PeerStreamSocketsTest` and `PeerStreamFramingTest` pin it, the latter
replaying the chapter's vectors. The group handling itself is not covered in
CI (C9).

## Testing

Unit tests live in
`bindings/react-native/android/src/test/java/com/offlineprotocol/`.

Robolectric is used where a platform type is unavoidable. Each suite that needs
it carries its own `@Config(sdk = [...])` annotation; there is no
`robolectric.properties` and no Robolectric block in `build.gradle`. Without the
annotation Robolectric picks a level the project does not target and fails for
unrelated reasons.

### Running them locally

CI runs them through the standalone harness in
`bindings/react-native/android-ci-harness`, and following that harness's README
on a development machine works with the currently pinned React Native.

It is worth knowing why, because the mechanism is one dependency bump away from
biting again. `android/build.gradle` chooses how to depend on React Native by
testing for `node_modules/react-native/**android**`, a legacy local Maven
repository:

| `node_modules/react-native/android` | Dependency | Result |
|-------------------------------------|-----------|--------|
| Absent (CI, and local dev on the currently pinned React Native) | `compileOnly` on a pinned version from Maven Central | Resolves |
| Present (React Native versions that still shipped that directory) | `implementation` with **no version**, from that local Maven directory | Resolves to an empty version and fails |

The directory test is deliberately on `android`, not on the package directory.
Newer React Native keeps its sources under `ReactAndroid/` and publishes no
`android/` Maven repo, so testing only for the package would produce an
unversioned `react-android` dependency and fail in the harness for a reason that
looks unrelated to whatever you changed.

If a future bump reintroduces that layout, or you need to reproduce CI exactly,
copy the module somewhere with no sibling `node_modules` and point a copy of the
harness at it:

```bash
rsync -a --exclude build/ --exclude .gradle/ bindings/react-native/android /tmp/rn-android-ci/
rsync -a bindings/react-native/android-ci-harness/ /tmp/rn-android-ci/harness/
cd /tmp/rn-android-ci/harness
ANDROID_HOME=~/Library/Android/sdk gradle :offlineprotocol:testDebugUnitTest
```

Needs JDK 17 and the Android SDK. The console prints only `BUILD SUCCESSFUL`;
for counts, read the `tests=` and `failures=` attributes in
`android/build/test-results/testDebugUnitTest/*.xml`.

Two consequences of the `compileOnly` path that shape what you can test:

- Anything React Native pulls in **transitively** is absent at test runtime.
  Production code compiles, then the test dies with `NoClassDefFoundError`. Add
  the specific androidx artifact as a test dependency when a new test reaches
  one.
- `OfflineProtocolModule` extends a React base class, so it cannot be
  instantiated at unit-test runtime **at all**. That is a design constraint, not
  only a testing one: put an invariant that needs coverage in a collaborator,
  not in the module.

## The Android library

The Android library is these sources, built without React Native. It is not a
second copy of the bridge. The Gradle build in `bindings/kotlin` points its
source sets at `bindings/react-native/android` and leaves out the three files
that need React
([C13](README.md#c13-a-shared-bridge-source-compiles-without-react),
[ADR 0025](../adr/0025-native-packages-are-assembled-in-place.md)).

What it takes from the module it reads from the module: the SDK levels and
the version of every dependency from `build.gradle`, the compiler and the
Android plugin from the test harness, the rules for an application's R8 from
`consumer-rules.pro`, and the manifest, which a task rewrites at build time
without the headless wake service and the `WAKE_LOCK` permission that only
React Native's headless task needs. That task fails when either entry is
missing, because a renamed entry would otherwise ship under its new name.

Six things about it are easy to get wrong:

- **It is built by the toolchain the module's tests run on, not by the
  newest.** Kotlin refuses metadata more than one version ahead of the
  compiler reading it. Built by Kotlin 2.2, the library could not be used by
  an application on Kotlin 1.9 or 2.0, which fails on every symbol in it.
- **JNA is an `api` dependency.** The generated types extend JNA's. An
  application never calls them, and can reach them, and with JNA published
  at runtime scope the compiler cannot see their supertypes.
- **`androidx.core` is declared by the library and not by the module.** The
  foreground service and both permission checks need it, and in a React
  Native application it arrives through React Native. The module declares it
  for its tests, which is the version the library reads.
- **A source set cannot leave a file out, so the tasks that read one are
  told.** The compiler is, and so is whatever packs sources. Left alone, the
  sources jar published the React module as the source of a library that
  does not contain it. Which task packs it is the plugin's to change, so
  `scripts/check_android_aar.py` looks inside the jar that is published.
- **The mesh wake is inert in the library.** `MeshForegroundService` starts
  the wake service by its class name, only when the application sets
  `MESH_WAKE_ENABLED`, and that class is not in the library. `startService`
  does not throw for a service that is not declared, it returns null, so the
  service resolves the name first. Set in a native application, the opt-in
  finds no such service and the keep-alive stops at once, as it does with the
  opt-in off. Before that check it held "Mesh Active" over no mesh until the
  watchdog fired.
- **Nothing calls `java.time`.** It needs API 26 and `minSdk` is 24, and on
  API 24 or 25 the call throws an `Error` that a `catch (e: Exception)` does
  not catch. The unit suite runs on a JVM that has it, so
  `android_bridge_sources_never_call_java_time` in the uniffi crate is what
  refuses one. `RelayTimestamps` parses ISO-8601 by hand.

The `Android Library` job runs the unit suite, and checks that it ran:
Gradle succeeds on a test task that found no test. It writes the library out
as a Maven repository, checks what is inside the AAR and the sources jar,
and builds `bindings/kotlin/consumer-check` against it. That last build is
an application that declares one dependency and nothing else, and minifies.
It fails when what it calls needs something the library's metadata does not
bring, and when R8 needs a rule the library does not ship. The second is how
it was found that no application could minify against the library: Tink,
which `security-crypto` brings, refers to annotation classes it does not
ship, and R8 treats a class it cannot find as an error.
