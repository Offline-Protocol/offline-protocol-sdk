# iOS Integration Guide

This guide shows how to integrate the Offline Protocol SDK into a native iOS app (Swift).

## Prerequisites

- Xcode 14+
- Rust toolchain with iOS targets
- CocoaPods or Swift Package Manager

## Setup

### 1. Install Rust iOS Targets

```bash
rustup target add aarch64-apple-ios
rustup target add x86_64-apple-ios    # For simulator
rustup target add aarch64-apple-ios-sim  # For M1 simulator
```

### 2. Build Rust Library

```bash
cd crates/offline-protocol-uniffi

# Build for iOS devices
cargo build --release --target aarch64-apple-ios

# Build for simulator
cargo build --release --target aarch64-apple-ios-sim
```

### 3. Create XCFramework

```bash
# Combine into universal library
lipo -create \\
    target/aarch64-apple-ios/release/liboffline_protocol.a \\
    -output liboffline_protocol.a

# Add to Xcode project under Frameworks
```

### 4. Use in Swift

```swift
import OfflineProtocolSDK

class ViewController: UIViewController {
    var protocol: OfflineProtocol?
    
    override func viewDidLoad() {
        super.viewDidLoad()
        
        // Initialize protocol
        let config = ProtocolConfig(
            appId: "my-ios-app",
            userId: "user123"
        )
        
        do {
            protocol = try OfflineProtocol(config: config)
            try protocol?.start()
            
            // Set up event handler
            protocol?.onEvent { event in
                switch event {
                case .messageReceived(let msg):
                    print("Received: \\(msg.content)")
                case .transportSwitched(let evt):
                    print("Switched to \\(evt.to)")
                default:
                    break
                }
            }
            
            // Send a message
            let messageId = try protocol?.sendMessage(
                recipient: "user456",
                content: "Hello from iOS!",
                priority: .high
            )
            
            print("Message sent: \\(messageId ?? "")")
            
        } catch {
            print("Error: \\(error)")
        }
    }
    
    deinit {
        try? protocol?.stop()
    }
}
```

## Permissions

Add to `Info.plist`:

```xml
<!-- Bluetooth permissions -->
<key>NSBluetoothAlwaysUsageDescription</key>
<string>This app uses Bluetooth to communicate with nearby devices</string>

<key>NSBluetoothPeripheralUsageDescription</key>
<string>This app uses Bluetooth to communicate with nearby devices</string>

<!-- Background modes (REQUIRED for reliable BLE operation) -->
<!-- Without these, iOS will throttle/stop BLE scanning and advertising -->
<!-- causing missed discoveries and false "peer lost" events -->
<key>UIBackgroundModes</key>
<array>
    <string>bluetooth-central</string>
    <string>bluetooth-peripheral</string>
</array>
```

## CocoaPods

Add to your `Podfile`:

```ruby
pod 'OfflineProtocolSDK', '~> 0.1'
```

## Swift Package Manager

Add to `Package.swift`:

```swift
dependencies: [
    .package(url: "https://github.com/offline-protocol/sdk", from: "0.1.0")
]
```

## Architecture

```
iOS App (Swift/Objective-C)
    ↓
Bridging Header (offline_protocol.h)
    ↓
Rust FFI (C API)
    ↓
Rust Core (100% safe)
```

## Platform Limitations

iOS does **not** support Wi-Fi Direct. Available transports:
- Bluetooth Low Energy
- Internet
- Wi-Fi Direct (Android only) - Not available

## Performance

- Message sending: <1ms overhead
- Memory safe: Zero buffer overflows or memory leaks
- Battery efficient: Optimized for iOS power management

