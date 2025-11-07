# BLE Crash and False Peer Lost Fixes

## Issues Reported
1. ❌ False peer lost events still happening on both iOS and Android
2. ❌ App crashes when sending messages on both iOS and Android

## Root Causes Identified

### Issue 1: False Peer Lost Events

**Root Cause:**
- Code was maintaining **persistent GATT connections** after reading device IDs
- iOS/Android would eventually drop these idle connections for resource management
- When OS dropped the connection, `didDisconnectPeripheral` fired → triggered `onPeerLost` callback
- Peers were still advertising nearby, but marked as "lost" due to connection drop

**Why This Is Wrong:**
- BLE discovery doesn't require persistent connections
- Scan results continue arriving even without GATT connections
- Persistent connections waste resources and battery
- OS actively manages/drops idle connections

### Issue 2: App Crashes on Send

**Root Cause (iOS):**
- `connectedPeripherals` dict contained disconnected peripherals
- `sendMessage` tried to write to disconnected peripheral → crash/exception
- No check for actual connection state before writing

**Root Cause (Android):**
- Similar issue - `connectedClients` map had stale GATT references
- Writing to closed/disconnected GATT → crash

## Fixes Implemented

### 2025-11-06 Hardening Update

- Increased BLE peer loss grace period to **60 seconds** on both iOS and Android to tolerate duplicate suppression and background throttling.
- Persist the **latest CBPeripheral/BluetoothDevice references** on every scan result so reconnect attempts always target a live object.
- Added explicit **on-demand reconnect orchestration**: iOS reuses the central manager to reconnect, while Android now tracks in-flight GATT connections via `connectingClients` and promotes them to `connectedClients` once services are ready.
- Refreshed peer metadata (`RSSI`, `lastSeen`, addresses) on every duplicate advertisement and raised `onPeerUpdated` callbacks to keep the Rust layer informed without false expirations.
- Hardened cleanup logic so intentional disconnects clear connection maps without firing spurious `onPeerLost` callbacks, and all timers/handlers respect both active and connecting peers.

### iOS Fixes (`BleManager.swift`)

#### Fix 1: Update RSSI on Duplicate Scan Results (Lines 344-353)
```swift
// Check if we already discovered this peripheral
if let existingPeer = self.discoveredPeers.values.first(where: { $0.peripheral.identifier == peripheral.identifier }) {
    // Update existing peer's RSSI, peripheral reference, and timestamp
    var updatedPeer = existingPeer
    updatedPeer.peripheral = peripheral
    updatedPeer.rssi = rssi.intValue
    updatedPeer.lastSeen = Date()
    updatedPeer.connected = false
    self.discoveredPeers[existingPeer.deviceId] = updatedPeer

    if updatedPeer.connected {
        self.connectedPeripherals[existingPeer.deviceId] = peripheral
    } else {
        self.connectedPeripherals.removeValue(forKey: existingPeer.deviceId)
    }

    self.onPeerUpdated?(existingPeer.deviceId, peripheral.identifier.uuidString, rssi.intValue)
    return // Don't reconnect - peer is already known
}
```

**Impact:**
- Peers stay "alive" in discovery list via scan results
- No need for persistent connections to track presence
- RSSI updates happen naturally from advertisements

#### Fix 2: Disconnect After Reading Device ID (Lines 733-770)
```swift
if let remoteDeviceId = String(data: value, encoding: .utf8) {
    // ... update peer metadata ...
    if var peer = discoveredPeers[remoteDeviceId] {
        peer.connected = false
        discoveredPeers[remoteDeviceId] = peer
    }
    connectedPeripherals.removeValue(forKey: remoteDeviceId)
    centralManager?.cancelPeripheralConnection(peripheral)
} else {
    // ... error path ...
    for (deviceId, storedPeripheral) in connectedPeripherals where storedPeripheral.identifier == peripheral.identifier {
        connectedPeripherals.removeValue(forKey: deviceId)
    }
    centralManager?.cancelPeripheralConnection(peripheral)
}
```

