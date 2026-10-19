# Flutter bridge contract

Covers the Flutter plugin under `bindings/flutter/offline_protocol`: Dart API,
Android and iOS host plugins, and the shared React Native bridge sources they
compile ([ADR 0025](../adr/0025-native-packages-are-assembled-in-place.md)).

Read [the shared contract](README.md) first. This document covers what is
specific to Flutter.

Integration steps live in [bindings/flutter/README.md](../../bindings/flutter/README.md).

## The generated layer

UniFFI produces Kotlin and Swift from the interface definition. Flutter does not
ship a fourth bindgen output: it links the same generated artifacts as React
Native (`bindings/react-native/android/.../uniffi`, `ios/Generated/`). See
[C1](README.md#c1-regenerate-every-binding-together).

Regenerate with `./scripts/generate-bindings.sh` and rebuild mobile libraries
before changing the UDL.

## F1. Shared bridge sources, explicit compile surface

Hand-written transport and config code lives under
`bindings/react-native/{android,ios}`. The Flutter plugin whitelists sources in
`android/build.gradle` (`pr3KotlinIncludes` + KotlinCompile excludes) and
`ios/offline_protocol.podspec` (`pr3_bridge_swift`). React Native–only entry
points (`OfflineProtocolModule`, mesh foreground services, sticky event
dispatcher, and similar) stay out of the Flutter compile graph.

Rust guards in `offline-protocol-uniffi` pin the compile lists and the
method-channel surface against Dart (`flutter_pr3_compile_lists_*`,
`flutter_pr3_method_channel_methods_match_native_plugins`).

The failure this prevents: a rename or a new bridge file that React Native
compiles but Flutter omits (or the reverse), found only when an app builds.

## F2. Engine thread and main-thread callbacks (K2 parity)

UniFFI and transport lifecycle run on a dedicated engine thread
(`OfflineProtocolEngine` on Android, `com.offlineprotocol.flutter.engine` on
iOS). Method-channel results and `EventChannel` deliveries are posted on the
main looper / main queue.

Nothing blocking runs on the UI thread. See [K2](kotlin.md#k2-nothing-blocking-on-the-main-looper).

## F3. Method-channel API is pinned across three languages

Dart calls `MethodChannel('com.offlineprotocol/offline_protocol')`. The method
names are listed in `OfflineProtocol.nativeMethodChannelMethods` and must match
`OfflineProtocolPlugin.kt` and `OfflineProtocolPlugin.swift` exactly. CI enforces
this via `flutter_pr3_method_channel_methods_match_native_plugins`.

New surface area: add the Dart wrapper, both native handlers, and extend the
const list (tests fail if the list drifts).

## F4. Events are opaque JSON with optional seq and replay

Events cross as JSON strings ([C3](README.md#c3-events-cross-as-opaque-json)).
The event channel may attach monotonic `seq` values. Dart `NativeEventPipeline`
implements the same gap-detection and `replayEvents` contract as React Native
`admitNativeEvent`.

Native code assigns `seq` and records to `EventReplayRing` only while an event
sink is attached. Apps must subscribe to `OfflineProtocol.events` **before**
`createFromConfig` if they need every event or replay recovery.

## F5. Config parsing

`createFromConfig` accepts JSON (camelCase or snake_case). Native
`ProtocolConfigParser` and `InitialRuntimeConfigApplier` are shared with React
Native ([C6](README.md#c6-config-parsers-must-not-default-to-literals)). Dart
may auto-enable transports after `start()` mirroring the React Native JS layer
(Internet, Wi‑Fi Direct).

## F6. Secure storage

MLS and protocol state use the same Kotlin/Swift storage adapters as React
Native (`MlsSecureStorage`, `AppContainerProtocolStateStorage` /
`ProtocolStateStorage`). See [K4](kotlin.md#k4-secure-storage) and
[S6](swift.md#s6-secure-storage).

## F7. Session lifecycle and foreground Internet heal

`destroy` tears down native resources; Dart cancels and reattaches the event
channel subscription while keeping the broadcast `events` stream. `stop` and
`createFromConfig` reset Dart seq tracking to stay aligned with native numbering.

Foreground Internet reconnect uses shared `ForegroundReconnectPolicy` (Android
`ActivityLifecycleCallbacks` on the plugin activity, iOS `UIApplication`
notifications).

## F8. Telemetry

The Flutter plugin does not yet call `enableTelemetry`. When it does, it must
follow [C12](README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform):
fill platform fields in the binding, own the session boundary off UI pause
granularity, and never run blocking flush on the main thread.

## F9. Binary footprint

PR3 links BLE and mesh bridge sources into the plugin AAR/CocoaPod. CI runs
`bindings/flutter/scripts/measure-plugin-aar-size.sh --check` against a
committed baseline (`bindings/flutter/baselines/`). Re-baseline intentionally
after compile-surface or UniFFI artifact changes.

## What CI runs

The `Flutter Binding` job in `.github/workflows/ci.yml` (path-filtered on pull
requests; full run on pushes to `main`):

- `flutter test` and `flutter analyze` on `bindings/flutter/offline_protocol`
- `:offline_protocol:assembleRelease` via `measure-plugin-aar-size.sh --check`
  after building arm64 UniFFI into jniLibs (one Gradle invocation)

iOS pod compilation is not gated yet (requires XCFramework assembly like the
Swift Package job); add when a macOS Flutter job is worth the runner cost.
