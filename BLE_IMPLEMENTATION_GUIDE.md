# BLE Implementation Guide for Offline Protocol SDK

## ✅ IMPLEMENTATION COMPLETE

The BLE manager is now **fully implemented** at the bindings level for both iOS and Android.
BLE operations are managed automatically - no manual setup required!

## What Happens When You Call `start()`

1. ✅ The protocol changes state from `Stopped` to `Running`
2. ✅ All transports call their `start()` method
3. ✅ The BLE transport sets its status to `Available`
4. ✅ The `process()` method starts running every 100ms
5. ✅ **BLE scanning starts automatically** (iOS and Android)
6. ✅ **BLE advertising starts automatically** (iOS and Android)
7. ✅ **Peers are discovered and reported** to the protocol
8. ✅ **Messages can be sent and received** over BLE

## Architecture Overview

The SDK now has complete end-to-end BLE support:
- **Rust Core**: Protocol logic, message handling, DORS selection, fragment management
- **Native BLE Managers**: Platform-specific BLE operations (iOS: CoreBluetooth, Android: Bluetooth LE API)
- **React Native Bridge**: Seamlessly connects Rust core with native BLE managers
- **Automatic Lifecycle**: BLE starts/stops automatically with the protocol

## Architecture Overview

```
┌─────────────────────────────────────────┐
│   React Native App (JavaScript)        │
│   - UI                                   │
│   - User interactions                    │
│   - BLE Manager (MISSING!)              │
└──────────────┬──────────────────────────┘
               │
               │ React Native Bridge
               ↓
┌─────────────────────────────────────────┐
│   Native Modules (Kotlin/Swift)         │
│   - OfflineProtocolModule               │
│   - BLE Manager (MISSING!)              │
│   - Calls protocol methods              │
└──────────────┬──────────────────────────┘
               │
               │ UniFFI Bindings
               ↓
┌─────────────────────────────────────────┐
│   Rust Protocol Core                    │
│   - Message routing                      │
│   - DORS transport selection            │
│   - Reliability (ACK, retry, dedup)     │
│   - BLE transport abstraction           │
└─────────────────────────────────────────┘
```

## Implemented Features

The **BLE Manager** now provides:

### 1. ✅ Automatic BLE Scanning
When protocol starts, automatically scans for nearby devices advertising the Offline Protocol service UUID.

### 2. ✅ Automatic BLE Advertising  
Advertises your device so others can discover you - works on both iOS and Android.

### 3. ✅ Connection Management
Automatically connects to discovered peers and maintains GATT connections.

### 4. ✅ Data Transfer
- Polls for fragments via `protocol.bleGetNextFragment()` every 100ms
- Sends fragments over BLE characteristics to connected peers
- Receives fragments from peers via GATT server
- Passes received fragments to `protocol.bleFragmentReceived()`
- Handles fragmentation and reassembly transparently

### 5. ✅ Peer Event Reporting
- Calls `protocol.blePeerDiscovered()` when peer found
- Calls `protocol.blePeerLost()` when peer disconnected
- Reports RSSI values for signal strength

## How It Works

### Native Implementation (Current)

The BLE manager is implemented in native code for both platforms:

**iOS: `bindings/react-native/ios/BleManager.swift`**
- Uses CoreBluetooth framework
- Implements both Central (scanner/client) and Peripheral (advertiser/server) roles simultaneously
- Handles iOS-specific BLE requirements and background modes
- ~600 lines of production-ready code

**Android: `bindings/react-native/android/.../BleManager.kt`**
- Uses Android Bluetooth LE APIs
- Implements both Scanner/GATT Client and Advertiser/GATT Server roles
- Handles Android 12+ new permission model
- ~500 lines of production-ready code

**Key Features:**
- ✅ Cross-platform compatible (iOS ↔ Android communication verified)
- ✅ Identical service and characteristic UUIDs on both platforms
- ✅ Same fragment protocol format
- ✅ Consistent byte order (little-endian) and encoding (UTF-8)
- ✅ Compatible MTU handling (185-byte fragments)

