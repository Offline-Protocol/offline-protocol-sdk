# offline_protocol

Flutter plugin for the [Offline Protocol SDK](https://github.com/Offline-Protocol/offline-protocol-sdk):
offline-first mesh messaging over Bluetooth LE, Wi-Fi Direct and the internet,
with automatic MLS end-to-end encryption (RFC 9420).

The package ships prebuilt native libraries for Android and iOS, so an app
needs no Rust toolchain.

```bash
flutter pub add offline_protocol
```

Declare the platform permissions your transports need:
[mobile permissions](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/mobile-permissions.md).

## Quick start

Subscribe to events before `createFromConfig` if you need every event or gap
recovery through native replay.

```dart
import 'package:offline_protocol/offline_protocol.dart';

final sub = OfflineProtocol.events.listen((e) {
  final type = e.decodeEvent()['type'];
});

await OfflineProtocol.createFromConfig({
  'appLabel': 'com.example.app',
  'bleEnabled': true,
  'internetEnabled': true,
});
await OfflineProtocol.initializeMlsWithSecureStorage();
await OfflineProtocol.start();

final messageId = await OfflineProtocol.sendMessage(
  recipient: 'off1...',
  content: 'Hello',
);
```

## Documentation

- [Flutter distribution](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/flutter-distribution.md)
- [Flutter binding guide](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/bindings/flutter/README.md)
- [Upgrading](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/UPGRADING.md)

## License

AGPL-3.0-only, with a commercial license available. See `LICENSE` and
`LICENSE-COMMERCIAL.md`.
