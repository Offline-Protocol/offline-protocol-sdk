# iOS BLE Stability Fixes

## Overview

This document describes critical fixes applied to address iOS CoreBluetooth reliability issues that were causing:
- Missed discovery events
- Random "peer lost" notifications
- Gaps where iOS doesn't deliver BLE callbacks
- Inconsistent peer connections

## Root Causes

### 1. CoreBluetooth Queue Management
**Problem:** CoreBluetooth managers (CBCentralManager and CBPeripheralManager) are sensitive to which dispatch queue they're created on. Creating them on arbitrary background queues can cause iOS to miss or delay callback delivery.

**Impact:**
- Discovery callbacks not firing
- Disconnection events not delivered
- State change callbacks delayed or lost

### 2. Scan Result Throttling
**Problem:** Without `CBCentralManagerScanOptionAllowDuplicatesKey: true`, iOS aggressively throttles duplicate advertisement packets. This means if a peer advertises every 100ms, iOS might only deliver one scan result every 3-5 seconds.

**Impact:**
- False "peer lost" events when the Rust layer hasn't seen an advertisement recently
- Inconsistent RSSI updates
- Delayed peer discovery

### 3. Background Mode Restrictions
**Problem:** Without proper `UIBackgroundModes` configuration, iOS will:
- Stop scanning after ~180 seconds in background
- Reduce advertisement frequency
- Throttle scan result delivery even more aggressively

**Impact:**
- Peers appear to disconnect when app goes to background
- Message delivery fails when app is backgrounded
- False peer loss events

## Changes Made

### 1. OfflineProtocolModule.swift

#### Change: Require Main Queue Setup
```swift
override class func requiresMainQueueSetup() -> Bool {
    // CoreBluetooth (CBCentralManager/CBPeripheralManager) must be created on main thread
    // or on a dedicated queue. Creating them off RN background queue causes:
    // - missed discovery events
    // - random "peer lost" events
    // - gaps where iOS doesn't deliver BLE callbacks
    return true  // Changed from false
}
```

**Why:** Tells React Native to initialize this module on the main thread, ensuring CoreBluetooth managers are created in the correct context from the start.

#### Change: Main Thread BLE Initialization
```swift
// Initialize BLE manager on main thread
// CoreBluetooth managers must be created on the same queue they'll be used on
DispatchQueue.main.async {
    self.initializeBleManager()
}
```

**Why:** Ensures BleManager (which creates CoreBluetooth managers) is initialized on the main queue, guaranteeing consistent callback delivery.

### 2. BleManager.swift

#### Change: Enhanced Queue Documentation
Added clear documentation that `queue: nil` means "use main queue" and why this is critical:

```swift
// IMPORTANT: queue: nil means use main queue. All CB operations must happen
// on the same queue where the manager was created to avoid missed callbacks.
self.centralManager = CBCentralManager(delegate: self, queue: nil)
```

#### Change: Scan Option Documentation
Updated comment to explain the iOS-specific issue:

```swift
options: [CBCentralManagerScanOptionAllowDuplicatesKey: true]  // CRITICAL: Without this, iOS throttles/suppresses duplicate advertisements
                                                                // causing false "peer lost" events when iOS doesn't deliver scans for 3-5s
```

**Note:** This was already set to `true` - no code change needed, just improved documentation.

### 3. Info.plist

#### Change: Add Background Modes
Added background Bluetooth capabilities:

```xml
<key>UIBackgroundModes</key>
<array>
    <string>bluetooth-central</string>
    <string>bluetooth-peripheral</string>
</array>
```

**Why:** Without these entries, iOS will:
- Suspend scanning after ~3 minutes in background
- Stop advertising when backgrounded
- Aggressively throttle BLE events

With these entries, the app can:
- Continue scanning and advertising in background
- Maintain connections when backgrounded
- Receive BLE callbacks while not in foreground

## Expected Improvements

After these changes, you should see:

1. **More Reliable Discovery**
   - Peers discovered consistently
   - No missed scan results
   - RSSI updates arrive regularly

2. **Fewer False "Peer Lost" Events**
   - iOS will deliver all duplicate advertisements
   - Rust layer sees consistent peer presence
   - No premature peer expiry

3. **Better Background Behavior**
   - Scanning continues in background
   - Advertising continues in background
   - Connections maintained when app backgrounded

4. **Consistent Callbacks**
   - State changes delivered reliably
   - Connection/disconnection events not missed
   - Characteristic updates arrive on time

## Testing Recommendations

1. **Foreground Discovery**
   - Place two devices near each other
   - Start both apps in foreground
   - Verify peers discover each other within 1-2 seconds
   - Verify no "peer lost" events while both running

2. **Background Scanning**
   - Start app on Device A, background it
   - Start app on Device B
   - Verify Device A discovers Device B while backgrounded
   - Send message from B to A
   - Verify A receives message while backgrounded

3. **Background Advertising**
   - Start app on Device A
   - Start app on Device B, background it
   - Verify Device A discovers Device B
   - Send message from A to B
   - Verify B receives message while backgrounded

4. **Long-Running Stability**
   - Run both apps for 10+ minutes
   - Move devices in/out of range
   - Verify no spurious "peer lost" events
   - Verify re-discovery works after range loss

## Additional Considerations

### Peer Timeout Configuration (Future Work)

Currently, the BleManager doesn't implement time-based peer expiry - peers are only marked as lost when iOS reports a disconnection. For future improvement, consider:

1. **Adding a peer TTL** (e.g., 10-15 seconds)
   - Track `lastSeen` timestamp (already stored)
   - Periodically check for stale peers
   - Only expire if no updates for configured TTL

2. **RSSI-based filtering**
   - Currently no RSSI filtering is applied
   - Could add threshold to ignore very weak signals
   - Useful for avoiding marginal connections

3. **Adaptive timeouts**
   - Longer TTL when in background (iOS throttles more)
   - Shorter TTL when in foreground
   - Account for iOS throttling behavior

## References

- [Apple CoreBluetooth Documentation](https://developer.apple.com/documentation/corebluetooth)
- [CBCentralManager Queue Behavior](https://developer.apple.com/documentation/corebluetooth/cbcentralmanager)
- [iOS Background Execution Modes](https://developer.apple.com/documentation/bundleresources/information_property_list/uibackgroundmodes)
- [Bluetooth Background Processing](https://developer.apple.com/library/archive/documentation/NetworkingInternetWeb/Conceptual/CoreBluetooth_concepts/CoreBluetoothBackgroundProcessingForIOSApps/PerformingTasksWhileYourAppIsInTheBackground.html)

## Changelog

- **2025-11-06**: Initial fixes applied
  - Changed `requiresMainQueueSetup()` to return `true`
  - Added `DispatchQueue.main.async` wrapper for BLE initialization
  - Added `UIBackgroundModes` to Info.plist
  - Enhanced documentation for queue management and scan options

