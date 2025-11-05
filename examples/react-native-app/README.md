# Offline Protocol Example App

A complete React Native example application demonstrating the offline-protocol SDK. This app showcases all features of the SDK including messaging, network monitoring, event handling, and more.

## Overview

This example app is designed to help developers understand how to integrate and use the `@offlineprotocol/react-native` SDK in their own applications. It demonstrates:

- Protocol initialization and lifecycle management
- Sending and receiving messages with different priority levels
- Real-time event monitoring
- Network status and metrics visualization
- Transport switching (Internet, BLE, Wi-Fi Direct)
- Relay promotion/demotion
- Neighbor discovery
- Error handling

## Prerequisites

Before running this example app, ensure you have:

- **Node.js** (v20 or higher)
- **React Native development environment** set up
  - For iOS: Xcode, CocoaPods
  - For Android: Android Studio, Android SDK
- **Physical devices** recommended for testing BLE and Wi-Fi Direct features

## Installation

1. **Navigate to the example directory:**
   ```bash
   cd examples/react-native-app
   ```

2. **Install dependencies:**
   ```bash
   npm install
   ```

3. **iOS Setup:**
   ```bash
   cd ios
   LANG=en_US.UTF-8 pod install
   cd ..
   ```

4. **Android Setup:**
   No additional setup needed. The native libraries are automatically included.

## Running the App

### iOS

```bash
npm run ios
```

Or open `ios/OfflineProtocolExample.xcworkspace` in Xcode and run from there.

### Android

```bash
npm run android
```

Or open the `android` folder in Android Studio and run from there.

## Using the App

### 1. Start the Protocol

- The app generates a random User ID on launch (you can change it)
- Click the **"Start Protocol"** button to initialize the SDK
- The status bar will turn green when the protocol is running

### 2. Send Messages

- Navigate to the **"Messaging"** tab
- Enter a recipient User ID
- Type your message
- Select a priority level (Low, Medium, High, Critical)
- Click **"Send Message"**
- Messages will appear in the list below with their delivery status

### 3. Monitor Network Status

- Navigate to the **"Network"** tab
- View current transport being used
- Check if the device is acting as a relay
- See connected neighbors
- View network metrics (delivery ratio, latency, etc.)
- Review transport switch history

### 4. View Events

- Navigate to the **"Events"** tab
- See all protocol events in real-time
- Events are color-coded by type
- Click **"Clear"** to reset the event log

## Architecture

The app is organized into:

```
src/
├── App.tsx                    # Main app component with tabs
├── hooks/
│   └── useOfflineProtocol.ts # Custom hook for SDK management
├── components/
│   ├── StatusBar.tsx         # Connection status indicator
│   ├── EventLog.tsx          # Event display component
│   └── MessageList.tsx       # Message history component
└── screens/
    ├── MessagingScreen.tsx   # Message sending interface
    └── NetworkScreen.tsx     # Network metrics display
```

### Key Patterns

**Custom Hook (`useOfflineProtocol.ts`)**
- Encapsulates protocol initialization and lifecycle
- Manages events and state updates
- Provides clean API for components

**Event-Driven Architecture**
- All SDK events are captured and displayed
- Components react to event changes
- Event history maintained for analysis

**TypeScript Integration**
- Full type safety with SDK types
- Type-safe event handling
- IntelliSense support

## Features Demonstrated

### Protocol Configuration

The app configures the SDK with:

```typescript
{
  appId: 'offline-protocol-example',
  userId: 'user_xxxxx',
  transport: {
    bleEnabled: true,
    wifiDirectEnabled: true,
    internetEnabled: true,
  },
  dors: {
    preferOnline: true,
  },
  relay: {
    allowRelay: true,
    minBatteryForRelay: 20,
    relayThreshold: 3,
  },
  network: {
    initialTtl: 10,
  },
}
```

### Message Priorities

All four priority levels are supported:
- **Low** - Non-urgent messages
- **Medium** - Standard messages (default)
- **High** - Important messages
- **Critical** - Urgent, high-priority messages

### Event Types Handled

The app handles all SDK event types:

