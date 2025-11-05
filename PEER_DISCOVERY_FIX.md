# Peer Discovery Fix Summary

## Problem
The peers were not being discovered and the transport was showing as "none" with 0 connected neighbors. This occurred because:

1. **Native BLE managers were disconnected from Rust protocol** - The iOS and Android BLE managers were discovering peers but not notifying the Rust transport layer
2. **Wrong event types** - BLE managers emitted `peer_discovered` events instead of `neighbor_discovered` events that the NetworkScreen expected
3. **Missing transport events** - No `transport_switched` events were being emitted to indicate BLE was available
4. **No FFI bridge** - There were no functions to pass peer discovery information from native code to Rust

## Solution Implemented

### 1. Added FFI Bridge Functions (Rust)
**File: `crates/offline-protocol-ffi/src/lib.rs`**

Added new FFI functions to notify the Rust BLE transport:
- `offline_protocol_ble_peer_discovered()` - Notifies when a peer is discovered
- `offline_protocol_ble_peer_lost()` - Notifies when a peer is lost
- `offline_protocol_ble_status_changed()` - Notifies of transport status changes
- `offline_protocol_ble_get_peer_count()` - Gets the number of discovered peers

The `ProtocolHandle` now includes a `BleTransport` instance that tracks discovered peers.

### 2. Updated iOS Integration
**File: `bindings/react-native/ios/OfflineProtocolModule.swift`**

- Calls FFI functions when peers are discovered/lost
- Changed event types from `peer_discovered` to `neighbor_discovered` (matches NetworkScreen expectations)
- Emits `transport_switched` events when BLE status changes
- Logs all peer discoveries for debugging

### 3. Updated Android Integration
**File: `bindings/react-native/android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt`**

- Added native method declarations for BLE FFI functions
- Calls FFI functions when peers are discovered/lost  
- Changed event types to match NetworkScreen expectations
- Emits `transport_switched` events

### 4. Added JNI Wrappers (Android)
**File: `bindings/react-native/android/src/main/cpp/offline_protocol_jni.cpp`**

Implemented JNI wrappers for the new FFI functions:
- `nativeBlePeerDiscovered()`
- `nativeBlePeerLost()`
- `nativeBleStatusChanged()`
- `nativeBleGetPeerCount()`

## How It Works Now

```
┌─────────────────────────────────────────────────────────────┐
│                    React Native App                          │
│  ┌────────────────┐           ┌─────────────────┐          │
│  │ NetworkScreen  │◄──────────┤ Event Emitter   │          │
│  └────────────────┘           └─────────────────┘          │
└─────────────────────────────────────────────────────────────┘
                                      ▲
                                      │ Events
                                      │
┌─────────────────────────────────────────────────────────────┐
│              Native Layer (iOS / Android)                    │
│  ┌──────────────┐         FFI Calls         ┌─────────────┐│
│  │ BLE Manager  │───────────────────────────►│   Rust FFI  ││
│  │              │  offline_protocol_ble_*()  │             ││
│  │ - Discovery  │                             │  Protocol   ││
│  │ - Advertise  │                             │  Handle     ││
│  │ - Messaging  │                             │             ││
│  └──────────────┘                             └─────────────┘│
└─────────────────────────────────────────────────────────────┘
                                                       │
                                                       ▼
                                        ┌──────────────────────┐
                                        │  BleTransport        │
                                        │  - Stores peers      │
                                        │  - Tracks status     │
                                        │  - Queue messages    │
                                        └──────────────────────┘
```

**Flow:**
1. Native BLE manager discovers a peer
2. Calls FFI function `offline_protocol_ble_peer_discovered()`
3. Rust `BleTransport` stores the peer information
4. Native emits `neighbor_discovered` event to React Native
5. NetworkScreen updates to show:
   - Transport: "ble"
   - Connected Neighbors: count of discovered peers
   - Discovered Neighbors list with RSSI values

## How to Rebuild and Test