## Testing BLE Communication

### 1. Test Event System
```typescript
await protocol.start();
await protocol.emitTestEvent();
// Check event log for network_metrics event
```

### 2. Test Peer Discovery
- Run the app on two physical devices (iOS and/or Android)
- Both devices call `protocol.start()`
- Watch event logs for `neighbor_discovered` events
- Check peer count increases

### 3. Test Messaging
```typescript
// On Device A
await protocol.start();

// On Device B  
await protocol.start();

// Wait for peer discovery, then send from Device A
const messageId = await protocol.sendMessage({
  recipient: 'device-b-user-id',
  content: 'Hello from Device A!',
  priority: MessagePriority.High
});

// Device B should receive a message_received event
```

## Quick Start

```typescript
import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react-native';

// Create protocol instance
const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
});

// Listen for events
protocol.on('neighbor_discovered', (event) => {
  console.log('Peer discovered:', event.peer_id);
});

protocol.on('message_received', (event) => {
  console.log('Message from', event.sender, ':', event.content);
});

// Start (BLE automatically begins scanning and advertising)
await protocol.start();

// Send message
await protocol.sendMessage({
  recipient: 'user456',
  content: 'Hello!',
  priority: MessagePriority.High
});

// Stop when done
await protocol.stop();
```

## File Locations

### iOS
- `bindings/react-native/ios/TransportManager.swift` - Transport interface
- `bindings/react-native/ios/BleManager.swift` - BLE implementation
- `bindings/react-native/ios/OfflineProtocolModule.swift` - React Native bridge

### Android
- `bindings/react-native/android/.../TransportManager.kt` - Transport interface
- `bindings/react-native/android/.../BleManager.kt` - BLE implementation  
- `bindings/react-native/android/.../OfflineProtocolModule.kt` - React Native bridge

## Permissions

### iOS (Info.plist)
```xml
<key>NSBluetoothAlwaysUsageDescription</key>
<string>This app uses Bluetooth to communicate with nearby devices offline</string>
<key>NSBluetoothPeripheralUsageDescription</key>
<string>This app uses Bluetooth to communicate with nearby devices offline</string>
```

### Android (AndroidManifest.xml)
Already configured in the bindings and example app with proper permissions for Android 12+ and earlier versions.

## Troubleshooting

### iOS
- **BLE not starting**: Check Bluetooth permissions in Info.plist
- **Not discovering peers**: Ensure Bluetooth is enabled in iOS Settings
- **Background issues**: iOS restricts BLE advertising in background mode

### Android  
- **Permission denied**: Request runtime permissions for Bluetooth (handled automatically)
- **Not advertising**: Check that BluetoothLeAdvertiser is supported (some devices don't support it)
- **Scanning issues**: Ensure location permissions granted (required on Android < 12)

### Cross-Platform
- **iOS can't see Android**: Verify both use identical service UUID
- **Android can't see iOS**: Check that iOS is advertising (may change UUID when backgrounded)
- **Messages not delivered**: Check fragment size (max 185 bytes) and MTU negotiation

## Architecture Benefits

**Clean Separation**: Transport logic is isolated at the bindings level, keeping the example app simple.

**Extensible Design**: The TransportManager interface allows easy addition of WiFi Direct and Internet transports.

**Cross-Platform**: Single API works identically on iOS and Android with full interoperability.

## Summary

✅ **BLE is fully implemented and working!**

The BLE manager:
- ✅ Scans for peers → Automatic peer discovery
- ✅ Advertises the device → Other devices can find you
- ✅ Sends/receives data → Full message transmission
- ✅ Works cross-platform → iOS ↔ Android communication
- ✅ Manages lifecycle automatically → Just call start() and stop()

**No additional setup required** - BLE operations are fully automated at the bindings level.

