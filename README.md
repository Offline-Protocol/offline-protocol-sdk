# Offline Protocol SDK

> Offline-first messaging protocol with intelligent multi-transport switching and mesh networking

## Quick Start

### For React Native Apps:

```bash
npm install @offlineprotocol/react-native
```

```typescript
import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react-native';

const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
  bleEnabled: true,
  preferOnline: false,
});

await protocol.start();
const messageId = await protocol.sendMessage(
  'recipient456',
  'Hello!',
  MessagePriority.Medium
);
```


## Building the SDK

### Prerequisites:
- Rust (via rustup)
- uniffi-bindgen: `cargo install uniffi --features="cli"`
- ndk: `cargo install cargo-ndk`

### Build UniFFI Libraries:

```bash
cd bindings/react-native

# Build for all platforms
npm run build:uniffi:all

# Or build individually
npm run build:uniffi:ios      # iOS only
npm run build:uniffi:android  # Android only

# Regenerate bindings after UDL changes
npm run generate:bindings
```


## Architecture

The SDK consists of modular Rust crates:

- **offline-protocol-core** - Core types and data structures
- **offline-protocol-transport** - Multi-transport abstraction (BLE, WiFi, Internet)
- **offline-protocol-router** - DORS routing and relay management
- **offline-protocol-reliability** - ACKs, retries, deduplication
- **offline-protocol** - Main protocol engine
- **offline-protocol-uniffi** - UniFFI bindings for Swift/Kotlin (NEW!)

## Development

### Running Tests:
```bash
cargo test --workspace
```

### Building:
```bash
cargo build --workspace --release
```


---
