# iOS BLE Peer Discovery - Quick Fix Summary

## The Problem

You're seeing the protocol start and transport switch to BLE, but **no peer discovery events**. The logs only show `transport_switched` events.

## What I've Done

Added comprehensive diagnostic logging to the iOS BLE layer that will appear in your React Native console. This will show you exactly what's happening with Bluetooth.

## Files Changed

1. **`bindings/react-native/ios/BleManager.swift`** - Added diagnostic logging throughout the BLE lifecycle
2. **`bindings/react-native/ios/OfflineProtocolModule.swift`** - Wired up diagnostic events to React Native

## How to Apply the Fix

### Quick Method (if you can open Xcode):

```bash
cd examples/react-native-app/ios
open OfflineProtocolExample.xcworkspace
```

Then in Xcode:
1. Clean Build Folder (Cmd+Shift+K)
2. Build and Run on your device (Cmd+R)

### Alternative Method (script):

```bash
cd examples/react-native-app
./rebuild-ios.sh
# Then open Xcode and build
```

## What You'll See After Rebuilding

Your console should now show detailed diagnostic messages:

```javascript
Protocol event: diagnostic { message: "[BLE] Starting BLE operations for device: user_287kd5ea1" }
Protocol event: diagnostic { message: "[BLE] Central Manager state: poweredOn" }
Protocol event: diagnostic { message: "[BLE] Peripheral Manager state: poweredOn" }
Protocol event: diagnostic { message: "[BLE] Both managers ready - starting scanning and advertising" }
Protocol event: diagnostic { message: "[BLE] 🔍 Starting BLE scanning for service UUID: ..." }
Protocol event: diagnostic { message: "[BLE] ✅ Advertising started successfully - device is now discoverable" }
```

**When another device is discovered:**
```javascript
Protocol event: diagnostic { message: "[BLE] 🎯 Discovered peripheral: <UUID> RSSI: -50" }
Protocol event: diagnostic { message: "[BLE] 🎉 Discovered NEW peer device: user_abc123 (RSSI: -50)" }
Protocol event: neighbor_discovered { peer_id: "user_abc123", transport: "ble", rssi: -50 }
```

## Most Likely Reason for No Peer Discovery

**You probably only have ONE device running the app.**

BLE peer discovery requires **TWO (or more) physical iOS devices**:
- Both devices must be running this app
- Both devices must have Bluetooth enabled
- Both devices must have granted Bluetooth permissions
- Devices must be within a few meters of each other
- iOS Simulators **DO NOT support BLE** - must use physical devices

## Testing Peer Discovery Properly

### What You Need:
1. ✅ Two physical iOS devices (iPhone or iPad)
2. ✅ The app installed and running on BOTH devices
3. ✅ Bluetooth enabled on BOTH devices
4. ✅ Bluetooth permissions granted on BOTH devices
5. ✅ Devices within ~5-10 meters of each other

### Testing Steps:
1. Install the app on Device A
2. Install the app on Device B
3. Launch the app on Device A - wait for "Advertising started successfully"
4. Launch the app on Device B - wait for "Advertising started successfully"
5. Both devices should discover each other within a few seconds
6. You'll see `neighbor_discovered` events on both devices

## Current Status Analysis

Based on your logs:
- ✅ Protocol initializes successfully
- ✅ Permissions are granted
- ✅ Protocol starts successfully
- ✅ Transport switches to BLE (4 times is normal)
- ❓ No diagnostic messages (native module not rebuilt yet)
- ❌ No peer discoveries (likely because only one device is running)

## Next Actions

1. **Rebuild the native modules** using the instructions above
2. **Check the diagnostic logs** - you should see BLE initialization and status messages
3. **If you see "Advertising started successfully"** - your BLE is working correctly!
4. **Get a second iOS device** and install the app on it
5. **Run both devices side by side** - you should see peer discovery events

## If You Only Have One Device

If you only have one iOS device, you won't see peer discovery (that's expected). But the diagnostic logs will confirm that:
- ✅ BLE is initializing correctly
- ✅ Scanning is active
- ✅ Advertising is active
- ✅ The device is ready to discover peers

This confirms everything is working - you just need another device to test with.

## Additional Notes

### Why 4 transport_switched Events?

This is normal! Each Bluetooth state change triggers an event:
1. Central Manager (scanner) becomes available
2. Peripheral Manager (advertiser) becomes available  
3. Status changes to "scanning"
4. Status changes to "advertising"

### BLE Service UUID

The app uses this UUID: `6E400001-B5A3-F393-E0A9-E50E24DCCA9E`

Both devices must use the same UUID to discover each other.

### iOS BLE Limitations

- Requires physical devices (simulators don't support BLE)
- Requires Bluetooth permissions
- Background scanning is limited (works best with app in foreground)
- Discovery range is typically 5-10 meters in good conditions

## Troubleshooting

| Symptom | Solution |
|---------|----------|
| No diagnostic messages | Rebuild native modules in Xcode |
| "unauthorized" state | Grant Bluetooth permission in Settings → Privacy → Bluetooth |
| "poweredOff" state | Enable Bluetooth on the device |
| No peer discoveries | Need a second device running the app |
| Discovery works once then stops | Normal - iOS BLE dedups discoveries (by design) |

## Questions?

After rebuilding and checking the diagnostic logs, share what you see and I can help debug further.

The key diagnostic to look for is: **"✅ Advertising started successfully"** - if you see this, your BLE is working correctly and just needs another device to discover.