| Event Type | Description |
|------------|-------------|
| `message_sent` | Message successfully sent |
| `message_received` | Message received from another user |
| `message_delivered` | Message delivered to recipient |
| `message_failed` | Message delivery failed |
| `transport_switched` | Transport layer changed (e.g., Internet → BLE) |
| `relay_promoted` | Device promoted to relay node |
| `relay_demoted` | Device demoted from relay node |
| `neighbor_discovered` | New nearby device discovered |
| `neighbor_lost` | Connection to nearby device lost |
| `network_metrics` | Network performance metrics |
| `file_progress` | File transfer progress update |
| `file_received` | File transfer completed |

## Testing

### Single Device Testing

You can test basic functionality on a single device:
- Start the protocol
- Send messages to non-existent users (will show as failed)
- View events and network status changes

### Multi-Device Testing

For full testing, use two or more devices:

1. **Install on multiple devices**
2. **Note each device's User ID** (shown in the header)
3. **Start protocol on all devices**
4. **Send messages between devices** using their User IDs
5. **Observe:**
   - Transport switching
   - Neighbor discovery
   - Relay promotion
   - Message delivery

### Testing Scenarios

**Scenario 1: Internet Connectivity**
- Both devices have internet
- Messages should use internet transport
- Fast delivery with low hop count

**Scenario 2: Offline Mode**
- Disable internet on both devices
- Keep devices close together (BLE range)
- Messages should use BLE transport
- Observe neighbor discovery events

**Scenario 3: Relay Testing**
- Use 3+ devices
- Place some devices out of direct range
- Middle device should become relay
- Observe relay_promoted event

## Troubleshooting

### App Crashes on Android (Android 12+)

**Problem:** App crashes immediately on launch or when starting the protocol.

**Cause:** On Android 12+ (API 31+), Bluetooth permissions must be requested at runtime. If the app tries to use Bluetooth before permissions are granted, it will crash with a `SecurityException`.

**Solution:**

1. **Grant Permissions First:** When you tap "Start Protocol", the app will automatically request the required permissions. You must grant:
   - **Bluetooth** permissions (BLUETOOTH_SCAN, BLUETOOTH_CONNECT, BLUETOOTH_ADVERTISE)
   - **Location** permission (required by Android for BLE scanning)

2. **Permission Warning:** If you see a yellow warning banner saying "⚠️ Bluetooth permissions required", tap the "Grant Permissions" button before starting the protocol.

3. **Manual Permission Grant:** If you denied permissions, go to:
   - Settings → Apps → Offline Protocol Example → Permissions
   - Enable "Nearby devices" (or "Bluetooth") and "Location"

4. **Clean Reinstall:** If the app continues to crash:
   ```bash
   cd android
   ./gradlew clean
   cd ..
   npm run android
   ```

**Note:** Location permission is required by Android for BLE scanning but your location is never tracked or stored by this app.

### App Won't Build

**iOS:**
- Ensure CocoaPods are installed: `sudo gem install cocoapods`
- Clean build folder: `cd ios && xcodebuild clean && cd ..`
- Delete Pods: `cd ios && rm -rf Pods Podfile.lock && pod install && cd ..`

**Android:**
- Clean gradle cache: `cd android && ./gradlew clean && cd ..`
- Rebuild: `cd android && ./gradlew assembleDebug && cd ..`

### Protocol Won't Start

**Common Issues:**

1. **Permissions Not Granted** (Most Common)
   - Symptom: Error message about permissions, or app crashes
   - Solution: Tap "Grant Permissions" button and allow all permissions
   - On Android 12+, you need Bluetooth AND Location permissions

2. **Bluetooth Disabled**
   - Symptom: Error message "Bluetooth must be enabled"
   - Solution: Enable Bluetooth in your device settings
   - The app will prompt you automatically

3. **Invalid Configuration**
   - Symptom: "Protocol not initialized" error
   - Solution: Verify User ID is not empty and valid
   - Check error message in status bar for details

4. **Check Console Logs**
   - Run `npx react-native log-android` or `npx react-native log-ios`
   - Look for error messages from "OfflineProtocolModule"

### Messages Not Sending

- Ensure protocol is started (green status bar)
- Verify recipient ID is valid
- Check network connectivity
- Review event log for error details

### BLE Not Working

