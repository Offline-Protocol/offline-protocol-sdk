# Quick Start Guide

Get started with the Offline Protocol SDK in 5 minutes.

> **Want to see a complete working example?** Check out the [React Native Example App](examples/react-native-app/README.md) that demonstrates all SDK features.

## For React Native Developers

### 1. Install

```bash
npm install @offline-protocol/mesh-sdk
cd ios && pod install  # iOS only
```

### 2. Initialize

```typescript
import { OfflineProtocol, MessagePriority } from '@offline-protocol/mesh-sdk';

const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'current-user-id',
});

await protocol.start();
```

### 3. Send Messages

```typescript
const messageId = await protocol.sendMessage({
  recipient: 'friend-user-id',
  content: 'Hello!',
  priority: MessagePriority.Medium,
});
```

### 4. Receive Messages

```typescript
protocol.on('message_received', (event) => {
  console.log(`From ${event.sender}: ${event.content}`);
  // Update your UI here
});
```

### 5. Monitor Status

```typescript
protocol.on('transport_switched', (event) => {
  console.log(`Now using ${event.to}`);
});

protocol.on('relay_promoted', () => {
  console.log('This device is now a relay!');
});
```

### 6. Report the Battery

No transport can observe the host's battery, so the SDK only learns it from
you. Until it does, the relay role is never evaluated — `relay_promoted` and
`relay_demoted` never fire — DORS energy scoring skips its battery term, and
the floor that stops a dying phone carrying other people's traffic never
applies.

```typescript
// On start, and again on each platform battery notification.
await protocol.setBatteryState(level, isCharging);
```

Report `isCharging` where the platform provides it: a charging device is
deliberately excused the soft `minBatteryForRelay` floor, so sending the level
alone strips relay duty from plugged-in devices that should keep it.

**That's it!** Your app now works offline with automatic transport switching.

---

## For Android Developers

> Native (no React Native) integration needs the full `ProtocolConfig`, permission
> setup, and a call to `initializeMls(secureStorage, protocolStateStorage)` with two
> storage providers you supply — there is no auto-initialization on the native path,
> and encryption is fail-closed, so sends fail until MLS is initialized. See the
> [Android Integration Guide](docs/android-integration.md) for the complete
> walkthrough — the snippet below is just the shape.

### 1. Build Rust Library

```bash
cargo build --release --target aarch64-linux-android
```

This produces `liboffline_protocol_uniffi.so`.

### 2. Add to Android Project

Copy it into `app/src/main/jniLibs/arm64-v8a/` as `libuniffi_offline_protocol.so` (the name
UniFFI's loader expects). The `scripts/build-uniffi-android.sh` helper builds every ABI and
renames automatically.

### 3. Use in Kotlin

```kotlin
import uniffi.offline_protocol.*

// Build `config` with the full ProtocolConfig(...) — see the Android integration guide.
val protocol = OfflineProtocol(config)
protocol.start()

val messageId = protocol.sendMessage(
    recipient = "friend",
    content = "Hello!",
    priority = MessagePriority.HIGH,
    replyToMsg = null,
)
```

---

## For iOS Developers

> Native (no React Native) integration needs the full `ProtocolConfig`, permission
> setup, and a call to `initializeMls(secureStorage:protocolStateStorage:)` with two
> storage providers you supply — there is no auto-initialization on the native path,
> and encryption is fail-closed, so sends fail until MLS is initialized. See the
> [iOS Integration Guide](docs/ios-integration.md) for the complete
> walkthrough — the snippet below is just the shape.

### 1. Build Rust Library

```bash
cargo build --release --target aarch64-apple-ios
```

This produces `liboffline_protocol_uniffi.a`.

### 2. Add to Xcode

Add `liboffline_protocol_uniffi.a` (or the device/simulator slices from
`scripts/build-uniffi-ios.sh`) and the generated `offline_protocol.swift` to your project.

### 3. Use in Swift

```swift
// Build `config` with the full ProtocolConfig(...) — see the iOS integration guide.
let mesh = try OfflineProtocol(config: config)
try mesh.start()

let messageId = try mesh.sendMessage(
    recipient: "friend",
    content: "Hello!",
    priority: .high,
    replyToMsg: nil
)
```

---

## Next Steps

- **[Upgrading](docs/UPGRADING.md)** - Read this first if you are moving an existing app onto the storage-split release
- **[React Native Example App](examples/react-native-app/README.md)** - Complete working example
- [React Native Integration Guide](docs/react-native-integration.md) - Full SDK integration walkthrough
- [Integration Guide](examples/react-native-app/INTEGRATION_GUIDE.md) - Step-by-step project setup
- [API Reference](docs/api-reference.md)
- [Configuration Guide](docs/configuration.md)
- [Reticulum Transport](docs/reticulum.md)
- [Nostr Transport](docs/nostr.md)
- [Architecture Overview](docs/architecture.md)
- [All Documentation](docs/)

## Common Issues

### React Native: "Module not found"

Make sure you've run:
- iOS: `cd ios && pod install`
- Android: Rebuild the app

### Android: "Library not found"

Ensure `libuniffi_offline_protocol.so` is in the correct `jniLibs` folder for your architecture.

### iOS: "Undefined symbols"

Link against the Rust static library in Xcode Build Settings.

## Getting Help

- [React Native Example App](examples/react-native-app/README.md) - See complete implementation
- [React Native Integration Guide](docs/react-native-integration.md) - Full SDK integration walkthrough
- [Integration Guide](examples/react-native-app/INTEGRATION_GUIDE.md) - Step-by-step project setup
- [GitHub Issues](https://github.com/Offline-Protocol/offline-protocol-sdk/issues)
- [All Documentation](docs/)