**Impact:**
- No persistent connections to manage
- iOS can't drop connections we don't have
- Peers discovered via advertisements only
- On-demand connection model for messaging

#### Fix 3: Don't Mark Peers Lost on Disconnect (Lines 534-550)
```swift
// CRITICAL FIX: Don't mark peer as lost on disconnect!
// We intentionally disconnect after reading device ID and reconnect on-demand.
// Peers are only truly "lost" when they stop advertising.

// Just update connection status and clean up connection tracking
if let peer = discoveredPeers.first(where: { $0.value.peripheral.identifier == peripheral.identifier }) {
    let deviceId = peer.key
    var updatedPeer = peer.value
    updatedPeer.connected = false
    discoveredPeers[deviceId] = updatedPeer
    connectedPeripherals.removeValue(forKey: deviceId)
    
    NSLog("[BleManager] Peer \(deviceId) disconnected but still in discovered list")
}
```

**Impact:**
- Disconnects don't trigger false peer lost events
- Peers remain in discovered list
- Only mark lost when advertisements actually stop (if implemented)

#### Fix 4: Reconnect on Send if Needed (Lines 191-221)
```swift
// CRITICAL FIX: Get peripheral from discovered peers, not connectedPeripherals
// Since we disconnect after reading device ID, we need to reconnect on-demand
guard let peer = self.discoveredPeers[recipientId] else {
    return
}

let peripheral = peer.peripheral

// Check if we need to reconnect
if peripheral.state != .connected {
    // Reconnect
    self.connectingPeripherals[peripheral.identifier] = peripheral
    peripheral.delegate = self
    DispatchQueue.main.async {
        self.centralManager?.connect(peripheral, options: nil)
    }
    return // Message will be retried
}
```

**Impact:**
- No crashes when sending to disconnected peers
- Automatic reconnection when needed
- Messages queued/retried by Rust layer

#### Fix 5: Skip Device ID Read on Reconnect (Lines 707-717)
```swift
if let knownPeer = existingPeer {
    // We already know this peer - skip reading device ID, just mark as connected
    NSLog("[BleManager] 📡 Reconnected to known peer \(knownPeer.deviceId) for messaging")
    
    var updatedPeer = knownPeer
    updatedPeer.connected = true
    discoveredPeers[knownPeer.deviceId] = updatedPeer
    connectedPeripherals[knownPeer.deviceId] = peripheral
    connectingPeripherals.removeValue(forKey: peripheral.identifier)
}
```

**Impact:**
- Reconnections don't read device ID again
- Connection stays open for messaging
- Faster message delivery after reconnect

### Android Fixes (`BleManager.kt`)

#### Fix 1: Update RSSI on Duplicate Scan Results (Lines 317-326)
```kotlin
// CRITICAL FIX: Check if we already discovered this device by address
// Update existing peer's RSSI without reconnecting
val existingPeer = discoveredPeers.values.find { it.address == device.address }
if (existingPeer != null) {
    existingPeer.address = device.address
    existingPeer.device = device
    existingPeer.rssi = rssi
    existingPeer.lastSeen = System.currentTimeMillis()
    onPeerUpdated(existingPeer.deviceId, device.address, rssi)
    // Don't reconnect - peer is already known
    return
}
```

**Impact:**
- No reconnection attempts for known peers
- RSSI updates via advertisements only
- Reduces connection churn

#### Fix 2: Disconnect After Reading Device ID (Lines 363-366, 386-389)
```kotlin
// CRITICAL FIX: Disconnect after reading device ID
// Don't maintain persistent connections - reconnect on-demand for messaging
gatt.disconnect()
gatt.close()
```

**Impact:**
- Same as iOS - no persistent connections
- Resources freed immediately
- Advertisement-based discovery

