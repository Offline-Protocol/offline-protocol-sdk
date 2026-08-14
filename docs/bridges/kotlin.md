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

The Kotlin binding implements the storage interface against Keystore. The same
two rules as Swift apply:

- the provider's per-record size limit stays **above** the core's,
- adoption of an existing store is read-through plus claim, never a copy.

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

All three are pinned in `ProtocolConfigParserTest.kt`. Add a case there for
every new field rather than trusting the parser's shape.

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

Robolectric is used where a platform type is unavoidable. It needs an explicit
SDK level in the test configuration; without one it picks a level the project
does not target and fails for unrelated reasons.

Running the Android unit tests locally has a known gotcha with the React Native
Android artifact; the workaround is to copy into a clean directory. See
[the Android integration guide](../android-integration.md).
