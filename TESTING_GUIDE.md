# BLE Manager Testing Guide

## ✅ Implementation Status

### What's Working
- ✅ iOS BLE Manager - Fully implemented and tested
- ✅ Android BLE Manager - Fully implemented
- ✅ Config transformation - Fixed (nested → flat)
- ✅ Native module integration - Complete
- ✅ TypeScript bindings - Updated
- ✅ Active transport display - Shows "BLE" when neighbors discovered

### Current Test Results
- **iOS**: ✅ Started successfully, event system working
- **Android**: ⚠️ Needs fresh install to pick up new native libraries

## Quick Start Testing

### Step 1: iOS Testing

```bash
cd examples/react-native-app

# Clean and reinstall pods
cd ios
rm -rf Pods Podfile.lock
pod install
cd ..

# Run on iOS device (simulator won't work for BLE)
npm run ios -- --device
```

**What to watch for:**
- Console log: `[BleManager] Starting BLE transport for device: ...`
- Console log: `[BleManager] Waiting for Bluetooth to power on...`
- Console log: `[BleManager] Starting scan...`
- Console log: `[BleManager] Starting advertising...`
- Event log: `network_metrics` event appears
- When second device nearby: `neighbor_discovered` event with `transport: "BLE"`

### Step 2: Android Testing

```bash
cd examples/react-native-app

# Kill metro bundler and restart fresh
# Ctrl+C to kill, then:
npm start -- --reset-cache

# In a NEW terminal:
# Clean everything
cd android
./gradlew clean
cd ..

# Uninstall old app from device
adb uninstall com.offlineprotocolexample

# Run on Android device
npm run android
```

**What to watch for:**
- LogCat: `BLE Manager initialized for user: ...`
- LogCat: `Starting BLE transport for device: ...`
- LogCat: `BLE Manager started successfully - scanning and advertising active`
- LogCat: `Started scanning for service: ...`
- LogCat: `Advertising started successfully`
- When second device nearby: `neighbor_discovered` event

### Step 3: Cross-Platform Testing (CRITICAL)

**Test iOS ↔ Android:**

1. Run app on iPhone (Device A)
2. Run app on Android phone (Device B)
3. Both devices tap "Start Protocol"
4. Both devices grant Bluetooth permissions
5. Wait 5-10 seconds
6. Check event logs for `neighbor_discovered` events
7. Note the peer's User ID from the event
8. Try sending a message to that User ID
9. Verify message appears on the other device

**Expected behavior:**
- Both devices discover each other within 5-10 seconds
- `neighbor_discovered` event shows `transport: "BLE"`
- Transport display changes from "None" to "BLE"
- Messages can be sent bidirectionally

## Troubleshooting

### iOS: BLE Not Starting

**Check logs for:**
```
[BleManager] Starting BLE transport for device: ...
[BleManager] Central state: 5  // 5 = poweredOn
[BleManager] Peripheral state: 5
```

**If not appearing:**
- Check Info.plist has Bluetooth usage descriptions
- Ensure Bluetooth is enabled in iOS Settings
- Grant Bluetooth permissions when prompted
- Check console for any permission errors

### Android: Missing Symbol Error

```
Error looking up function 'uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_emit_test_event': undefined symbol
```

**Solution:**
```bash
# The app has cached old native libraries
# Uninstall completely and reinstall:
adb uninstall com.offlineprotocolexample
cd android
./gradlew clean
cd ..
npm run android
```

### Android: BLE Not Starting

**Check LogCat for:**
```
BLE Manager initialized for user: ...
Starting BLE transport for device: ...
BLE Manager started successfully
```

**If errors appear:**
- `Bluetooth permissions not granted` → Grant permissions in app
- `Bluetooth is not enabled` → Enable Bluetooth in Android Settings
- `BluetoothLeAdvertiser not supported` → Some devices don't support BLE advertising

### Transport Shows "None"

**This is normal initially!** The transport will show "BLE" when:
1. A peer is discovered (neighbor_discovered event)
2. OR a transport_switched event is emitted

**If it stays "None" after discovering peers:**
- Check the neighbor_discovered event has a `transport` field
- Look in Events tab for the event details
- May indicate BLE manager isn't reporting properly

### No Peers Discovered

**Checklist:**
- ✅ Both devices have protocol started
- ✅ Both devices have Bluetooth enabled
- ✅ Both devices granted Bluetooth permissions
- ✅ Devices are within BLE range (< 10 meters recommended)
- ✅ Check console logs show "Started scanning" and "Advertising started"
- ✅ Try restarting both apps