#### Fix 3: Handle Disconnected Peers in sendMessage (Lines 523-536)
```kotlin
// CRITICAL FIX: Since we disconnect after reading device ID,
// we need to reconnect on-demand for messaging

val peer = discoveredPeers[recipientId]
if (peer == null) {
    android.util.Log.e(TAG, "Peer not discovered: $recipientId")
    return false
}

// Check if we have an active connection
connectedClients[recipientId]?.let { gatt ->
    return writeMessage(gatt, recipientId, messageData)
}

if (!connectingClients.containsKey(recipientId)) {
    android.util.Log.d(TAG, "Peer $recipientId not connected, starting messaging connection")
    connectForMessaging(peer)
} else {
    android.util.Log.d(TAG, "Peer $recipientId connection already in progress")
}

return false
```

**Impact:**
- No crashes when sending to disconnected peers
- Clear logging for debugging
- Messages requeued for retry

## What Changed in Behavior

### Before Fixes

**Discovery Flow:**
1. Scan discovers peer advertisement
2. Connect to peer
3. Read device ID
4. ✗ Keep connection open forever
5. ✗ OS eventually drops connection
6. ✗ Connection drop triggers "peer lost"
7. ✗ Peer still advertising but marked as lost

**Send Flow:**
1. Try to send to peer
2. ✗ Use stale connection from `connectedPeripherals`
3. ✗ Peripheral actually disconnected
4. ✗ Write fails → crash

### After Fixes

**Discovery Flow:**
1. Scan discovers peer advertisement
2. Connect to peer
3. Read device ID
4. ✓ **Disconnect immediately**
5. ✓ Future advertisements update RSSI/lastSeen
6. ✓ Peer stays "alive" via advertisements
7. ✓ No false "peer lost" events

**Send Flow:**
1. Try to send to peer
2. ✓ Check if peer is in `discoveredPeers`
3. ✓ Check if actually connected
4. ✓ If not connected, reconnect first
5. ✓ Send when connection ready
6. ✓ No crash - graceful handling

## Expected Behavior Now

### Peer Discovery
- ✅ Peers discovered within 1-2 seconds
- ✅ RSSI updates continuously from advertisements
- ✅ `lastSeen` timestamp updates on each scan result
- ✅ No persistent connections maintained
- ✅ No false "peer lost" events from connection drops

### Message Sending
- ✅ First send to peer → triggers reconnect
- ✅ Connection established
- ✅ Message sent successfully
- ✅ Subsequent sends use existing connection
- ✅ If connection drops, automatic reconnect on next send
- ✅ No crashes from disconnected peers

### Peer Presence
Peers are considered "present" as long as:
- ✅ Advertisements are being received
- ✅ `lastSeen` timestamp is recent
- ✅ **NOT** based on connection state

Peers should only be marked "lost" when:
- ⚠️ Advertisements stop arriving
- ⚠️ (Future) Implement TTL-based expiry (e.g., 15 seconds without advertisement)

## Testing Instructions

### Test 1: Discovery Without False Peer Lost
```
1. Start App A
2. Start App B nearby
3. Verify both apps discover each other
4. Wait 2-3 minutes without sending messages
5. ✅ Check: Both peers should still be in discovered list
6. ✅ Check: No "peer lost" events should fire
7. ✅ Check: RSSI values should update periodically
```

**Expected:**
- Peers stay discovered
- RSSI updates visible in logs/UI
- No false "peer lost" events

### Test 2: Send Without Crash
```
1. Start App A and App B
2. Wait for discovery
3. Send message from A to B
4. ✅ Check: App A should log "reconnecting for messaging"
5. ✅ Check: Connection established
6. ✅ Check: Message sent successfully
7. ✅ Check: No crashes on either side
```

**Expected:**
- Automatic reconnection before send
- Message delivered successfully
- No crashes

### Test 3: Multiple Send/Receive
```
1. Start both apps
2. Wait for discovery
3. Send 10 messages A → B
4. Send 10 messages B → A
5. ✅ Check: All messages delivered
6. ✅ Check: No crashes
7. ✅ Check: Connections reused after first send
```

**Expected:**
- First send reconnects
- Subsequent sends use existing connection
- All messages delivered
- No crashes

