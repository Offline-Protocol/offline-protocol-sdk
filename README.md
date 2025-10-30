# Offline Protocol SDK

A high-performance, cross-platform SDK for offline-first messaging with intelligent transport switching.

## Features

- **DORS (Dynamic Offline Relay Switch)**: Automatically switches between Internet, BLE Mesh, and Wi-Fi Direct
- **Reliable Delivery**: ACK-based reliability with exponential backoff retry
- **Cross-Platform**: Rust core with bindings for Android, iOS, React Native, and Web
- **Memory Safe**: 95% safe Rust core, unsafe code isolated to FFI boundaries only

## Architecture

```
Rust Core (100% safe)
├── offline-protocol-core      # Message types and core data structures
├── offline-protocol-transport # Transport abstraction layer
├── offline-protocol-router    # DORS and relay management
├── offline-protocol-reliability # ACK, retry, deduplication
└── offline-protocol           # Main SDK API

FFI Layer (5% unsafe, carefully reviewed)
└── offline-protocol-ffi       # C bindings

Platform Bindings
├── Android (Kotlin/JNI)
├── iOS (Swift)
├── React Native (JavaScript/TypeScript)
└── Web (WebAssembly)
```

## Building

```bash
# Build all crates
cargo build --all

# Run tests
cargo test --all

# Run linter
cargo clippy --all -- -D warnings

# Format code
cargo fmt --all
```

## License

MIT OR Apache-2.0

