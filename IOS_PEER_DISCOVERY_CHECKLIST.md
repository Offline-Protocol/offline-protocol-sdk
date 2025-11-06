# iOS Peer Discovery - Diagnostic Checklist

## Step 1: Rebuild Native Modules ✓

**Option A: Using Xcode (Recommended)**
```bash
cd examples/react-native-app/ios
open OfflineProtocolExample.xcworkspace
```
In Xcode:
- Clean Build Folder: `Product → Clean Build Folder` (Cmd+Shift+K)
- Build and Run: `Product → Run` (Cmd+R)

**Option B: Using Script**
```bash
cd examples/react-native-app
./rebuild-ios.sh
# Then open Xcode and build/run
```

## Step 2: What to Look For in Console

After rebuilding, your console should show diagnostic messages with 🔍 emoji:

### Expected Startup Logs:
```
🔍 [BLE] Starting BLE operations for device: user_287kd5ea1
🔍 [BLE] Initializing Central Manager (scanner)
🔍 [BLE] Initializing Peripheral Manager (advertiser)
🔍 [BLE] Central Manager state: poweredOn
🔍 [BLE] Peripheral Manager state: poweredOn
🔍 [BLE] Both managers ready - starting scanning and advertising
🔍 [BLE] 🔍 Starting BLE scanning for service UUID: 6E400001-B5A3-F393-E0A9-E50E24DCCA9E
🔍 [BLE] 📡 Starting BLE advertising with service UUID: 6E400001-B5A3-F393-E0A9-E50E24DCCA9E
🔍 [BLE] ✅ Advertising started successfully - device is now discoverable
```

### If You See This: ✅ Your BLE Implementation is Working!

The device is now:
- ✅ Scanning for other devices
- ✅ Advertising itself to be discovered
- ✅ Ready to discover peers

### If Another Device is Nearby:
```
🔍 [BLE] 🎯 Discovered peripheral: 1234-5678-ABCD RSSI: -45
🔍 [BLE] 🎉 Discovered NEW peer device: user_abc123 (RSSI: -45)
Protocol event: neighbor_discovered { peer_id: "user_abc123", transport: "ble", rssi: -45, ... }
```

## Step 3: Diagnosis

### ❌ If You See NO 🔍 Messages

**Problem:** Native modules didn't rebuild

**Solution:**
1. Close the app completely
2. In Xcode: Clean Build Folder (Cmd+Shift+K)
3. Delete derived data: `rm -rf ~/Library/Developer/Xcode/DerivedData/OfflineProtocolExample-*`
4. Rebuild and run again

### ⚠️ If You See: `[BLE] Central Manager state: unauthorized`

**Problem:** Bluetooth permission not granted

**Solution:**
1. Go to: Settings → Privacy & Security → Bluetooth → OfflineProtocolExample
2. Enable Bluetooth access
3. Restart the app

### ⚠️ If You See: `[BLE] Central Manager state: poweredOff`

**Problem:** Bluetooth is disabled

**Solution:**
1. Enable Bluetooth in Control Center or Settings
2. Restart the app

### ✅ If You See: "Advertising started successfully" BUT No Peer Discoveries

**This is NORMAL!** 

You need **TWO physical iOS devices** to test peer discovery:

#### Requirements for Peer Discovery:
- [ ] Device 1: Running the app with Bluetooth enabled
- [ ] Device 2: Running the app with Bluetooth enabled
- [ ] Both devices have granted Bluetooth permission
- [ ] Both devices are within 5-10 meters
- [ ] Both devices are showing "Advertising started successfully"

#### Single Device Test (What You Can Verify):
Even with one device, you can verify:
- ✅ BLE initializes correctly
- ✅ Permissions are granted
- ✅ Bluetooth is enabled
- ✅ Scanning is active
- ✅ Advertising is active
- ✅ Device is **ready** to discover peers
- ❓ No peers discovered (expected - need another device!)

