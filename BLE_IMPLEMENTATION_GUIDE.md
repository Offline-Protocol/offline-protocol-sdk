# BLE Implementation Guide for Offline Protocol SDK

## Problem Summary

The `start()` method **IS working**, but nothing appears to happen because **the BLE manager implementation is missing**.

### What Happens When You Call `start()`

1. ✅ The protocol changes state from `Stopped` to `Running`
2. ✅ All transports call their `start()` method
3. ✅ The BLE transport sets its status to `Available`
4. ✅ The `process()` method starts running every 100ms
5. ❌ **BUT**: No BLE scanning happens
6. ❌ **BUT**: No BLE advertising happens  
7. ❌ **BUT**: No peers are discovered
8. ❌ **BUT**: No messages can be sent/received

### Why Nothing Happens

The SDK architecture separates concerns:
- **Rust Core**: Protocol logic, message handling, DORS selection
- **Native Platform**: Actual Bluetooth operations (scanning, advertising, GATT)
- **React Native Bridge**: Connects the two layers

**The native BLE implementation is completely missing!**

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

## What's Missing: BLE Manager

You need to implement a **BLE Manager** that:

### 1. Starts BLE Scanning
When protocol starts, scan for nearby devices advertising the Offline Protocol service.

### 2. Starts BLE Advertising  
Advertise your device so others can discover you.

### 3. Manages Connections
Connect to discovered peers and maintain GATT connections.

### 4. Handles Data Transfer
- Get fragments to send via `protocol.bleGetNextFragment()`
- Send fragments over BLE characteristics
- Receive fragments from peers
- Pass received fragments to `protocol.bleFragmentReceived()`

### 5. Reports Peer Events
- Call `protocol.blePeerDiscovered()` when peer found
- Call `protocol.blePeerLost()` when peer disconnected

## Implementation Options

### Option 1: Use react-native-ble-plx (Recommended)

Install the library:
```bash
npm install react-native-ble-plx
cd ios && pod install
```

Then create a BLE Manager (pseudocode):

```typescript
import { BleManager, Device } from 'react-native-ble-plx';

class OfflineBleManager {
  private bleManager: BleManager;
  private protocol: OfflineProtocol;
  
  // UUIDs from Rust BLE transport
  private SERVICE_UUID = '6E400001-B5A3-F393-E0A9-E50E24DCCA9E';
  private MESSAGE_CHAR_UUID = '6E400002-B5A3-F393-E0A9-E50E24DCCA9E';
  
  async start() {
    // 1. Start scanning
    this.bleManager.startDeviceScan(
      [this.SERVICE_UUID], 
      { allowDuplicates: true },
      this.onDeviceDiscovered
    );
    
    // 2. Start advertising
    await this.startAdvertising();
    
    // 3. Poll for fragments to send
    setInterval(() => this.sendPendingFragments(), 100);
  }
  
  onDeviceDiscovered = async (error, device) => {
    if (!device) return;
    
    // Report to protocol
    await this.protocol.blePeerDiscovered(device.id, device.rssi);
    
    // Connect and setup GATT
    await this.connectToDevice(device);
  };
  
  async sendPendingFragments() {
    const fragment = await this.protocol.bleGetNextFragment();
    if (fragment) {
      // Send via BLE characteristic
      await this.sendFragment(fragment.recipientId, fragment.data);
    }
  }
  
  async onFragmentReceived(senderId: string, data: number[]) {
    await this.protocol.bleFragmentReceived(senderId, data);
  }
}
```

### Option 2: Native Implementation

Implement BLE managers in native code:

**iOS: BleManager.swift**
```swift
import CoreBluetooth

class BleManager: NSObject, CBCentralManagerDelegate, CBPeripheralManagerDelegate {
    private var centralManager: CBCentralManager!
    private var peripheralManager: CBPeripheralManager!
    private var protocol: OfflineProtocol
    
    // Implement CBCentralManagerDelegate methods
    // Implement CBPeripheralManagerDelegate methods
    // Bridge to protocol via protocol.blePeerDiscovered(), etc.
}
```

**Android: BleManager.kt**
```kotlin
import android.bluetooth.*

class BleManager(private val protocol: OfflineProtocol) {
    private var bluetoothAdapter: BluetoothAdapter? = null
    private var gattServer: BluetoothGattServer? = null
    
    fun start() {
        // Start scanning
        // Start advertising GATT server
        // Handle connections
        // Bridge to protocol
    }
}
```

## Testing the Event System

First, verify the event system works:

```typescript
// In your app after calling start()
await protocol.emitTestEvent();

// You should see a network_metrics event in your event log
```

If events work but you see no peer discovery:
- ✅ Event system is working
- ❌ BLE manager is missing - that's the problem!

## Quick Start: Minimal Working Example

For testing without full BLE:

```typescript
// Simulate peer discovery for testing
setTimeout(async () => {
  await protocol.blePeerDiscovered('test-peer-123', -60);
}, 2000);

// Simulate receiving a message
setTimeout(async () => {
  // Create a fake fragment (you'd need to generate a real one)
  const fakeFragment = [/* binary data */];
  await protocol.bleFragmentReceived('test-peer-123', fakeFragment);
}, 5000);
```

## What The Documentation References

The file `docs/ios-ble-fixes.md` references a `BleManager.swift` that should exist but doesn't. This file was supposed to:
- Create `CBCentralManager` for scanning
- Create `CBPeripheralManager` for advertising
- Handle GATT server/client operations
- Bridge BLE events to the protocol

## Next Steps

1. **Verify Event System**: Add a button to call `emitTestEvent()` and confirm events work
2. **Choose Implementation**: Decide between JS library (react-native-ble-plx) or native code
3. **Implement BLE Manager**: Follow one of the implementation options above
4. **Test Discovery**: Verify peers are discovered when both devices run the app
5. **Test Messaging**: Send messages and verify delivery

## Why This Happened

The SDK was designed with clean separation of concerns, expecting platform-specific BLE implementations to be provided separately. The example app shows the protocol API but doesn't include the complete platform BLE layer.

This is by design for flexibility (different apps might use different BLE libraries), but it means the example app is incomplete without a BLE manager implementation.

## Summary

**The protocol IS starting correctly.** The issue is that there's no BLE manager to:
- Scan for peers → No peer discovery
- Advertise the device → Other devices can't find you  
- Send/receive data → No message transmission

You need to implement or integrate a BLE manager to make the example app fully functional.