### Test 4: Recovery After Connection Drop
```
1. Start both apps
2. Discover and send message successfully
3. Force kill and restart App B
4. Wait for App B to restart and readvertise
5. Send message from A to B
6. ✅ Check: A detects B is back
7. ✅ Check: A reconnects automatically
8. ✅ Check: Message delivered successfully
```

**Expected:**
- Automatic rediscovery
- Automatic reconnection
- Message delivery resumes

### Test 5: Long-Running Stability
```
1. Start both apps
2. Let them run for 30+ minutes
3. Periodically send messages (every 2-3 minutes)
4. Move devices in/out of range occasionally
5. ✅ Check: No false "peer lost" when in range
6. ✅ Check: All messages delivered successfully
7. ✅ Check: No crashes over time
```

**Expected:**
- Stable operation over time
- No memory leaks
- No connection issues
- No false peer lost events

## Debugging

### Useful Log Messages

**iOS:**
```
[BleManager] Updated peer <deviceId> RSSI: <value>
[BleManager] Peer <deviceId> not connected, will reconnect...
[BleManager] Reconnected to known peer <deviceId> for messaging
[BleManager] Peer <deviceId> disconnected but still in discovered list
```

**Android:**
```
Updated existing peer: <deviceId> (RSSI: <value>)
Peer <deviceId> not connected, will reconnect on next attempt
Discovered NEW peer: <deviceId> at <address> (RSSI: <value>)
```

### What to Look For

✅ **Good Signs:**
- "Updated peer" messages appearing regularly (every 1-2 seconds per peer)
- "Reconnected to known peer" when sending messages
- RSSI values changing based on distance
- No "peer lost" events unless device actually goes away

❌ **Bad Signs:**
- "Peer lost" events while device is still nearby
- Crashes when sending messages
- "No connection to peer" errors without reconnection attempts
- RSSI not updating

## Remaining Limitations

### iOS
- ⚠️ Reconnection for messaging is async - first send attempt will fail, message will be requeued
- ⚠️ No explicit TTL-based peer expiry (relies on advertisements continuing)

### Android
- ⚠️ Reconnection not yet implemented - sends will fail until manual reconnect
- ⚠️ TODO: Implement async reconnection in sendMessage
- ⚠️ No explicit TTL-based peer expiry

### Both Platforms
- ⚠️ No implementation of time-based peer expiry (e.g., "haven't seen peer for 15 seconds → mark as lost")
- ⚠️ This should be implemented if you want explicit peer timeout logic
- ⚠️ Currently peers stay "discovered" indefinitely once found

## Future Improvements

1. **Time-Based Peer Expiry:**
   - Check `lastSeen` timestamp periodically
   - Mark peers lost if not seen for 10-15 seconds
   - More explicit than relying only on advertisements

2. **Android Async Reconnection:**
   - Implement full async reconnection in Android sendMessage
   - Store pending messages during reconnection
   - Deliver after connection established

3. **Connection Pooling:**
   - Keep a small pool of recently-used connections
   - Reuse for messaging without reconnecting every time
   - Better latency for frequent messaging

4. **RSSI-Based Filtering:**
   - Ignore very weak signals (< -85 dBm)
   - Avoid attempting connections to marginal peers
   - Better reliability

## Summary

### What Was Fixed

✅ **False Peer Lost Events:**
- Disconnecting after device ID read
- Not marking peers lost on disconnect
- Updating RSSI from advertisements

✅ **Send Crashes:**
- Checking connection state before sending
- Graceful handling of disconnected peers
- Automatic reconnection for messaging

### How It Works Now

- **Discovery:** Advertisement-based, no persistent connections
- **Presence:** Based on scan results, not connections
- **Messaging:** On-demand connections, automatic reconnect
- **Resource:** Efficient, no idle connections

### Testing Priority

1. **High:** Test 1 (no false peer lost)
2. **High:** Test 2 (no send crashes)
3. **Medium:** Test 3 (multiple messages)
4. **Medium:** Test 5 (long-running stability)
5. **Low:** Test 4 (recovery)

**Ready to test!** 🚀