## Step 4: Two Device Test

Once you have TWO devices:

### Device A (First Device):
1. Launch app
2. Wait for: `🔍 [BLE] ✅ Advertising started successfully`
3. Keep app in foreground
4. Device A is now discoverable

### Device B (Second Device):
1. Launch app
2. Wait for: `🔍 [BLE] ✅ Advertising started successfully`
3. Within a few seconds, you should see:
   ```
   🔍 [BLE] 🎯 Discovered peripheral: ...
   🔍 [BLE] 🎉 Discovered NEW peer device: [Device A's ID]
   Protocol event: neighbor_discovered
   ```

### Device A Should Also Discover Device B:
```
🔍 [BLE] 🎯 Discovered peripheral: ...
🔍 [BLE] 🎉 Discovered NEW peer device: [Device B's ID]
Protocol event: neighbor_discovered
```

### Expected Result:
Both devices discover each other and you'll see:
- `neighbor_discovered` events on both devices
- The discovered peer appears in the Neighbors list
- Connection is established

## Step 5: Troubleshooting Two-Device Setup

### Devices Not Discovering Each Other:

1. **Verify Both Devices Show "Advertising started successfully"**
   - If not, check Bluetooth permissions and state on each device

2. **Bring Devices Closer** (within 1-2 meters)
   - BLE range can be affected by obstacles and interference

3. **Ensure Apps are in Foreground**
   - iOS limits background BLE scanning
   - Keep both apps open and active

4. **Check Bluetooth Interference**
   - Turn off other Bluetooth devices nearby
   - Move away from WiFi routers or microwaves

5. **Restart Both Apps**
   - Kill and relaunch both apps
   - Wait for "Advertising started" on both

6. **Different Device IDs**
   - Verify each device has a unique `userId` in the config
   - Check console: "Starting BLE operations for device: [ID]"
   - Device IDs must be different for discovery to work

## Common Scenarios

### ✅ Scenario 1: Everything Works
```
🔍 [BLE] Starting BLE operations for device: user_abc
🔍 [BLE] Central Manager state: poweredOn
🔍 [BLE] Peripheral Manager state: poweredOn
🔍 [BLE] ✅ Advertising started successfully
🔍 [BLE] 🎯 Discovered peripheral: ...
🔍 [BLE] 🎉 Discovered NEW peer device: user_xyz
Protocol event: neighbor_discovered
```
**Result:** ✅ Peer discovery working!

### ⚠️ Scenario 2: Single Device
```
🔍 [BLE] Starting BLE operations for device: user_abc
🔍 [BLE] ✅ Advertising started successfully
[no further discoveries]
```
**Result:** ✅ BLE working, ❓ No peers nearby (expected)

### ❌ Scenario 3: Permission Denied
```
🔍 [BLE] Starting BLE operations for device: user_abc
🔍 [BLE] Central Manager state: unauthorized
```
**Result:** ❌ Need to grant Bluetooth permission

### ❌ Scenario 4: Bluetooth Off
```
🔍 [BLE] Starting BLE operations for device: user_abc
🔍 [BLE] Central Manager state: poweredOff
```
**Result:** ❌ Need to enable Bluetooth

## Summary

The diagnostic logging will show you **exactly** what's happening with BLE. Most likely, your BLE implementation is working correctly but you just need a second device to test peer discovery.

**Key Indicator of Success:**
```
🔍 [BLE] ✅ Advertising started successfully - device is now discoverable
```

If you see this message, your BLE is working and ready to discover peers!

## Questions After Testing?

After rebuilding and checking the console:

1. Do you see any 🔍 diagnostic messages?
2. What is the BLE state? (poweredOn, unauthorized, poweredOff?)
3. Do you see "Advertising started successfully"?
4. How many devices are you testing with?
5. If testing with two devices, do you see "Discovered peripheral" messages?

Share the console output and I can help debug further!

