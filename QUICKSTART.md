# Quick Start Guide

Get started with the Offline Protocol SDK in 5 minutes.

## For React Native Developers

### 1. Install

```bash
npm install @offlineprotocol/react-native
cd ios && pod install  # iOS only
```

### 2. Initialize

```typescript
import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react-native';

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
protocol.on('message:received', (event) => {
  console.log(`From ${event.sender}: ${event.content}`);
  // Update your UI here
});
```

### 5. Monitor Status

```typescript
protocol.on('transport:switched', (event) => {
  console.log(`Now using ${event.to}`);
});

protocol.on('relay:promoted', () => {
  console.log('This device is now a relay!');
});
```

**That's it!** Your app now works offline with automatic transport switching.

---

## For Web Developers

### 1. Install

```bash
npm install @offlineprotocol/web
```

### 2. Use

```javascript
import init, { OfflineProtocol } from '@offlineprotocol/web';

// Initialize WASM
await init();

// Create and start
const protocol = new OfflineProtocol(JSON.stringify({
  appId: 'web-app',
  userId: 'user123',
}));

await protocol.start();

// Send
await protocol.sendMessage('recipient', 'Hello from web!', 1);
```

---

## For Android Developers

### 1. Build Rust Library

```bash
cargo build --release --target aarch64-linux-android
```

### 2. Add to Android Project

Copy `liboffline_protocol.so` to `app/src/main/jniLibs/arm64-v8a/`

### 3. Use in Kotlin

```kotlin
val protocol = OfflineProtocol(ProtocolConfig(
    appId = "my-app",
    userId = "user123"
))

protocol.start()

val messageId = protocol.sendMessage(
    recipient = "friend",
    content = "Hello!",
    priority = MessagePriority.HIGH
)
```

---

## For iOS Developers

### 1. Build Rust Library

```bash
cargo build --release --target aarch64-apple-ios
```

### 2. Add to Xcode

Add `liboffline_protocol.a` to your project frameworks.

### 3. Use in Swift

```swift
let config = ProtocolConfig(
    appId: "my-app",
    userId: "user123"
)

let protocol = try OfflineProtocol(config: config)
try protocol.start()

let messageId = try protocol.sendMessage(
    recipient: "friend",
    content: "Hello!",
    priority: .high
)
```

---

## Next Steps

- [Complete API Reference](docs/api-reference.md)
- [Configuration Guide](docs/configuration.md)
- [Architecture Overview](docs/architecture.md)
- [Platform Integration Guides](docs/)

## Common Issues

### React Native: "Module not found"

Make sure you've run:
- iOS: `cd ios && pod install`
- Android: Rebuild the app

### Android: "Library not found"

Ensure `liboffline_protocol.so` is in correct `jniLibs` folder for your architecture.

### iOS: "Undefined symbols"

Link against the Rust static library in Xcode Build Settings.

### Web: WASM not loading

Make sure to call `await init()` before creating the protocol.

## Getting Help

- [GitHub Issues](https://github.com/offline-protocol/sdk/issues)
- [Documentation](docs/)
- [Examples](bindings/react-native/example-usage.tsx)