**Debug steps:**
1. Check if BLE Manager started: Look for log messages
2. Verify permissions: Settings → App → Permissions
3. Check Bluetooth: Settings → Bluetooth (should be ON)
4. Check logs for errors: Any SecurityException or permission errors
5. Try with devices closer together (< 1 meter)

## Expected Console Output

### iOS Success

```
[OfflineProtocol] Native config: {"appId":"...","bleEnabled":true,...}
[OfflineProtocolModule] BLE Manager initialized for user: user_abc123
[OfflineProtocolModule] BLE Manager started
[BleManager] Starting BLE transport for device: user_abc123
[BleManager] Waiting for Bluetooth to power on...
[BleManager] Central state: 5
[BleManager] Starting scan for service: 6E400001-B5A3-F393-E0A9-E50E24DCCA9E
[BleManager] Peripheral state: 5
[BleManager] Starting advertising with service: 6E400001-B5A3-F393-E0A9-E50E24DCCA9E
[BleManager] GATT server configured
[BleManager] Fragment polling started
[BleManager] Advertising started successfully

// When peer discovered:
[BleManager] Discovered peripheral: xxx RSSI: -45
[BleManager] Connecting to peripheral: xxx
[BleManager] Connected to peripheral: xxx
[BleManager] Enabled notifications for message characteristic
[BleManager] Peer discovered: user_def456
```

### Android Success

```
BLE Manager initialized for user: user_abc123
BLE Manager started
Starting BLE transport for device: user_abc123
GATT server configured
Starting advertising with service: 6E400001-B5A3-F393-E0A9-E50E24DCCA9E
Started scanning for service: 6E400001-B5A3-F393-E0A9-E50E24DCCA9E
BLE Manager started successfully - scanning and advertising active
Advertising started successfully

// When peer discovered:
Discovered device: XX:XX:XX:XX:XX:XX RSSI: -45
Connecting to device: XX:XX:XX:XX:XX:XX
GATT client: Connected to XX:XX:XX:XX:XX:XX
Peer discovered: user_def456 at XX:XX:XX:XX:XX:XX
```

## Testing Checklist

### Single Device Tests
- [ ] iOS: Protocol starts without errors
- [ ] iOS: BLE manager logs appear
- [ ] iOS: Scanning and advertising confirmed
- [ ] Android: Protocol starts without errors
- [ ] Android: BLE manager logs appear
- [ ] Android: Scanning and advertising confirmed

### Two Device Tests (Same Platform)
- [ ] iOS ↔ iOS: Peer discovery works
- [ ] iOS ↔ iOS: Messages can be sent
- [ ] Android ↔ Android: Peer discovery works
- [ ] Android ↔ Android: Messages can be sent

### Cross-Platform Tests (CRITICAL)
- [ ] iOS ↔ Android: iPhone discovers Android
- [ ] iOS ↔ Android: Android discovers iPhone
- [ ] iOS ↔ Android: iPhone → Android message delivery
- [ ] iOS ↔ Android: Android → iPhone message delivery
- [ ] iOS ↔ Android: Bidirectional messaging works

### Multi-Device Tests
- [ ] 2 iOS + 1 Android: All discover each other
- [ ] 1 iOS + 2 Android: All discover each other
- [ ] Messages route through multi-hop network

## Next Steps After Testing

1. **If iOS works but Android doesn't:**
   - Check native library versions match
   - Verify permissions granted
   - Check device Bluetooth support

2. **If transport shows "None":**
   - Wait for peer discovery
   - Check Events tab for neighbor_discovered events
   - Verify events have `transport` field

3. **If no peers discovered:**
   - Move devices closer (< 1 meter for testing)
   - Check both have Bluetooth ON
   - Check both granted permissions
   - Restart both apps

4. **If cross-platform doesn't work:**
   - Verify UUIDs match exactly
   - Check both are advertising/scanning
   - Try iOS as initiator, then Android as initiator
   - Check for SecurityException or permission errors

## Success Criteria

✅ **BLE Manager is working when:**
- Logs show scanning and advertising started
- Peers are discovered automatically
- neighbor_discovered events appear
- Transport shows as "BLE"
- Messages can be sent between devices
- iOS ↔ Android communication works

Happy testing! 🎉

