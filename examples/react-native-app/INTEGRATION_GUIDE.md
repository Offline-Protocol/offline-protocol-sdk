# Integration Guide: Offline Protocol React Native SDK

This guide walks you through integrating the Offline Protocol SDK into a new or existing React Native application. For internal development, this shows how to use the local SDK binding from the repository.

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Installation](#installation)
3. [iOS Configuration](#ios-configuration)
4. [Android Configuration](#android-configuration)
5. [Basic Integration](#basic-integration)
6. [Advanced Features](#advanced-features)
7. [Best Practices](#best-practices)
8. [Common Pitfalls](#common-pitfalls)

## Prerequisites

- React Native 0.70.0 or higher
- Node.js 20 or higher
- iOS 12.0+ or Android API 21+
- Physical devices for BLE and Wi-Fi Direct testing

## Installation

### Using Local SDK (Development)

For internal development or testing unreleased SDK changes:

1. **Add to package.json:**
   ```json
   {
     "dependencies": {
       "@offlineprotocol/react-native": "file:../../bindings/react-native"
     }
   }
   ```

2. **Install dependencies:**
   ```bash
   npm install
   ```

### Using Published Package (Production)

For production apps:

```bash
npm install @offlineprotocol/react-native
```

## iOS Configuration

### 1. Install CocoaPods

```bash
cd ios
LANG=en_US.UTF-8 pod install
cd ..
```

The `use_native_modules!` in your Podfile will automatically link the SDK.

### 2. Configure Info.plist

Add required permissions to `ios/YourApp/Info.plist`:

```xml
<!-- Bluetooth permissions -->
<key>NSBluetoothAlwaysUsageDescription</key>
<string>This app uses Bluetooth to communicate with nearby devices when offline</string>

<key>NSBluetoothPeripheralUsageDescription</key>
<string>This app uses Bluetooth to communicate with nearby devices when offline</string>

<!-- Location permission (required for BLE scanning) -->
<key>NSLocationWhenInUseUsageDescription</key>
<string>This app needs location access to discover nearby devices for offline messaging</string>

<!-- Local network permission -->
<key>NSLocalNetworkUsageDescription</key>
<string>This app uses local network to discover and communicate with nearby devices</string>

<!-- Bonjour services -->
<key>NSBonjourServices</key>
<array>
    <string>_offlineprotocol._tcp</string>
</array>
```

### 3. Verify Podfile

Ensure your `ios/Podfile` includes:

```ruby
platform :ios, min_ios_version_supported
prepare_react_native_project!

target 'YourApp' do
  config = use_native_modules!
  
  use_react_native!(
    :path => config[:reactNativePath],
    :app_path => "#{Pod::Config.instance.installation_root}/.."
  )
  
  # ... rest of configuration
end
```

### 4. Build

```bash
npm run ios
```

## Android Configuration

### 1. Update AndroidManifest.xml

Add required permissions to `android/app/src/main/AndroidManifest.xml`:

```xml
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    
    <!-- Internet permission -->
    <uses-permission android:name="android.permission.INTERNET" />
    
    <!-- Bluetooth permissions -->
    <uses-permission android:name="android.permission.BLUETOOTH" />
    <uses-permission android:name="android.permission.BLUETOOTH_ADMIN" />
    <uses-permission android:name="android.permission.BLUETOOTH_CONNECT" />
    <uses-permission android:name="android.permission.BLUETOOTH_SCAN" />
    <uses-permission android:name="android.permission.BLUETOOTH_ADVERTISE" />
    
    <!-- Location permissions (required for BLE scanning) -->
    <uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" />
    <uses-permission android:name="android.permission.ACCESS_COARSE_LOCATION" />
    
    <!-- Wi-Fi Direct permissions -->
    <uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
    <uses-permission android:name="android.permission.CHANGE_WIFI_STATE" />
    <uses-permission android:name="android.permission.CHANGE_NETWORK_STATE" />
    <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
    <uses-permission android:name="android.permission.NEARBY_WIFI_DEVICES" />
    
    <!-- Features -->
    <uses-feature android:name="android.hardware.bluetooth" android:required="false" />
    <uses-feature android:name="android.hardware.bluetooth_le" android:required="false" />
    <uses-feature android:name="android.hardware.wifi.direct" android:required="false" />
    
    <application ...>
        <!-- Your app configuration -->
    </application>
</manifest>
```

### 2. Verify build.gradle

Ensure your `android/app/build.gradle` includes:

```gradle
apply plugin: "com.android.application"
apply plugin: "org.jetbrains.kotlin.android"
apply plugin: "com.facebook.react"

react {
    autolinkLibrariesWithApp()
}

android {
    // ... your configuration
}

dependencies {
    implementation("com.facebook.react:react-android")
    // ... other dependencies
}
```

The `autolinkLibrariesWithApp()` call automatically links the SDK.

### 3. Build

```bash
npm run android
```

## Basic Integration

### 1. Import the SDK

```typescript
import {
  OfflineProtocol,
  MessagePriority,
  type ProtocolConfig,
  type ProtocolEvent,
} from '@offlineprotocol/react-native';
```

### 2. Create a Custom Hook (Recommended)

Create `src/hooks/useOfflineProtocol.ts`:

```typescript
import { useEffect, useState, useCallback, useRef } from 'react';
import {
  OfflineProtocol,
  ProtocolConfig,
  ProtocolEvent,
  MessagePriority,
} from '@offlineprotocol/react-native';

export function useOfflineProtocol(config: ProtocolConfig) {
  const [protocol, setProtocol] = useState<OfflineProtocol | null>(null);
  const [isStarted, setIsStarted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [events, setEvents] = useState<ProtocolEvent[]>([]);
  const protocolRef = useRef<OfflineProtocol | null>(null);

  // Initialize protocol
  useEffect(() => {
    try {
      const instance = new OfflineProtocol(config);
      protocolRef.current = instance;
      setProtocol(instance);

      // Listen to all events
      instance.on('all', (event) => {
        setEvents((prev) => [event, ...prev].slice(0, 100));
      });

      return () => {
        instance.destroy().catch(console.error);
      };
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to initialize');
    }
  }, [config.appId, config.userId]);

  const start = useCallback(async () => {
    if (!protocolRef.current) return;
    try {
      await protocolRef.current.start();
      setIsStarted(true);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to start');
    }
  }, []);

  const stop = useCallback(async () => {
    if (!protocolRef.current) return;
    try {
      await protocolRef.current.stop();
      setIsStarted(false);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to stop');
    }
  }, []);

  const sendMessage = useCallback(
    async (recipient: string, content: string, priority: MessagePriority) => {
      if (!protocolRef.current || !isStarted) return null;
      try {
        return await protocolRef.current.sendMessage({
          recipient,
          content,
          priority,
        });
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to send');
        return null;
      }
    },
    [isStarted]
  );

  return { protocol, isStarted, error, events, start, stop, sendMessage };
}
```

### 3. Use in Your App

```typescript
import React from 'react';
import { View, Text, Button } from 'react-native';
import { MessagePriority } from '@offlineprotocol/react-native';
import { useOfflineProtocol } from './hooks/useOfflineProtocol';

export default function App() {
  const { isStarted, start, stop, sendMessage } = useOfflineProtocol({
    appId: 'my-app',
    userId: 'user123',
    transport: {
      bleEnabled: true,
      internetEnabled: true,
    },
  });

  const handleSend = async () => {
    const messageId = await sendMessage(
      'user456',
      'Hello!',
      MessagePriority.Medium
    );
    console.log('Message sent:', messageId);
  };

  return (
    <View>
      <Text>Status: {isStarted ? 'Started' : 'Stopped'}</Text>
      <Button
        title={isStarted ? 'Stop' : 'Start'}
        onPress={isStarted ? stop : start}
      />
      <Button
        title="Send Message"
        onPress={handleSend}
        disabled={!isStarted}
      />
    </View>
  );
}
```

## Advanced Features

### Event Handling

Handle specific events:

```typescript
useEffect(() => {
  if (!protocol) return;

  // Handle message received
  protocol.on('message_received', (event) => {
    console.log(`From ${event.sender}: ${event.content}`);
    // Update UI, show notification, etc.
  });

  // Handle transport switching
  protocol.on('transport_switched', (event) => {
    console.log(`Transport: ${event.from} → ${event.to}`);
    // Update connection indicator
  });

  // Handle relay promotion
  protocol.on('relay_promoted', (event) => {
    console.log('Device is now a relay');
    // Show relay status
  });

  return () => {
    protocol.removeAllListeners();
  };
}, [protocol]);
```

### Configuration Options

```typescript
const config: ProtocolConfig = {
  // Required
  appId: 'my-app-id',
  userId: 'current-user-id',

  // Transport configuration
  transport: {
    bleEnabled: true,          // Enable Bluetooth Low Energy
    wifiDirectEnabled: true,   // Enable Wi-Fi Direct (Android)
    internetEnabled: true,     // Enable Internet connectivity
  },

  // DORS (Dynamic Offline Routing Strategy) configuration
  dors: {
    preferOnline: true,        // Prefer online routes when available
  },

  // Relay configuration
  relay: {
    allowRelay: true,          // Allow device to act as relay
    minBatteryForRelay: 20,    // Minimum battery % to be relay
    relayThreshold: 3,         // Connection count threshold
  },

  // Network configuration
  network: {
    initialTtl: 10,            // Initial time-to-live for messages
  },
};
```

### Runtime Permission Handling

```typescript
import { PermissionsAndroid, Platform } from 'react-native';

async function requestPermissions() {
  if (Platform.OS === 'android') {
    const granted = await PermissionsAndroid.requestMultiple([
      PermissionsAndroid.PERMISSIONS.BLUETOOTH_CONNECT,
      PermissionsAndroid.PERMISSIONS.BLUETOOTH_SCAN,
      PermissionsAndroid.PERMISSIONS.ACCESS_FINE_LOCATION,
    ]);

    return Object.values(granted).every(
      (status) => status === PermissionsAndroid.RESULTS.GRANTED
    );
  }
  return true;
}

// Use before starting protocol
const hasPermissions = await requestPermissions();
if (hasPermissions) {
  await protocol.start();
}
```

## Best Practices

### 1. Error Handling

Always handle errors:

```typescript
try {
  await protocol.start();
} catch (error) {
  console.error('Failed to start protocol:', error);
  // Show user-friendly error message
  Alert.alert('Error', 'Failed to start offline protocol');
}
```

### 2. Lifecycle Management

Properly clean up:

```typescript
useEffect(() => {
  return () => {
    protocol?.destroy();
  };
}, [protocol]);
```

### 3. Event Listener Management

Remove listeners when not needed:

```typescript
useEffect(() => {
  const handler = (event) => {
    console.log('Message:', event);
  };

  protocol?.on('message_received', handler);

  return () => {
    protocol?.off('message_received', handler);
  };
}, [protocol]);
```

### 4. State Management

Use a custom hook or state management library:

```typescript
// Option 1: Custom hook (recommended for simple apps)
const { isStarted, events } = useOfflineProtocol(config);

// Option 2: Redux/Zustand (for complex apps)
// Store protocol state in global state
```

### 5. Type Safety

Use TypeScript for type safety:

```typescript
import type {
  MessageReceivedEvent,
  TransportSwitchedEvent,
} from '@offlineprotocol/react-native';

protocol.on('message_received', (event: MessageReceivedEvent) => {
  // event is fully typed
  console.log(event.sender, event.content);
});
```

## Common Pitfalls

### Not Requesting Permissions

**Problem:**
```typescript
await protocol.start(); // May fail without permissions
```

**Solution:**
```typescript
await requestPermissions();
await protocol.start();
```

### Creating Multiple Instances

**Problem:**
```typescript
const protocol1 = new OfflineProtocol(config);
const protocol2 = new OfflineProtocol(config); // Don't do this!
```

**Solution:**
```typescript
// Use a single instance throughout the app
// Manage it with a hook or context
```

### Not Cleaning Up

**Problem:**
```typescript
useEffect(() => {
  const p = new OfflineProtocol(config);
  p.start();
  // No cleanup!
}, []);
```

**Solution:**
```typescript
useEffect(() => {
  const p = new OfflineProtocol(config);
  p.start();
  
  return () => {
    p.destroy();
  };
}, []);
```

### Sending Messages Before Starting

**Problem:**
```typescript
const protocol = new OfflineProtocol(config);
await protocol.sendMessage({ ... }); // Protocol not started!
```

**Solution:**
```typescript
const protocol = new OfflineProtocol(config);
await protocol.start();
await protocol.sendMessage({ ... });
```

### Ignoring Events

**Problem:**
```typescript
// Not listening to events means missing important updates
```

**Solution:**
```typescript
protocol.on('all', (event) => {
  console.log('Event:', event.type);
  // Handle events appropriately
});
```

### Hardcoding User IDs

**Problem:**
```typescript
const config = {
  appId: 'my-app',
  userId: 'user123', // Same for all users!
};
```

**Solution:**
```typescript
const config = {
  appId: 'my-app',
  userId: getCurrentUserId(), // Unique per user
};
```

## Testing Checklist

Before deploying:

- [ ] Tested on physical iOS device
- [ ] Tested on physical Android device
- [ ] Verified all permissions are requested
- [ ] Tested with internet enabled
- [ ] Tested with internet disabled (BLE only)
- [ ] Tested message sending between devices
- [ ] Verified event handling works
- [ ] Tested protocol start/stop lifecycle
- [ ] Checked for memory leaks
- [ ] Reviewed error handling

## Next Steps

1. **Explore the example app** to see a complete implementation
2. **Review the API reference** for detailed method documentation
3. **Check the architecture docs** to understand SDK internals
4. **Join the community** for support and discussions

## Support

For issues or questions:
- Check this integration guide
- Review the [example app](./README.md)
- Read the [API reference](../../docs/api-reference.md)
- Open an issue on GitHub

## Resources

- [Example App](./README.md)
- [API Reference](../../docs/api-reference.md)
- [Architecture](../../docs/architecture.md)
- [SDK Binding README](../../bindings/react-native/README.md)