### Step 1: Build iOS Libraries
```bash
cd /Users/goku/projects/offline/offline-protocol-sdk/bindings/react-native
chmod +x scripts/build-ios.sh
./scripts/build-ios.sh
```

### Step 2: Build Android Libraries  
```bash
./scripts/build-android.sh
```

### Step 3: Rebuild React Native App
```bash
cd examples/react-native-app

# For iOS
cd ios
pod install
cd ..
npm run ios

# For Android
npm run android
```

### Step 4: Test Peer Discovery

**Setup:**
1. Run the app on two physical devices (iOS and/or Android)
2. Make sure Bluetooth is enabled on both devices
3. Grant all necessary permissions when prompted

**Testing Steps:**
1. On both devices, click "Start Protocol"
2. Check logs for "Peer discovered" messages
3. Go to the "Network" tab
4. You should see:
   - **Transport:** "ble" (instead of "None")
   - **Connected Neighbors:** 1 or more (instead of 0)
   - **Discovered Neighbors section** showing peer IDs with RSSI values

**Expected Logs (Android):**
```
OfflineProtocolModule: Peer discovered: user_abc at AA:BB:CC:DD:EE:FF (RSSI: -60)
OfflineProtocolModule: Successfully notified Rust transport of peer: user_abc
BleManager: Discovered peer: user_abc at AA:BB:CC:DD:EE:FF (RSSI: -60)
```

**Expected Logs (iOS):**
```
[OfflineProtocol] Peer discovered: user_abc at <UUID> (RSSI: -60)
[OfflineProtocol] Successfully notified Rust transport of peer: user_abc
[BleManager] Discovered peer device: user_abc
```

## Key Files Changed

### Rust
- `crates/offline-protocol-ffi/src/lib.rs` - Added BLE FFI functions
- `crates/offline-protocol-ffi/Cargo.toml` - Added transport dependency
- `crates/offline-protocol-ffi/offline_protocol.h` - Auto-generated header

### iOS
- `bindings/react-native/ios/OfflineProtocolModule.swift` - Added FFI calls and event type fixes
- `bindings/react-native/ios/BleManager.swift` - No changes (already working)

### Android
- `bindings/react-native/android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt` - Added FFI calls and event type fixes
- `bindings/react-native/android/src/main/cpp/offline_protocol_jni.cpp` - Added JNI wrappers
- `bindings/react-native/android/src/main/java/com/offlineprotocol/BleManager.kt` - No changes (already working)

## Troubleshooting

### Peers Still Not Discovered

**iOS:**
- Check Bluetooth permissions in Settings > App > Permissions
- Check logs for "BLE manager state: 5" (poweredOn)
- Make sure both devices have different user IDs

**Android:**  
- Check Bluetooth and Location permissions are granted
- Check logs for "BLE started successfully"
- Verify Bluetooth is enabled system-wide

### Transport Shows "None"

- Check that `transport_switched` events are being emitted
- Verify BLE manager status is "available" or "scanning"
- Check console logs for any errors during BLE initialization

### Connected Neighbors Shows 0

- Verify `neighbor_discovered` events (not `peer_discovered`) are being emitted
- Check React Native debugger for events array
- Confirm FFI functions are being called (check logs for "Successfully notified Rust transport")

## Next Steps

1. Test message sending between discovered peers
2. Verify reconnection after BLE disconnect
3. Test with 3+ devices for relay functionality
4. Monitor battery usage during extended peer discovery
5. Add metrics for peer discovery latency

## Technical Notes

- The BLE transport is created when the protocol handle is initialized
- Peer information is stored in the Rust `BleTransport` and can be queried via FFI
- Native BLE managers handle the actual Bluetooth operations (scanning, advertising, GATT)
- The protocol itself still uses `MockTransport` internally but this doesn't affect peer discovery
- Event emission happens at the native layer and flows up to React Native

## Related Documentation

- `BLE_IMPLEMENTATION_GUIDE.md` - Complete BLE architecture
- `REBUILD_AND_TEST.md` - General rebuild instructions
- `docs/architecture.md` - Overall protocol architecture

