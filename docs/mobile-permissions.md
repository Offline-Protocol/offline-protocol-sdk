# Mobile permissions and background setup

One reference for **iOS and Android** hosts: React Native, Flutter (same native
bridges), and native apps built on the shared Kotlin/Swift bridge sources.

**What this document is:** what your app must declare, what the SDK already
merges in, when to request runtime permissions, and how background rules affect
mesh reliability.

**What it is not:** bridge contributor contracts (see
[bridges/README.md](bridges/README.md)) or transport tuning (see
[react-native-integration.md](react-native-integration.md#4-enabling-transports)).

---

## Before you call `start()`

Several transports stay degraded for the **whole session** if a grant is missing
when `start()` runs. Fix grants first, or call `enableTransport(...)` after the
user allows access (see [grant timing](#runtime-grant-timing)).

| Transport / feature | Grant or setup before `start()` |
|---------------------|----------------------------------|
| BLE (Android 12+) | `BLUETOOTH_SCAN`, `BLUETOOTH_CONNECT`, `BLUETOOTH_ADVERTISE` |
| BLE (Android 11 and lower) | `ACCESS_FINE_LOCATION` (platform requirement for scan) |
| Wi‑Fi Direct (Android 13+) | `NEARBY_WIFI_DEVICES` |
| Wi‑Fi Direct (Android 12 and lower) | `ACCESS_FINE_LOCATION` |
| LAN peer discovery on same Wi‑Fi (Android, API 37+ app target) | `ACCESS_LOCAL_NETWORK` in **your** manifest + runtime grant |
| iOS LAN / peer-stream on Wi‑Fi | `NSLocalNetworkUsageDescription`, `NSBonjourServices` (first `start()` with Wi‑Fi slot enabled may prompt) |

Internet relay needs no extra runtime permission beyond what your app already
uses for network access. The React Native library merges `INTERNET` on Android;
Flutter apps declare it (see [Flutter](#flutter)).

---

## Choose your checklist

| You enable… | Follow |
|-------------|--------|
| BLE only | [iOS BLE](#ios-bluetooth-and-background) + [Android BLE](#android-bluetooth) |
| BLE + Wi‑Fi Direct (Android) | Above + [Android Wi‑Fi Direct and LAN slot](#android-wi-fi-direct-and-lan-peer-discovery) |
| Wi‑Fi / LAN discovery (iOS) | [iOS local network and Bonjour](#ios-local-network-and-bonjour) |
| Mesh while app backgrounded (Android RN / Flutter) | [Android foreground service](#android-foreground-service-background-mesh) |
| Internet only | `INTERNET` on Android; no extra iOS keys for relay |

---

## What the SDK already merges (Android)

The React Native Android library manifest declares install-time
permissions and its own `MeshForegroundService`. Your app's manifest **merges**
these declarations. You do **not** need to duplicate Wi‑Fi state, multicast, or
SDK service entries unless your tooling strips library manifests.

The SDK declares (among others): legacy Bluetooth (≤ API 30), Android 12+
Bluetooth with `neverForLocation` on scan, Wi‑Fi Direct permissions with
`neverForLocation` on `NEARBY_WIFI_DEVICES`, `INTERNET`,
`ACCESS_NETWORK_STATE`, `CHANGE_WIFI_MULTICAST_STATE`,
`FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_CONNECTED_DEVICE`, and
`WAKE_LOCK` (for opt-in mesh wake only).

Source of truth: `bindings/react-native/android/src/main/AndroidManifest.xml`.

**Flutter is different:** the plugin's manifest declares only the foreground
service and three permissions, so a Flutter app declares the rest itself. See
[Flutter](#flutter) for the exact list.

**Your app still owns:**

- Runtime **requests** for dangerous permissions (Bluetooth, nearby Wi‑Fi,
  location on older API levels).
- The **declaration** of `ACCESS_FINE_LOCATION`. The SDK does not declare it,
  but BLE scanning needs it on API 30 and lower, and Wi‑Fi Direct discovery
  needs it on API 32 and lower. Declare it with
  `android:maxSdkVersion="32"` (see [Android Bluetooth](#android-bluetooth)),
  unless your app uses location for its own features: then omit
  `maxSdkVersion`, or the bound takes location away from those features on
  Android 13 and later.
- Any permission the SDK **deliberately does not** declare (`ACCESS_LOCAL_NETWORK`
  for API 37+ targets; see [Android 17](#android-17-api-37-local-network)).
- Optional feature declarations (`uses-feature`) if you want Play filtering.
- User-visible **strings** on iOS (Info.plist usage descriptions).

---

## iOS

### iOS Bluetooth and background

Add usage descriptions and background modes to your app's `Info.plist`:

```xml
<key>NSBluetoothAlwaysUsageDescription</key>
<string>Bluetooth is used for offline peer-to-peer messaging.</string>

<!-- Optional: iOS reads this key only below iOS 13, the SDK's minimum. -->
<key>NSBluetoothPeripheralUsageDescription</key>
<string>Bluetooth is used for offline peer-to-peer messaging.</string>

<key>UIBackgroundModes</key>
<array>
    <string>bluetooth-central</string>
    <string>bluetooth-peripheral</string>
</array>
```

**Why background modes matter:** without `bluetooth-central` and
`bluetooth-peripheral`, iOS throttles or stops BLE in the background. Symptom:
false `peer_lost` events and missed discovery. This applies to production apps
that expect mesh while backgrounded.

### iOS local network and Bonjour

When the peer-stream transport uses TCP over the local network (including AWDL),
iOS expects:

```xml
<key>NSLocalNetworkUsageDescription</key>
<string>This app discovers nearby devices on your local network for offline messaging.</string>

<key>NSBonjourServices</key>
<array>
    <string>_offlineprotocol._tcp</string>
</array>
```

The system may prompt on first use. Enable the Wi‑Fi / peer-stream slot in config
only when you are ready to show that flow.

### iOS native (Swift package / XCFramework)

The same `Info.plist` rules apply. Native hosts do not get permission prompts from
the JavaScript layer: request nothing extra in Swift unless you add your own
features. Follow [ios-integration.md](ios-integration.md) for storage and lifecycle;
permissions remain as on this page.

---

## Android

### Android Bluetooth

**Manifest (app):** the React Native library already merges the Bluetooth
entries below (Flutter apps declare them; see [Flutter](#flutter)). Location is
the one entry **every** app declares itself, because no SDK manifest does:

```xml
<!-- Android 12 (API 31) and newer (merged by the React Native library) -->
<uses-permission android:name="android.permission.BLUETOOTH_SCAN"
    android:usesPermissionFlags="neverForLocation" />
<uses-permission android:name="android.permission.BLUETOOTH_ADVERTISE" />
<uses-permission android:name="android.permission.BLUETOOTH_CONNECT" />

<!-- Android 11 (API 30) and older (merged by the React Native library) -->
<uses-permission android:name="android.permission.BLUETOOTH" android:maxSdkVersion="30" />
<uses-permission android:name="android.permission.BLUETOOTH_ADMIN" android:maxSdkVersion="30" />

<!-- Your app declares this: BLE scan on API 30 and lower, Wi-Fi Direct
     discovery on API 32 and lower. Drop maxSdkVersion if your app uses
     location for its own features. -->
<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION"
    android:maxSdkVersion="32" />
```

**Runtime:** on API 31+, request the Bluetooth permissions. On API 30 and below,
request `ACCESS_FINE_LOCATION` before BLE scan (platform rule).

The SDK's merged manifest marks `BLUETOOTH_SCAN` with `neverForLocation`, so
from Android 12 (API 31) BLE needs no location grant. Your app should not
request location for BLE on API 31+ unless you have an unrelated location
feature.

### Android Wi-Fi Direct and LAN peer discovery

The React Native library merges Wi‑Fi and nearby-device permissions (Flutter
apps declare them; see [Flutter](#flutter)). **You** request at runtime:

| API level | Request before `start()` when Wi‑Fi Direct is enabled |
|-----------|--------------------------------------------------------|
| 33+ (Android 13+) | `NEARBY_WIFI_DEVICES` |
| 32 and below | `ACCESS_FINE_LOCATION` |

**`neverForLocation` on Wi‑Fi:** the SDK asserts that its Wi‑Fi use is not for
location. If **your** app derives location from Wi‑Fi scans, you must replace
that merged permission with `tools:node="replace"` and carry the grants your
privacy policy requires, including `ACCESS_FINE_LOCATION` on 13+ for discovery.
The SDK checks `NEARBY_WIFI_DEVICES` on 13+, not your location grant.

**LAN (same Wi‑Fi, no Wi‑Fi Direct group):** uses multicast browsing on the
network the device is already on. No extra runtime grant on API 36 and lower.
Guest or office networks that block multicast may show no LAN peers even when
permissions are correct.

### Android 17 (API 37) local network

If **`targetSdkVersion` is 37 or higher**, **your app** must declare and request:

```xml
<uses-permission android:name="android.permission.ACCESS_LOCAL_NETWORK" />
```

Request it before `start()` if LAN discovery is enabled. The SDK **does not**
declare this permission: adding it in the library manifest would change grant
behaviour for apps still targeting API 36 and lower.

If the grant is missing, the LAN carrier stays off and the SDK may emit a
diagnostic warning. See [UPGRADING.md](UPGRADING.md) for the upgrade note on
API 37.

### Android foreground service (background mesh)

React Native and Flutter start `MeshForegroundService` when the protocol runs so
BLE and connected-device work can continue with the screen off. The SDK merges:

- `FOREGROUND_SERVICE`
- `FOREGROUND_SERVICE_CONNECTED_DEVICE`
- A non-exported service with `foregroundServiceType="connectedDevice"`

The service's `connectedDevice` type is declared in the library manifest, so
your app declares no foreground service type of its own. From Android 14
(API 34), a `connectedDevice` service may start only if the app holds one of
the prerequisite permissions Android lists for that type. The SDK satisfies
this with `CHANGE_WIFI_STATE`, an install-time permission in that list declared
by both the React Native and the Flutter manifest, so the service starts even
before the user has answered a Bluetooth prompt. Do not remove it with
`tools:node="remove"`.

The notification is part of normal mesh operation on Android, not an optional
debug feature.

**Optional mesh wake (React Native only):** headless restart after process kill
requires opt-in meta-data `com.offlineprotocol.MESH_WAKE_ENABLED` and a
registered headless task. Flutter does not ship `MeshHeadlessWakeService`.
Details: [react-native-integration.md](react-native-integration.md) and
[bridges/kotlin.md](bridges/kotlin.md).

**Native Android library consumers:** foreground service behaviour matches the
shared bridge when the engine is started from the same module entry points as
React Native. If you embed only the AAR without the RN lifecycle, you own when
the service starts and stops.

---

## Runtime grant timing

```mermaid
flowchart LR
  A[App install] --> B[Declare manifest permissions]
  B --> C[Request runtime grants user approves]
  C --> D["protocol.start()"]
  D --> E[Enable late transport if needed]
```

1. Declare permissions in the app manifest (and iOS usage strings).
2. Request runtime permissions on Android **before** `start()` when the table
   above applies.
3. Call `start()`.
4. If the user granted Wi‑Fi access only after `start()`, call
   `enableTransport('wifiDirect')` once (React Native / TypeScript API).

---

## Flutter

Flutter uses the same iOS and Android bridge sources as React Native. The
`Info.plist` rules above apply unchanged to `ios/Runner`.

The Android manifest is **not** the same. The plugin's manifest
(`bindings/flutter/offline_protocol/android/src/main/AndroidManifest.xml`)
declares only `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_CONNECTED_DEVICE`,
`CHANGE_WIFI_STATE` and the `MeshForegroundService` entry. Declare everything
else in `android/app/src/main/AndroidManifest.xml`. The bounds match the React
Native library manifest, plus the location entry every app owns:

```xml
<!-- Bluetooth LE, Android 11 (API 30) and older -->
<uses-permission android:name="android.permission.BLUETOOTH" android:maxSdkVersion="30" />
<uses-permission android:name="android.permission.BLUETOOTH_ADMIN" android:maxSdkVersion="30" />

<!-- Bluetooth LE, Android 12 (API 31) and newer -->
<uses-permission android:name="android.permission.BLUETOOTH_SCAN"
    android:usesPermissionFlags="neverForLocation" />
<uses-permission android:name="android.permission.BLUETOOTH_ADVERTISE" />
<uses-permission android:name="android.permission.BLUETOOTH_CONNECT" />

<!-- BLE scan on API 30 and lower, Wi-Fi Direct discovery on API 32 and lower.
     Drop maxSdkVersion if your app uses location for its own features. -->
<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION"
    android:maxSdkVersion="32" />

<!-- Wi-Fi Direct (CHANGE_WIFI_STATE comes from the plugin) -->
<uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
<uses-permission android:name="android.permission.NEARBY_WIFI_DEVICES"
    android:usesPermissionFlags="neverForLocation" />

<!-- LAN carrier on the same Wi-Fi network -->
<uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
<uses-permission android:name="android.permission.CHANGE_WIFI_MULTICAST_STATE" />

<!-- Internet relay, and the peer-stream sockets -->
<uses-permission android:name="android.permission.INTERNET" />

<uses-feature android:name="android.hardware.bluetooth_le" android:required="false" />
```

`flutter create` puts `INTERNET` only in the debug and profile manifests, so a
release build without the line above has no internet relay. Add
`ACCESS_LOCAL_NETWORK` as well if you target API 37+ (see
[Android 17](#android-17-api-37-local-network)). The plugin does not prompt for
anything: request the runtime grants in
[Before you call `start()`](#before-you-call-start) from your app.

Platform setup in the plugin README points here:
[bindings/flutter/README.md](../bindings/flutter/README.md#platform-setup).

After changing shared iOS bridge files, run the plugin's `sync_bridge_rn.sh`
before `pod install` (Flutter-specific; unrelated to permissions content).

---

## Native-only Android / iOS

| Path | Permissions doc | Integration |
|------|-----------------|-------------|
| Swift package / XCFramework | This page (Info.plist) | [ios-integration.md](ios-integration.md) |
| Android library AAR | This page | [android-integration.md](android-integration.md) |

Native paths must still call `initializeMls` and `setEventCallback` before
`start()`; permissions do not replace that lifecycle.

---

## Verify before release

| Check | Pass criteria |
|-------|----------------|
| Cold start on physical devices | Peers discover on BLE when both apps granted and started |
| Wi‑Fi Direct (if enabled) | Group forms within documented time budget (often ~1–2 min); BLE still works meanwhile |
| Background (Android) | Foreground notification appears when mesh is running; no immediate `RemoteServiceException` on start |
| Background (iOS) | With background modes, peers do not drop solely because app moved to background |
| API 37 target (if applicable) | LAN discovery works only after `ACCESS_LOCAL_NETWORK` granted |

Simulators do not validate BLE or Wi‑Fi Direct behaviour. Use hardware for mesh QA.

---

## Related docs

| Document | Role |
|----------|------|
| [Integrator map](integrator-map.md) | Platform entry and lifecycle |
| [react-native-integration §4](react-native-integration.md#4-enabling-transports) | Transport configuration |
| [configuration.md](configuration.md) | Which transports to enable |
| [UPGRADING.md](UPGRADING.md) | Version-specific behaviour (API 37, iOS peer-stream changes) |

---

## React Native helper (optional)

`@offline-protocol/mesh-sdk` exports:

| API | Role |
|-----|------|
| `requestMeshPermissions(options?)` | Request Android dangerous grants before `start()` |
| `requestMeshPermissionsForConfig(config)` | Same, with options derived from `transports` |
| `meshPermissionOptionsFromConfig(config)` | Build options without prompting |

```typescript
import { OfflineProtocol, requestMeshPermissionsForConfig } from '@offline-protocol/mesh-sdk';

const config = { appId: 'my-app', profile: 'default', transports: { ble: { enabled: true } } };

const permissions = await requestMeshPermissionsForConfig(config);
if (!permissions.granted) {
  // A grant was refused: the transport that needs it stays off. Once the user
  // allows it, call enableTransport(...) for that transport after start().
}

const protocol = new OfflineProtocol(config);
await protocol.start();
```

**iOS:** the helper does not show dialogs. Add Info.plist keys from this guide;
Bluetooth and local network prompts appear when mesh starts.

**Android:** the helper requests only permissions that are not already granted.
It does not add manifest entries or replace `ACCESS_LOCAL_NETWORK` in the app
manifest for API 37+ targets.

This document remains the contract for what must be granted; the helpers mirror
the native permission checks in the React Native Android bridge.