**iOS:**
- Grant Bluetooth permission in Settings
- Ensure Bluetooth is enabled
- Use a physical device (not simulator)
- The system will automatically prompt for Bluetooth permission when needed

**Android (especially Android 12+):**

1. **Grant All Required Permissions:**
   - Open the app and tap "Grant Permissions"
   - Allow "Nearby devices" or "Bluetooth" permission
   - Allow "Location" permission (required for BLE scanning)

2. **Enable Bluetooth:**
   - Make sure Bluetooth is turned ON in device settings
   - The app will prompt you if Bluetooth is disabled

3. **Enable Location Services:**
   - Settings → Location → Turn ON
   - Required for BLE scanning on Android (security requirement)

4. **Check Android Version:**
   - Android 12+ (API 31+): Uses new Bluetooth permissions
   - Android 10-11: Uses legacy Bluetooth + Location permissions
   - Best support on Android 12 and higher

5. **Test on Physical Device:**
   - BLE doesn't work reliably on emulators
   - Use two physical devices for testing

**Common Android BLE Issues:**

- **"SecurityException" in logs:** Permissions not granted → Restart app and grant permissions
- **"Bluetooth adapter not found":** Device doesn't support BLE → Use a different device
- **Scanning not finding devices:** Location services disabled → Enable in Settings

### No Neighbors Discovered

**Troubleshooting Steps:**

1. **Both Devices Running:**
   - Ensure protocol is started on BOTH devices (green status bar)
   - Both should show "Protocol started successfully" in logs

2. **Permissions Granted:**
   - Both devices need Bluetooth + Location permissions
   - Check for yellow warning banner on either device

3. **Bluetooth Enabled:**
   - Verify Bluetooth is ON on both devices
   - Try toggling Bluetooth OFF and ON again

4. **Proximity:**
   - Devices must be within BLE range (~10-30 meters / 30-100 feet)
   - Keep devices closer together initially (< 5 meters)
   - Remove obstacles between devices

5. **Wait for Discovery:**
   - BLE discovery can take 10-30 seconds
   - Check the "Events" tab for "neighbor_discovered" events

6. **Check Event Log:**
   - Go to "Events" tab on both devices
   - Look for scanning/advertising events
   - Any errors will appear here

7. **Restart Protocol:**
   - Stop protocol on both devices
   - Wait 5 seconds
   - Start again on both devices

## Development Notes

### Local SDK Binding

This app uses the local SDK binding from the repository:

```json
"@offlineprotocol/react-native": "file:../../bindings/react-native"
```

This allows testing SDK changes without publishing to npm.

### Rebuilding Native Code

If you modify the Rust SDK or native bindings:

1. **Rebuild iOS libraries:**
   ```bash
   cd ../../bindings/react-native
   npm run build:ios
   cd ../../examples/react-native-app
   cd ios && pod install && cd ..
   ```
   
   **Note**: The iOS build creates a library for arm64 (device) which works on both physical devices and Apple Silicon simulators. For Intel Mac simulators, you may need to build separately for x86_64.

2. **Rebuild Android libraries:**
   ```bash
   cd ../../bindings/react-native
   npm run build:android
   cd ../../examples/react-native-app
   ```
   
   This builds for all Android architectures: arm64-v8a, armeabi-v7a, x86, x86_64.

3. **Rebuild the app:**
   ```bash
   npm run ios    # or npm run android
   ```

## Next Steps

After exploring this example:

1. **Review the source code** to understand SDK integration
2. **Check the Integration Guide** (`INTEGRATION_GUIDE.md`) for step-by-step instructions
3. **Read the API Reference** in the SDK documentation
4. **Build your own app** using this example as a template

## Resources

- [Offline Protocol SDK Documentation](../../bindings/react-native/README.md)
- [Integration Guide](./INTEGRATION_GUIDE.md)
- [API Reference](../../docs/api-reference.md)
- [Architecture Overview](../../docs/architecture.md)

## Support

For issues or questions:
- Check the [Troubleshooting](#troubleshooting) section
- Review the [Integration Guide](./INTEGRATION_GUIDE.md)
- Open an issue on GitHub

## License

This example app is part of the Offline Protocol SDK and is licensed under MIT OR Apache-2.0.
