# Offline Protocol SDK

> Offline-first messaging protocol with intelligent multi-transport switching, mesh networking, and automatic end-to-end encryption

[![CI](https://github.com/Offline-Protocol/offline-protocol-sdk/actions/workflows/ci.yml/badge.svg)](https://github.com/Offline-Protocol/offline-protocol-sdk/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/@offline-protocol/mesh-sdk.svg?logo=npm)](https://www.npmjs.com/package/@offline-protocol/mesh-sdk)
[![License](https://img.shields.io/badge/license-AGPL--3.0--only%20or%20Commercial-blue.svg)](#license)
[![Platforms](https://img.shields.io/badge/platforms-iOS%20%7C%20Android%20%7C%20macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)](#building-the-sdk)

**Dual-licensed:** use it under [AGPL-3.0-only](LICENSE), or buy a [commercial license](LICENSE-COMMERCIAL.md), your call. The AGPL requires you to publish your source whenever you distribute the SDK or expose it over a network (section 13); the commercial license lifts that requirement for closed-source mobile apps, embedded firmware, and SaaS deployments. See the [License](#license) section for the full breakdown.

## Features

- **Multi-Transport**: Automatically switches between BLE, WiFi Direct, Internet, Reticulum, and Nostr relays
- **Mesh Networking**: Automatic peer discovery and message relay
- **End-to-End Encryption**: Automatic MLS encryption with forward secrecy (RFC 9420)
- **Group Roles**: Admin/member role management with last-admin safety invariants
- **DORS**: Dynamic Offline Relay Switch for optimal transport selection
- **Reliability**: ACKs, retries, and deduplication built-in
- **Cross-Platform Bindings**: React Native for iOS/Android and Python for macOS/Linux/Windows

> **What you must implement:** the Rust crates are I/O-free protocol engines — they queue, route, encrypt, and select transports, but never open a socket or touch a radio. A *platform bridge* does the actual I/O: it drains each transport's outbound queue, performs the send, reports the outcome, and injects inbound bytes. The React Native binding ships these bridges for iOS and Android (BLE, WiFi Direct, Internet, Nostr, Reticulum), and the Python binding ships BLE and Internet bridges. If you consume the Rust crates directly via `cargo add`, you write the bridge yourself — the contract is documented in the [`offline-protocol-transport`](crates/offline-protocol-transport/src/lib.rs) crate docs.

## Quick Start

### For React Native Apps

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

// Messages are automatically encrypted!
const messageId = await protocol.sendMessage({
  recipient: 'recipient456',
  content: 'Hello!',  // Automatically encrypted
  priority: MessagePriority.Medium,
});
```

### For Python Desktop Apps

The Python binding supports macOS, Linux, and Windows. Build and install it
from the repository:

```bash
cd bindings/python
bash scripts/build-desktop.sh
pip install -e .
```

```python
from offline_protocol_sdk import ProtocolManager
from offline_protocol_sdk.offline_protocol import ProtocolConfig, OverflowPolicy

config = ProtocolConfig(
    app_id="my-app",
    user_id="user123",
    ble_enabled=False,
    wifi_direct_enabled=False,
    internet_enabled=True,
    reticulum_enabled=False,
    nostr_enabled=False,
    prefer_online=True,
    initial_ttl=8,
    encryption_enabled=True,
    auto_key_exchange=True,
    store_pending=True,
    require_encryption=False,
    max_pending_per_peer=64,
    max_pending_global=4096,
    pending_ttl_ms=120_000,
    overflow_policy=OverflowPolicy.DROP_OLDEST,
)
protocol = ProtocolManager(config)
```

See the [Python binding guide](bindings/python/README.md) for transport setup,
secure storage, and complete lifecycle examples.

### Encryption Configuration

Encryption is **fail-closed by default**: if a message cannot be encrypted
(e.g. MLS was never initialized), the send fails with a typed error instead of
silently falling back to plaintext. Messages to peers whose secure session is
still being established are queued and delivered encrypted once it is ready.

```typescript
const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
  encryption: {
    enabled: true,           // Auto-encrypt (default)
    autoKeyExchange: true,   // Exchange keys on peer discovery (default)
    storePending: true,      // Queue messages until session ready (default)
    requireEncryption: true, // Fail closed, never silent plaintext (default)
  },
});
```

To deliberately operate in plaintext (e.g. an open-broadcast mesh with no
provisioned key storage), opt out explicitly with `requireEncryption: false` —
each plaintext send then emits a `security_warning` event with the
`PLAINTEXT_SEND` reason code (once per peer).


## Building the SDK

### Prerequisites
- Rust (via rustup)
- uniffi-bindgen: `cargo install uniffi --version 0.30.0 --features cli --locked` (must match the workspace `uniffi = "0.30"` pin)
- For Android: the Android NDK (set `ANDROID_NDK_HOME`); for iOS: Xcode

### Build Mobile UniFFI Libraries

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

### Build Python Desktop Bindings

```bash
cd bindings/python
bash scripts/build-desktop.sh
```

The desktop build produces the native `.dylib`, `.so`, or `.dll` for the host
platform and regenerates the Python UniFFI module.


## Architecture

The SDK consists of modular Rust crates:

- **offline-protocol-core** - Core types and data structures
- **offline-protocol-transport** - Multi-transport abstraction (BLE, WiFi, Internet, Reticulum, Nostr)
- **offline-protocol-router** - DORS routing and relay management
- **offline-protocol-reliability** - ACKs, retries, deduplication
- **offline-protocol-mls** - End-to-end encryption using MLS (RFC 9420)
- **offline-protocol-services** - Service discovery and request/response over mesh
- **offline-protocol** - Main protocol engine with auto-encryption
- **offline-protocol-uniffi** - UniFFI bindings for Swift/Kotlin

## DORS: Dynamic Offline Relay Switch

DORS automatically selects and switches between Internet, BLE Mesh, Wi-Fi Direct, Reticulum, and Nostr based on real-time network conditions. It scores each transport on signal strength, proximity, bandwidth, congestion, energy efficiency, reliability, and available capacity, then applies hysteresis, cooldown, and stability checks to prevent flapping.

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
- [Python Desktop Bindings](bindings/python/README.md)
- [Reticulum Transport](docs/reticulum.md) / [Nostr Transport](docs/nostr.md)
- [Telemetry](docs/telemetry.md)
- [iOS Integration](docs/ios-integration.md) / [Android Integration](docs/android-integration.md)

## Development

```bash
cargo build --workspace            # Build
cargo test --workspace             # Test
cargo clippy --workspace -- -D warnings  # Lint
cargo fmt --all                    # Format
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for development guidelines and [QUICKSTART.md](QUICKSTART.md) for platform-specific setup.

## License

The Offline Protocol SDK is **dual-licensed**:

- **GNU Affero General Public License v3.0** (AGPL-3.0-only) — see [LICENSE](LICENSE).
  Free for use in projects that comply with AGPL-3.0, including its network-use
  source-disclosure requirement (section 13).
- **Commercial License** — for organizations that cannot or do not wish to comply
  with the AGPL (e.g., shipping the SDK inside a proprietary mobile app or SaaS
  without releasing source). See [LICENSE-COMMERCIAL.md](LICENSE-COMMERCIAL.md)
  for terms and contact details.

You may use the SDK under **either** license; you do not need both. Contributions
are accepted under the terms described in [CONTRIBUTING.md](CONTRIBUTING.md).
