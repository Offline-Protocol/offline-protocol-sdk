# iOS Peer Discovery Fix

## Problem

iOS devices were unable to discover peers while Android devices could discover peers successfully.

## Root Cause

The iOS `BleManager.swift` implementation had a race condition where BLE scanning and advertising were started independently when their respective managers became ready:

- `CBCentralManager` (for scanning) would call `startScanning()` when it became powered on
- `CBPeripheralManager` (for advertising) would call `startAdvertising()` when it became powered on

This created a situation where:
1. Only one operation might start if the other manager wasn't ready yet
2. iOS device might only be **scanning** OR **advertising**, but not both simultaneously
3. Without both operations running, peer discovery cannot work (the device needs to advertise to be discoverable AND scan to discover others)

In contrast, the Android implementation correctly started all three operations in sequence:
1. Start GATT server
2. Start advertising
3. Start scanning

## Solution

Added synchronization to ensure both BLE operations (scanning and advertising) start only when **both** managers are ready:

### Changes Made

1. **Added state tracking variables** (lines 45-48):
   ```swift
   private var centralManagerReady = false
   private var peripheralManagerReady = false
   private var shouldStartOperations = false
   ```

2. **Updated `start()` method** to set a flag and attempt to start operations (lines 75-94):
   ```swift
   @objc func start() -> Bool {
       shouldStartOperations = true
       // ... initialize managers ...
       startOperationsIfReady()
       return true
   }
   ```

3. **Added `startOperationsIfReady()` method** (lines 212-226):
   - Checks if both managers are ready
   - Only starts scanning and advertising when BOTH are available
   - Provides detailed logging for debugging

4. **Updated delegate methods** to track ready state:
   - `centralManagerDidUpdateState` (lines 233-248): Sets `centralManagerReady` and calls `startOperationsIfReady()`
   - `peripheralManagerDidUpdateState` (lines 370-385): Sets `peripheralManagerReady` and calls `startOperationsIfReady()`

5. **Updated `stop()` method** to reset the flag (line 99):
   ```swift
   shouldStartOperations = false
   ```

## Testing

To verify the fix works:

1. **Rebuild the iOS app**:
   ```bash
   cd examples/react-native-app
   
   # For iOS
   cd ios
   pod install
   cd ..
   npx react-native run-ios
   ```

2. **Test peer discovery**:
   - Run the app on an iOS device (Device A)
   - Run the app on another device - iOS or Android (Device B)
   - Check the Network screen on both devices
   - Both devices should now appear in each other's "Discovered Peers" list

3. **Check logs** for confirmation:
   ```
   [BleManager] Both managers ready - starting scanning and advertising
   [BleManager] Starting BLE scanning...
   [BleManager] Starting BLE advertising...
   [BleManager] Discovered peripheral: <UUID>
   [BleManager] Discovered peer device: <peer-id>
   ```

## Expected Behavior

After this fix:
- iOS device will wait until both `CBCentralManager` and `CBPeripheralManager` are ready
- Both scanning and advertising will start simultaneously
- iOS devices can now discover other iOS and Android devices
- iOS devices can be discovered by other iOS and Android devices
- Peer discovery works bidirectionally on all platforms

## Additional Notes

- The fix maintains backward compatibility
- No changes to the public API
- Follows iOS BLE best practices for concurrent peripheral/central operations
- Detailed logging helps with debugging BLE state issues

