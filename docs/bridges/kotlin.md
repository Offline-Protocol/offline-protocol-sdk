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
file in the publish job.** The generated artifact, not the committed one, is what
ships. Any generation step in CI must therefore go through the same script, or
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

`RelayAnswerPrefixes.kt` holds one of the three copies of the relay-answer
exemption list, pinned in `RelayAnswerPrefixesTest.kt`.

See [C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language).

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
