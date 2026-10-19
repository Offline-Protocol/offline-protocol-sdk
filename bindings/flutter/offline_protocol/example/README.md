# Offline Protocol Flutter example

## Android (in-repo)

From this directory:

```bash
flutter pub get
flutter run -d android
```

Requires native libraries built first (see [bindings/flutter/README.md](../../README.md)).

## iOS

The example does not commit an Xcode `Runner` project (Flutter SDK versions differ).
Generate platform files once:

```bash
../../scripts/bootstrap-example-platforms.sh
flutter run -d ios
```

Or manually:

```bash
flutter pub get
flutter create . --platforms=ios,android --org com.offlineprotocol.flutter
flutter run -d ios
```

## What it does

Boots the engine with all transports disabled, subscribes to core events,
initializes MLS with platform secure storage, starts the core, and prints this
device's `off1…` address.

Generated plugin registrant files and `.flutter-plugins-dependencies` are not
committed; run `flutter pub get` (and `bootstrap-example-platforms.sh` for iOS)
locally after clone.
