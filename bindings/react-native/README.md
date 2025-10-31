# @offlineprotocol/react-native

React Native bindings for the Offline Protocol SDK - enabling offline-first messaging with automatic transport switching.

## Features

- 🔄 **Automatic Transport Switching**: DORS intelligently switches between Internet, BLE Mesh, and Wi-Fi Direct
- 📡 **Offline-First**: Messages delivered even without internet connectivity
- 🔐 **Reliable Delivery**: ACK-based reliability with exponential backoff retry
- ⚡ **High Performance**: Rust core provides native-level performance
- 🌐 **Cross-Platform**: Works on both iOS and Android

## Installation

```bash
npm install @offlineprotocol/react-native
# or
yarn add @offlineprotocol/react-native
```

### iOS

```bash
cd ios && pod install
```

### Android

No additional steps required.

## Usage

### Basic Setup

```typescript
import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react-native';

// Initialize the protocol
const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
  transport: {
    bleEnabled: true,
    wifiDirectEnabled: true,  // Android only
    internetEnabled: true,
  },
  dors: {
    preferOnline: false,  // false = offline-first, true = online-first
  },
});

// Start the protocol
await protocol.start();
```

### Sending Messages

```typescript
// Send a simple message
const messageId = await protocol.sendMessage({
  recipient: 'user456',
  content: 'Hello from offline!',
  priority: MessagePriority.High,
});

console.log(`Message sent with ID: ${messageId}`);
```

### Receiving Messages

```typescript
protocol.on('message:received', (event) => {
  console.log(`Received message from ${event.sender}: ${event.content}`);
  console.log(`Delivered via ${event.transport} with ${event.hopCount} hops`);
});
```

### Monitoring Network Status

```typescript
// Transport switching
protocol.on('transport:switched', (event) => {
  console.log(`Switched from ${event.from} to ${event.to}`);
  console.log(`Reason: ${event.reason}`);
});

// Relay status
protocol.on('relay:promoted', (event) => {
  console.log(`Became relay with ${event.connectionCount} connections`);
});

protocol.on('relay:demoted', (event) => {
  console.log(`Demoted from relay: ${event.reason}`);
});

// Network metrics
protocol.on('network:metrics', (event) => {
  console.log(`Neighbors: ${event.neighborCount}`);
  console.log(`Delivery ratio: ${event.deliveryRatio * 100}%`);
});
```

### File Transfer

```typescript
// Send a file
const fileId = await protocol.sendFile({
  recipient: 'user456',
  filePath: '/path/to/file.jpg',
  priority: MessagePriority.Medium,
});

// Monitor progress
protocol.on('file:progress', (event) => {
  console.log(`Upload progress: ${event.percentage}%`);
  console.log(`Chunks: ${event.chunksSent}/${event.totalChunks}`);
});

// Receive file
protocol.on('file:received', (event) => {
  console.log(`Received file: ${event.fileName} (${event.fileSize} bytes)`);
});
```

### Lifecycle Management

```typescript
// When app goes to background
AppState.addEventListener('change', (nextAppState) => {
  if (nextAppState === 'background') {
    await protocol.pause(); // Reduces battery usage
  } else if (nextAppState === 'active') {
    await protocol.resume(); // Resume full operations
  }
});

// When app closes
await protocol.stop();
```

## Configuration Options

See `types.ts` for complete TypeScript definitions.

### Transport Configuration

- `bleEnabled`: Enable Bluetooth Low Energy mesh (default: true)
- `wifiDirectEnabled`: Enable Wi-Fi Direct (Android only, default: true)
- `internetEnabled`: Enable Internet transport (default: true)

### DORS Configuration

- `preferOnline`: Prefer Internet when available (default: false)
- `switchHysteresis`: Prevent rapid switching (default: 15.0)
- `switchCooldownSecs`: Wait time after switch (default: 20)

### Relay Configuration

- `allowRelay`: Allow device to act as relay (default: true)
- `minBatteryForRelay`: Min battery % to relay (default: 30)
- `relayThreshold`: Min connections to become relay (default: 3)

## Platform Support

- ✅ Android: Full support (BLE + Wi-Fi Direct + Internet)
- ✅ iOS: BLE + Internet (Wi-Fi Direct not available)
- ⚠️ Permissions required:
  - Android: Bluetooth, Location, Nearby Devices
  - iOS: Bluetooth

## Architecture

```
React Native App (JavaScript/TypeScript)
    ↓
React Native Bridge (@offlineprotocol/react-native)
    ↓
Native Modules (Kotlin/Swift)
    ↓
Rust FFI Layer (C bindings)
    ↓
Rust Core (DORS + Routing + Reliability)
```

The Rust core ensures high performance and memory safety, with all complex protocol logic written once and shared across platforms.

## License

MIT OR Apache-2.0

