# Offline Protocol Flutter example

Minimal app: start the engine, listen for events, print this device’s `off1…` address.

For native library setup, see [bindings/flutter/README.md](../../README.md).

## Android

```bash
flutter pub get
flutter run -d android
```

## iOS

Generate the Xcode `Runner` project once (not committed):

```bash
../../scripts/bootstrap-example-platforms.sh
flutter pub get
flutter run -d <device-or-simulator-id>
```

To enable BLE in your own config, add the Bluetooth keys from [mobile-permissions.md](../../../../docs/mobile-permissions.md). The example smoke config does not start BLE.
