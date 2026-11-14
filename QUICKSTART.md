# Quick Start Guide

Get started with the Offline Protocol SDK in 5 minutes.

> **Not sure this is the right entry point?** See the
> [integrator map](docs/integrator-map.md) for every platform (Flutter, Python,
> Web, native iOS/Android, C#, and more) and when to read [UPGRADING](docs/UPGRADING.md).

> **Want to see a complete working example?** Check out the [React Native Example App](examples/react-native-app/README.md) that demonstrates all SDK features.

## For React Native Developers

### 1. Install

> **Expo Go does not work** with this SDK (native module). Use a development
> build (`expo-dev-client`), a native build (`npx expo run:ios`) or bare React
> Native. See [react-native-host-doctor.md](docs/react-native-host-doctor.md).

```bash
npm install @offline-protocol/mesh-sdk
npx --no-install mesh-sdk-doctor   # catch Podfile / Expo mistakes early
cd ios && pod install  # iOS only
```

Before `start()` on a real device, add iOS and Android permissions from
[mobile-permissions.md](docs/mobile-permissions.md). For `recipient` and peer
ids, use `off1…` addresses, not `profile`: [identity-for-integrators.md](docs/identity-for-integrators.md).

### 2. Initialize

```typescript
import { OfflineProtocol, MessagePriority } from '@offline-protocol/mesh-sdk';

const protocol = new OfflineProtocol({
  appId: 'my-app',
  profile: 'default',
});

await protocol.start();
```

For another transport mix (Wi-Fi Direct, an internet relay, Nostr), build the
config with [`forScenario`](docs/react-native-config-presets.md).

### 3. Send Messages

```typescript
const messageId = await protocol.sendMessage({
  recipient: peerAddress,  // the peer's off1… address; a username reaches nobody
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

// Fires once this device has actually been carrying traffic for other
// devices — it reports what happened, not what the device is capable of.
protocol.on('relay_promoted', () => {
  console.log('This device is now relaying for the mesh!');
});
```

### 6. Report the Battery

No transport can observe the host's battery, so the SDK only learns it from
you. Until it does, DORS energy scoring skips its battery term, the floor that
stops a dying phone carrying other people's traffic never applies, and mesh
forwarding runs at full effort on every device — a healthy laptop and a phone
at 20% carry an equal share, rather than the laptop carrying more.

(The relay events are the exception: they report traffic this device has
actually carried, so they fire with or without a battery reading.)

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
cargo build --profile mobile --target aarch64-linux-android
```

This produces `liboffline_protocol_uniffi.so`.

### 2. Add to Android Project

Copy it into `app/src/main/jniLibs/arm64-v8a/` as `libuniffi_offline_protocol.so` (the name
UniFFI's loader expects). The `bindings/react-native/scripts/build-uniffi-android.sh` helper builds every ABI and
renames automatically.

### 3. Use in Kotlin

```kotlin
import uniffi.offline_protocol.*

// Build `config` with the full ProtocolConfig(...) — see the Android integration guide.
val protocol = OfflineProtocol(config)
protocol.start()

val messageId = protocol.sendMessage(
    recipient = peerAddress, // the peer's off1… address
    content = "Hello!",
    priority = MessagePriority.HIGH,
    replyToMsg = null,
)
```

---

## For iOS Developers (native Swift)

Native apps should install the **release Swift package** `OfflineProtocolSDK`, not
build Rust by hand. Start with
[ios-native-distribution.md](docs/ios-native-distribution.md), then
[ios-integration.md](docs/ios-integration.md) for transports and plist setup.

### 1. Add the package

In Xcode: **File → Add Package Dependencies…** →
`https://github.com/Offline-Protocol/offline-protocol-swift.git`

### 2. Initialize MLS (built-in stores)

```swift
import OfflineProtocolSDK

try OfflineProtocolReferenceStorage.initializeMls(on: mesh, appId: "my-app", profile: "default")
try mesh.start()
```

### 3. Send a message

```swift
let messageId = try mesh.sendMessage(
    recipient: peerAddress, // the peer's off1… address
    content: "Hello!",
    priority: .high,
    replyToMsg: nil
)
```

Custom storage or an XCFramework built from source: see the integration and
distribution guides above; that path is for maintainers, not the default app flow.

---

## For Python (desktop / gateway)

```bash
pip install 'offline-protocol-sdk[lan,http]'
```

Supported wheels and fallbacks (Alpine, Intel Mac, old glibc): [python-distribution.md](docs/python-distribution.md).

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
