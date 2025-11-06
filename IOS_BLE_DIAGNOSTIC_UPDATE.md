# iOS BLE Peer Discovery Diagnostic Update

## Summary

I've added comprehensive diagnostic logging to the iOS BLE implementation that will show up in your React Native console. This will help us understand why peer discovery isn't working.

## Changes Made

### 1. Enhanced BleManager.swift

Added a new `onDiagnostic` callback that emits detailed diagnostic messages throughout the BLE lifecycle:

- **Initialization**: When Central Manager (scanner) and Peripheral Manager (advertiser) are created
- **State Changes**: Detailed Bluetooth state transitions (poweredOn, poweredOff, unauthorized, etc.)
- **Readiness**: When managers become ready and operations start
- **Scanning**: When BLE scanning starts with the service UUID
- **Advertising**: When BLE advertising starts successfully
- **Discovery**: When peripherals are discovered and when new peers are identified

### 2. OfflineProtocolModule.swift

Connected the diagnostic callback to emit `diagnostic` events to React Native that will appear in your console.

## How to Test

### Option 1: Rebuild in Xcode (Recommended)

1. Open Xcode:
   ```bash
   cd examples/react-native-app/ios
   open OfflineProtocolExample.xcworkspace
   ```

2. Clean build folder: **Product → Clean Build Folder** (Cmd+Shift+K)

3. Build and run on your physical device (Cmd+R)

### Option 2: Rebuild via CLI (if device is already running)

```bash
cd examples/react-native-app/ios
xcodebuild -workspace OfflineProtocolExample.xcworkspace \
  -scheme OfflineProtocolExample \
  -configuration Debug \
  -destination 'platform=iOS,id=YOUR_DEVICE_UUID' \
  clean build
```

Then reload the app on your device.

### Option 3: Full npm rebuild

```bash
cd examples/react-native-app
# Kill any running Metro bundler
pkill -f "node.*metro"
# Clean
rm -rf ios/build
# Reinstall pods
cd ios && pod install && cd ..
# Rebuild - use Xcode since CLI has simulator issues
```

## What You Should See

After rebuilding and running, you should see diagnostic messages in your React Native console like:

```
Protocol event: diagnostic { message: "[BLE] Starting BLE operations for device: user_287kd5ea1", ... }
Protocol event: diagnostic { message: "[BLE] Initializing Central Manager (scanner)", ... }
Protocol event: diagnostic { message: "[BLE] Initializing Peripheral Manager (advertiser)", ... }
Protocol event: diagnostic { message: "[BLE] Central Manager state: poweredOn", ... }
Protocol event: diagnostic { message: "[BLE] Peripheral Manager state: poweredOn", ... }
Protocol event: diagnostic { message: "[BLE] Both managers ready - starting scanning and advertising", ... }
Protocol event: diagnostic { message: "[BLE] 🔍 Starting BLE scanning for service UUID: 6E400001-B5A3-F393-E0A9-E50E24DCCA9E", ... }
Protocol event: diagnostic { message: "[BLE] 📡 Starting BLE advertising with service UUID: ...", ... }
Protocol event: diagnostic { message: "[BLE] ✅ Advertising started successfully - device is now discoverable", ... }
```

**When a peer is discovered:**
```
Protocol event: diagnostic { message: "[BLE] 🎯 Discovered peripheral: <UUID> RSSI: -50", ... }
Protocol event: diagnostic { message: "[BLE] 🎉 Discovered NEW peer device: user_abc123 (RSSI: -50)", ... }
Protocol event: neighbor_discovered { peer_id: "user_abc123", transport: "ble", rssi: -50, ... }
```

## Troubleshooting

### If you see no diagnostic messages at all:
- The native module didn't rebuild. Try cleaning and rebuilding in Xcode.

### If you see initialization but Bluetooth state is not "poweredOn":
- Check that Bluetooth is enabled on your device
- Check that the app has Bluetooth permissions in Settings → Privacy → Bluetooth

### If you see scanning/advertising started but no peer discoveries:
- **This is expected if there's only one device** - you need TWO devices running the app to test peer discovery
- Make sure both devices are close together (within a few meters)
- Make sure Bluetooth is enabled on both devices
- Make sure both devices have granted Bluetooth permissions to the app

### If you see "unauthorized" state:
- Go to Settings → Privacy → Bluetooth → [Your App] and enable access
- Uninstall and reinstall the app to trigger permission prompts again

## Testing Peer Discovery

To properly test peer discovery, you need:

1. **Two physical iOS devices** (simulators don't support BLE)
2. Both devices running the same app with the example code
3. Both devices with Bluetooth enabled
4. Both devices with Bluetooth permissions granted
5. Devices within a few meters of each other

The BLE service UUID being used is: `6E400001-B5A3-F393-E0A9-E50E24DCCA9E`

## Next Steps

1. Rebuild the app in Xcode
2. Run it on your device
3. Share the diagnostic logs you see in the console
4. If you have a second iOS device, install the app on it too and bring them close together
5. You should see peer discovery events when both devices are running

## Technical Details

### BLE Architecture

The iOS implementation uses two CoreBluetooth managers:

- **CBCentralManager** (scanner): Scans for peripherals advertising the service UUID
- **CBPeripheralManager** (advertiser): Advertises the service UUID to make the device discoverable

Both must be in the `poweredOn` state before operations can start.

### Discovery Flow

1. Device A advertises with service UUID
2. Device B scans and discovers Device A's peripheral
3. Device B connects to Device A
4. Device B reads the device ID characteristic from Device A
5. Device B emits a `neighbor_discovered` event
6. The process happens in reverse for Device A to discover Device B

### Why Multiple transport_switched Events?

You're seeing 4 `transport_switched` events because:
1. Central Manager becomes available
2. Peripheral Manager becomes available
3. Scanning starts (status changes to .scanning)
4. Advertising starts (status changes to .advertising)

This is normal behavior. Each state change triggers the event.

