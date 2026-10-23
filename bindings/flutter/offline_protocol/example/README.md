# Offline Protocol Flutter example

Minimal app: boot the engine, listen for core events, print this device’s
`off1…` address.

Build native libraries first ([bindings/flutter/README.md](../../README.md)).

## Android

```bash
flutter pub get
flutter run -d android
```

## iOS

The Xcode `Runner` project is not committed. Generate it once:

```bash
../../scripts/bootstrap-example-platforms.sh
flutter pub get
flutter run -d <device-or-simulator-id>
```

After bootstrap, merge Bluetooth `Info.plist` keys from
[React Native iOS permissions](../../../react-native/README.md#ios-permissions)
if you enable BLE in your own config (the example smoke config does not start BLE).

For iOS pods and the `BridgeRN` copy step, see
[bindings/flutter/README.md](../../README.md).
