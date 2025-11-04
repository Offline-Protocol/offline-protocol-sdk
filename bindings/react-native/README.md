# Offline Protocol SDK - React Native Bindings

React Native bindings for the Offline Protocol SDK, enabling offline-first messaging with automatic transport switching between Internet, BLE Mesh, and Wi-Fi Direct.

## Installation

### Prerequisites

- React Native 0.70.0 or higher
- For iOS: Xcode 14+ with iOS 12.0+ deployment target
- For Android: Android SDK 21+ (Android 5.0+)
- Rust toolchain (for building native libraries - see Building from Source below)

### Install the Package

```bash
npm install @offlineprotocol/react-native
# or
yarn add @offlineprotocol/react-native
```

**Good news**: The pre-built Rust FFI libraries are included in the npm package, so you don't need to build them separately!

### iOS Setup

1. Install CocoaPods dependencies:
```bash
cd ios
pod install
cd ..
```

The Rust library (`liboffline_protocol_ffi.a`) and header file (`offline_protocol.h`) are already included in the package.

### Android Setup

The Rust libraries for all Android architectures are already included in:
- `android/src/main/jniLibs/arm64-v8a/liboffline_protocol_ffi.so`
- `android/src/main/jniLibs/armeabi-v7a/liboffline_protocol_ffi.so`
- `android/src/main/jniLibs/x86_64/liboffline_protocol_ffi.so`
- `android/src/main/jniLibs/x86/liboffline_protocol_ffi.so`

No additional setup required!

2. Ensure your `android/app/build.gradle` includes:
```gradle
android {
    defaultConfig {
        ndk {
            abiFilters 'armeabi-v7a', 'arm64-v8a', 'x86', 'x86_64'
        }
    }
}
```

## Usage

### Basic Example

```typescript
import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react-native';

// Create and configure the protocol
const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
  transport: {
    bleEnabled: true,
    wifiDirectEnabled: true,  // Android only
    internetEnabled: true,
  },
});

// Start the protocol
await protocol.start();

// Send a message
const messageId = await protocol.sendMessage({
  recipient: 'user456',
  content: 'Hello offline world!',
  priority: MessagePriority.High,
});

// Listen for incoming messages
protocol.on('message:received', (event) => {
  console.log(`From ${event.sender}: ${event.content}`);
  console.log(`Delivered via ${event.transport} in ${event.hopCount} hops`);
});

// Monitor transport switching
protocol.on('transport:switched', (event) => {
  console.log(`Switched from ${event.from || 'none'} to ${event.transport}`);
  console.log(`Reason: ${event.reason}`);
});

// Stop when done
await protocol.stop();
```

### Event Types

The SDK emits various events that you can listen to:

- **`message:received`** - A message was received
- **`message:delivered`** - A message was successfully delivered (ACK received)
- **`message:failed`** - A message failed to deliver
- **`transport:switched`** - Transport was switched by DORS
- **`relay:promoted`** - This device was promoted to relay role
- **`relay:demoted`** - This device was demoted from relay role
- **`file:progress`** - File transfer progress update
- **`file:received`** - A file was completely received
- **`neighbor:discovered`** - A new neighbor was discovered
- **`neighbor:lost`** - A neighbor was lost (disconnected)
- **`network:metrics`** - Network metrics update

### Message Priorities

```typescript
import { MessagePriority } from '@offlineprotocol/react-native';

// Low priority - can be delayed or dropped under congestion
MessagePriority.Low

// Medium priority - default for most messages
MessagePriority.Medium

// High priority - important messages that should be delivered quickly
MessagePriority.High

// Critical priority - emergency messages, highest delivery guarantee
MessagePriority.Critical
```

### Background Mode

```typescript
// Pause when app goes to background
await protocol.pause();

// Resume when app comes to foreground
await protocol.resume();
```

## API Reference

### `OfflineProtocol`

#### Constructor

```typescript
new OfflineProtocol(config: ProtocolConfig)
```

Creates a new protocol instance.

#### Methods

- **`start()`** - Starts the protocol
- **`stop()`** - Stops the protocol
- **`pause()`** - Pauses the protocol (for background mode)
- **`resume()`** - Resumes from pause
- **`sendMessage(params)`** - Sends a message
  - `params.recipient: string` - Recipient user ID
  - `params.content: string` - Message content
  - `params.priority?: MessagePriority` - Message priority (default: Medium)
  - Returns: `Promise<string>` - Message ID
- **`sendFile(params)`** - Sends a file (not yet implemented)
  - `params.recipient: string` - Recipient user ID
  - `params.filePath: string` - Path to file
  - `params.priority?: MessagePriority` - Message priority
  - Returns: `Promise<string>` - File ID
- **`on(event, listener)`** - Registers an event listener
- **`off(event, listener)`** - Removes an event listener

## Building from Source

The npm package includes pre-built Rust FFI libraries for all supported platforms. However, if you need to build from source (for example, to use a custom build or different architecture), you can do so using the build scripts.

### Prerequisites

- Rust toolchain (`rustup`)
- Android NDK (for Android builds)
- Xcode (for iOS builds)

### Building Rust Libraries

If you have the full SDK repository:

```bash
# Clone the main SDK repository
git clone https://github.com/offline-protocol/sdk.git
cd sdk/bindings/react-native

# Build for Android (all architectures)
./scripts/build-android.sh

# Build for iOS (universal library)
./scripts/build-ios.sh
```

The build scripts will place the libraries in the correct locations:
- **Android**: `android/src/main/jniLibs/{arch}/liboffline_protocol_ffi.so`
- **iOS**: `ios/liboffline_protocol_ffi.a` and `ios/offline_protocol.h`

**Note**: The published npm package already includes these pre-built libraries, so building from source is only needed if you're developing the SDK itself or need custom builds.

## Troubleshooting

### iOS: "Module 'OfflineProtocol' not found"

1. Run `pod install` in the `ios` directory
2. Clean and rebuild: `cd ios && xcodebuild clean`
3. Ensure the podspec path is correct in `react-native.config.js`

### Android: "UnsatisfiedLinkError: dlopen failed"

1. Ensure the native libraries are in `android/src/main/jniLibs/`
2. Check that your `build.gradle` includes the correct ABI filters
3. Rebuild the app after adding the libraries

### Build Script Errors

**Android NDK not found:**
- Set `ANDROID_NDK_HOME` environment variable
- Or install NDK via Android Studio → SDK Manager → SDK Tools

**iOS build fails:**
- Ensure Xcode command-line tools are installed: `xcode-select --install`
- Verify Rust iOS targets are installed: `rustup target add aarch64-apple-ios`

## License

MIT OR Apache-2.0

## Contributing

See [CONTRIBUTING.md](../../CONTRIBUTING.md) for details.

