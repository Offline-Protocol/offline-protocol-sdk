# Offline Protocol SDK

> Offline-first messaging protocol with intelligent multi-transport switching and mesh networking

## 🎉 Now Using UniFFI for Type-Safe Cross-Platform Bindings!

This SDK has been migrated from manual C FFI to Mozilla's UniFFI for safer, cleaner, more maintainable cross-platform bindings.

**Benefits:**
- ✅ Zero unsafe application code (down from ~2,400 lines)
- ✅ Type-safe Swift and Kotlin APIs (auto-generated)
- ✅ Compiler-enforced correctness across Rust/Swift/Kotlin
- ✅ 70% code reduction (716 lines vs 2,400 lines)

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

### For Native iOS/Android:

See [iOS Integration Guide](docs/ios-integration.md) and [Android Integration Guide](docs/android-integration.md).

## Building the SDK

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

### Prerequisites:
- Rust (via rustup)
- uniffi-bindgen: `cargo install uniffi-bindgen --version 0.28.3`
- iOS targets: `rustup target add aarch64-apple-ios x86_64-apple-ios`
- Android targets: `rustup target add aarch64-linux-android armv7-linux-androideabi`

## Architecture

The SDK consists of modular Rust crates:

- **offline-protocol-core** - Core types and data structures
- **offline-protocol-transport** - Multi-transport abstraction (BLE, WiFi, Internet)
- **offline-protocol-router** - DORS routing and relay management
- **offline-protocol-reliability** - ACKs, retries, deduplication
- **offline-protocol** - Main protocol engine
- **offline-protocol-uniffi** - UniFFI bindings for Swift/Kotlin (NEW!)

**All core code is 100% safe Rust** (`#![deny(unsafe_code)]`)

## Features

- 📱 **Multi-Transport Support** - BLE, WiFi Direct, Internet
- 🔄 **Dynamic Offline Relay Switch (DORS)** - Intelligent transport selection
- 🌐 **Mesh Networking** - Multi-hop message routing
- 📊 **Network Visualization** - Real-time topology tracking
- 📁 **File Transfer** - Chunked file sending with progress tracking
- ⚡ **Reliable Delivery** - ACKs, retries, deduplication
- 🔒 **Type-Safe APIs** - Compiler-enforced correctness
- 🛡️ **Memory-Safe** - No buffer overflows, no leaks

## Documentation

### Getting Started:
- **[START_HERE.md](START_HERE.md)** - Navigation guide
- **[QUICK START](QUICKSTART.md)** - Quick setup
- **[Architecture](docs/architecture.md)** - System design

### Integration:
- **[iOS Integration](docs/ios-integration.md)** - iOS setup
- **[Android Integration](docs/android-integration.md)** - Android setup
- **[API Reference](docs/api-reference.md)** - Complete API

### Migration:
- **[README_UNIFFI.md](README_UNIFFI.md)** - UniFFI migration summary
- **[MIGRATION_SUCCESS_REPORT.md](MIGRATION_SUCCESS_REPORT.md)** - Verification report
- **[Build Guide](bindings/react-native/BUILD_UNIFFI.md)** - Building UniFFI

## Development

### Running Tests:
```bash
cargo test --workspace
```

### Building:
```bash
cargo build --workspace --release
```

### Benchmarks:
```bash
cargo bench
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

MIT OR Apache-2.0

---

## Recent Changes

### ✅ Version 0.2.0 - UniFFI Migration (November 2024)
- **BREAKING:** Migrated from C FFI to UniFFI
- Eliminated ~2,400 lines of unsafe code
- Type-safe Swift and Kotlin bindings now auto-generated
- All 36 API methods preserved with full compatibility
- Build system updated (`npm run build:uniffi:all`)

**See [MIGRATION_SUCCESS_REPORT.md](MIGRATION_SUCCESS_REPORT.md) for complete details.**

---

**Status:** Production Ready ✅  
**Binding System:** UniFFI (type-safe)  
**Unsafe Code:** 0 lines in application layer  
**Tests:** 127/127 passing
