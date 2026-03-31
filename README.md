# Offline Protocol SDK

> Offline-first messaging protocol with intelligent multi-transport switching, mesh networking, and automatic end-to-end encryption

## Features

- **Multi-Transport**: Seamlessly switches between BLE, WiFi Direct, Internet, and Reticulum
- **Mesh Networking**: Automatic peer discovery and message relay
- **End-to-End Encryption**: Automatic MLS encryption with forward secrecy (RFC 9420)
- **Group Roles**: Admin/member role management with last-admin safety invariants
- **DORS**: Dynamic Offline Relay Switch for optimal transport selection
- **Reliability**: ACKs, retries, and deduplication built-in

## Quick Start

### For React Native Apps:

```bash
npm install @offline-protocol/mesh-sdk
```

```typescript
import { OfflineProtocol, MessagePriority } from '@offline-protocol/mesh-sdk';

const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
  // Encryption is enabled by default!
});

await protocol.start();

// Initialize MLS encryption (required once)
await protocol.initializeMlsWithSecureStorage();

// Messages are automatically encrypted when possible!
const messageId = await protocol.sendMessage({
  recipient: 'recipient456',
  content: 'Hello!',  // Automatically encrypted
  priority: MessagePriority.Medium,
});
```

### Encryption Configuration

```typescript
const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
  encryption: {
    enabled: true,           // Auto-encrypt (default)
    autoKeyExchange: true,   // Exchange keys on peer discovery (default)
    storePending: true,      // Queue messages until session ready (default)
  },
});
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
- **offline-protocol-transport** - Multi-transport abstraction (BLE, WiFi, Internet, Reticulum)
- **offline-protocol-router** - DORS routing and relay management
- **offline-protocol-reliability** - ACKs, retries, deduplication
- **offline-protocol-mls** - End-to-end encryption using MLS (RFC 9420)
- **offline-protocol-services** - Service discovery and request/response over mesh
- **offline-protocol** - Main protocol engine with auto-encryption
- **offline-protocol-uniffi** - UniFFI bindings for Swift/Kotlin

## DORS: Dynamic Offline Relay Switch

DORS automatically selects and switches between Internet, BLE Mesh, Wi-Fi Direct, and Reticulum based on real-time network conditions. It scores each transport on signal strength, proximity, bandwidth, congestion, energy efficiency, reliability, and available capacity, then applies hysteresis, cooldown, and stability checks to prevent flapping.

For details, see the [DORS Deep Dive](docs/dors.md) and [DORS Configuration Guide](docs/dors-configuration.md).

## Mesh Networking

The SDK implements a cluster-based, self-organizing mesh network. Devices discover peers via BLE advertisements, form clusters with scored peer connections, and bridge separate clusters automatically. Messages route through the mesh using gossip-based forwarding with TTL-based expiration and gradient routing when routes are known.

For details, see the [Mesh Networking Guide](docs/mesh.md).

## Documentation

See the [docs/](docs/) directory for detailed guides:

- [Architecture Deep Dive](docs/architecture.md)
- [API Reference](docs/api-reference.md)
- [Configuration Guide](docs/configuration.md)
- [DORS Deep Dive](docs/dors.md) / [DORS Configuration](docs/dors-configuration.md)
- [Mesh Networking Guide](docs/mesh.md)
- [MLS Encryption Integration](docs/mls-integration.md)
- [Transport Architecture](docs/transport-architecture.md)
- [Service Discovery](docs/service-discovery.md)
- [React Native Integration](docs/react-native-integration.md)
- [Reticulum Transport](docs/reticulum.md)
- [iOS Integration](docs/ios-integration.md) / [Android Integration](docs/android-integration.md)

## Development

```bash
cargo build --workspace            # Build
cargo test --workspace             # Test
cargo clippy --workspace -- -D warnings  # Lint
cargo fmt --workspace              # Format
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for development guidelines and [QUICKSTART.md](QUICKSTART.md) for platform-specific setup.
