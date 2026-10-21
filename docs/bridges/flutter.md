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

## F1. Shared bridge sources, compiled less a named list

The plugin compiles every hand-written source under
`bindings/react-native/{android,ios}` except the ones it names, as ADR 0025
decision 3 asks: a list of what to leave out, never a list of what to take.
On Android that is the `KotlinCompile` excludes in `android/build.gradle`. On
iOS it is `rn_only_swift` in `ios/offline_protocol.podspec`, which must agree
with the `--exclude` list in `ios/sync_bridge_rn.sh`. What is left out needs
React (the module and package entrypoints, RN-only bridge adapters, and
`MeshHeadlessWakeService`). Reticulum, Nostr, `MeshForegroundService`, and on
Android `StickyEventDispatcher` / `StickyEventBuffer` compile with the plugin;
headless mesh wake stays React-only.
Release AAR
size is gated in CI ([F9](#f9-binary-footprint)); use
`OfflineProtocol.minimalFootprintConfig` when you ship a subset of transports.

A new shared source reaches the plugin without being registered. A list of
what to take would not: the Kotlin plugin ignores `java { include }`, so such
a list drives nothing while looking like the compile surface.

Rust guards in `offline-protocol-uniffi/src/tests/flutter_bridge_guards.rs`
hold this: `flutter_android_excludes_name_real_files` and
`flutter_ios_exclude_lists_agree_and_name_real_files` refuse an excluded name
that matches no file, which is a rename nobody carried over and would let the
renamed file in, and refuse an exclusion of a shared transport.
`flutter_method_channel_methods_match_native_plugins` pins the channel surface
(F3).

**The iOS copy can be stale.** CocoaPods indexes only files under the pod
root and does not follow a directory symlink, so `pod install` copies the
shared Swift sources into `ios/BridgeRN/`. An edit to a shared source followed
by a Flutter build without `pod install` compiles the old copy and nothing
says so. Run `ios/sync_bridge_rn.sh` or `pod install` after editing one.

## F1b. Values are read, not written again

The plugin's version, Android SDK levels, Android dependency versions and iOS
deployment target are read from the React Native package: the podspec reads
`package.json` and the deployment target line of `MeshSdk.podspec`, and the
Gradle build reads `package.json` and the module's `build.gradle`, each from a
line that has to be there exactly once (ADR 0025 decision 5). A copy is right
on the day it is made and drifts with nothing failing. `pubspec.yaml` cannot
read another file, so it carries the version itself and the release gate in
`release.yml` refuses a tag it does not match.
`flutter_native_builds_read_what_the_react_native_package_declares` holds
both.

## F2. Engine thread and main-thread callbacks (K2 parity)

UniFFI and transport lifecycle run on a dedicated engine thread
(`OfflineProtocolEngine` on Android, `com.offlineprotocol.flutter.engine` on
iOS). Method-channel results and `EventChannel` deliveries are posted on the
main looper / main queue.

Nothing blocking runs on the UI thread, detach included: engine stop can wait
on transport I/O, so detach queues the teardown behind the calls already
submitted and returns.

**The engine thread is one per process, not one per plugin instance.** Flutter
creates a new plugin instance for every engine, for example after an Android
activity is destroyed and relaunched. With a thread per instance, the new
engine's `create` would run while the old engine's teardown is still blocked
on transport I/O, leaving two cores with one identity on BLE and the relay,
and the old one able to flush protocol state over what the new one loaded.
So the Android executor lives in the plugin's companion object and is never
shut down, and the iOS queue is a `static let`: every call from every
instance queues behind the teardown before it.
`flutter_engine_thread_is_shared_by_every_plugin_instance` holds both. On
iOS, `deinit` tears down directly and never syncs onto the engine queue,
where it may itself be running.

`create` parses and validates its config before it tears down the running
engine, so a config the plugin refuses leaves the current session untouched.
See [K2](kotlin.md#k2-nothing-blocking-on-the-main-looper).

Every engine-thread failure is caught as `Throwable` on Android. The generated
bindings load the native library on first use, and a missing or mismatched
library throws `UnsatisfiedLinkError`, an `Error`: uncaught on the engine
thread it kills the process and the call is never answered. It is reported as
`NATIVE_LIBRARY_UNAVAILABLE`.

## F3. Method-channel API is pinned across three languages

Dart calls `MethodChannel('com.offlineprotocol/offline_protocol')`. The canonical
list is `method_channel_methods.txt` (171 React Native–parity methods plus
`replayEvents`). Dart mirrors it in `OfflineProtocol.nativeMethodChannelMethods`;
Android dispatches in `FlutterEngineChannel.kt`; iOS declares the same surface in
`FlutterEngineChannel.swift`. CI enforces parity via
`flutter_method_channel_methods_match_native_plugins`.

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

## F5b. iOS BLE and `Info.plist`

Shared `BleManager` uses CoreBluetooth state restoration. Before `BleManager.start()`
runs, the host app must declare `NSBluetoothAlwaysUsageDescription` and
`UIBackgroundModes` (`bluetooth-central`, `bluetooth-peripheral`). Same keys as
[React Native iOS setup](../../bindings/react-native/README.md#ios-permissions).
The example smoke path enables the Internet transport flag only so it can boot
without starting BLE; enabling BLE in config requires those plist entries.

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

## F7b. One-shot events (`internet_session_superseded`, `mesh_stopped_by_user`)

These tags report state nothing else restates. Delivery is at-least-once; apps
treat them as state, not one-time edges.

**Android** holds both in `StickyEventDispatcher` when the event channel has no
listener, flushes on subscribe and activity start, and discards held
`internet_session_superseded` when `enableTransport('internet', …)` re-enables
the relay (same as React Native). **`mesh_stopped_by_user`** is Android-only
(notification Stop); there is no iOS counterpart.

**iOS** re-derives `internet_session_superseded` from the live transport latch
on each app foreground (`supersedeRestatementJson()`), matching React Native iOS.
Use `MeshSdk.internetIsSupersededTyped()` after process restart when no event
was buffered.

Subscribe to `OfflineProtocol.events` before `createFromConfig` when you need
every engine event and gap replay; one-shots still benefit from the native
redelivery above when Dart was not listening at emit time.

## F8. Telemetry

Telemetry methods are exposed on the method channel (`enableTelemetry`,
`disableTelemetry`, `flushTelemetry`, `telemetryStats`, `telemetryInstallId`,
`endTelemetrySession`, `setTelemetryEnabled`) and in Dart via `MeshSdk`. Android
fills platform fields in `ProtocolEngineHost.parseTelemetryConfig` ([C12](README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform)).
The binding owns the session boundary off UI pause granularity and never runs
blocking flush on the main thread. `disableTelemetry`, `endTelemetrySession`, and
`flushTelemetry` may block the dedicated engine thread for up to a few seconds
(same as React Native); other method-channel calls queue behind that work. iOS
and Android Flutter hosts seed and update `AppState` from process lifecycle
observers so telemetry sessions follow foreground/background edges.

On shutdown or before tearing down the engine, prefer `MeshSdk.disableTelemetryAndWait`,
`endTelemetrySessionAndWait`, or `flushTelemetryAndWait` so your Dart `await`
matches the engine-thread flush. Calling other channel methods while a flush is
in flight is safe but queues behind it; avoid overlapping polls you rely on for
UI gating until the wait completes.

During in-app navigation you may call `MeshSdk.disableTelemetry()` without
`await` so Dart keeps rendering, as long as you defer engine polls you gate UI on
(for example `internetIsReadyTyped`) until that future completes or you call the
matching `*AndWait` helper before `destroy`.

## F8b. Staged native artifacts (pub.dev)

Release workflow runs `bindings/flutter/scripts/stage-plugin-native-artifacts.sh`
after mobile UniFFI artifacts are assembled. The plugin Gradle build prefers
`android/src/main/jniLibs` when staged; the podspec prefers
`ios/Frameworks/offline_protocol_uniffi.xcframework` when present. See
[flutter-integration.md](../flutter-integration.md).

## F9. Binary footprint

The plugin links BLE and mesh bridge sources into the AAR and CocoaPod. CI runs
`bindings/flutter/scripts/measure-plugin-aar-size.sh --check` against a
committed baseline (`bindings/flutter/baselines/`). The baseline is the
arm64-v8a plugin with the library built under the `mobile` profile, the one
the Android release ships ([ADR 0029](../adr/0029-mobile-libraries-are-sized-not-aborting.md)).
The script subtracts any other ABI present under the React Native `jniLibs`,
so the local check measures what CI measures. Re-baseline intentionally after
compile-surface or UniFFI artifact changes.

Reticulum and Nostr managers compile with the Flutter Android library (F1), so
the release AAR is larger than a BLE-and-Internet-only build. That growth is
expected and gated by the baseline, not a silent regression. Runtime footprint
still follows `ProtocolConfig`: disable transports and mesh features you do not
ship so the engine does not start managers you never use (same guidance as React
Native). [OfflineProtocol.minimalFootprintConfig](../flutter/offline_protocol/lib/src/offline_protocol.dart)
builds a create payload with only the transport flags you enable; pair it with
narrow nested sections for mesh and data features you actually ship.

## What CI runs

The `Flutter Binding` job in `.github/workflows/ci.yml` (path-filtered on pull
requests; full run on pushes to `main`):

- **Dart (fast path):** `flutter test` and `flutter analyze` when the plugin,
  `docs/bridges/flutter.md`, workflow, or `flutter_bridge_guards.rs` change.
- **UniFFI production (`lib.rs`):** any edit runs the native path (jniLibs +
  AAR gate), not the guard-only fast path.
- **Android native (slow path):** arm64 UniFFI build, a check of the
  example's committed Gradle wrapper, then `:offline_protocol:assembleRelease`
  via `measure-plugin-aar-size.sh --check` when the plugin's own Android tree
  or the example's, baselines, Flutter scripts, shared RN Android tree
  (Kotlin, jniLibs, ProGuard consumer rules, Gradle), bindgen, UniFFI ABI
  inputs, or root `Cargo.lock` / workspace `Cargo.toml` change. Changes
  confined to Dart or guard-only tests can skip the NDK/Gradle path; pushes to
  `main` always run the full native gate. `flutter analyze` excludes
  `android/**`, so the plugin's own Kotlin is compiled only by this path.

**iOS native (slow path):** the `Flutter Binding (iOS bridge)` job runs
`bindings/flutter/scripts/ci-typecheck-flutter-ios-bridge.sh` when the plugin
iOS tree, shared React Native Swift, or UniFFI inputs change (full run on
pushes to `main`). The script syncs `BridgeRN/`, then typechecks plugin
`Classes/` plus shared RN Swift against the Flutter engine framework and the
simulator UniFFI archive. Run the same script locally after editing shared Swift
or before relying on a stale `BridgeRN/` copy (F1).
