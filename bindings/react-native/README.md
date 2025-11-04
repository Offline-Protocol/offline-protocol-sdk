# @offlineprotocol/react-native

Offline-first messaging SDK with intelligent transport switching for React Native. Built with Rust for maximum performance and reliability.

## Features

- **Offline-First**: Messages delivered even without internet connectivity
- **Intelligent Transport Switching**: Automatically switches between Internet, BLE Mesh, and Wi-Fi Direct
- **Cross-Platform**: Works on iOS and Android
- **Type-Safe**: Full TypeScript support
- **Event-Driven**: Real-time event notifications
- **Plug & Play**: Pre-built binaries included - no Rust toolchain needed!

## Installation

```bash
npm install @offlineprotocol/react-native
# or
yarn add @offlineprotocol/react-native
```

### iOS Setup

```bash
cd ios && pod install
```

That's it! The pre-built library is automatically linked.

### Android Setup

No additional setup needed! The pre-built `.so` files are automatically included in your APK.

## Quick Start

```typescript
import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react-native';

// Create protocol instance
const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
});

// Listen for incoming messages
protocol.on('message_received', (event) => {
  console.log(`From ${event.sender}: ${event.content}`);
  console.log(`Delivered via ${event.transport} in ${event.hop_count} hops`);
});

// Listen for all events
protocol.on('all', (event) => {
  console.log('Event:', event.type, event);
});

// Start the protocol
await protocol.start();

// Send a message
const messageId = await protocol.sendMessage({
  recipient: 'user456',
  content: 'Hello offline world!',
  priority: MessagePriority.High,
});

console.log('Message sent:', messageId);

// Stop the protocol
await protocol.stop();

// Clean up
await protocol.destroy();
```

## API Reference

### `OfflineProtocol`

Main class for interacting with the SDK.

#### Constructor

```typescript
new OfflineProtocol(config: ProtocolConfig)
```

**Parameters:**

```typescript
interface ProtocolConfig {
  appId: string;                // Application identifier
  userId: string;               // User identifier
  transport?: {                 // Optional transport configuration
    bleEnabled?: boolean;       // Enable BLE transport (default: true)
    wifiDirectEnabled?: boolean; // Enable Wi-Fi Direct (Android only, default: true)
    internetEnabled?: boolean;  // Enable Internet transport (default: true)
  };
  dors?: {                      // DORS configuration
    preferOnline?: boolean;     // Prefer online mode (default: true)
  };
  relay?: {                     // Relay configuration
    allowRelay?: boolean;       // Allow device to act as relay (default: true)
    minBatteryForRelay?: number; // Min battery % for relaying (default: 30)
    relayThreshold?: number;    // Connections needed for relay (default: 3)
  };
  network?: {
    initialTtl?: number;        // Initial TTL for messages (default: 8)
  };
}
```

#### Methods

##### `start(): Promise<void>`

Starts the protocol.

```typescript
await protocol.start();
```

##### `stop(): Promise<void>`

Stops the protocol gracefully.

```typescript
await protocol.stop();
```

##### `sendMessage(params: SendMessageParams): Promise<string>`

Sends a message and returns the message ID.

```typescript
const messageId = await protocol.sendMessage({
  recipient: 'user456',
  content: 'Hello!',
  priority: MessagePriority.Medium, // Optional, defaults to Medium
});
```

##### `on(eventType: EventType | 'all', listener: EventListener): this`

Registers an event listener.

```typescript
protocol.on('message_received', (event) => {
  console.log('Received:', event);
});

// Listen to all events
protocol.on('all', (event) => {
  console.log('Any event:', event);
});
```

##### `off(eventType: EventType | 'all', listener: EventListener): this`

Removes an event listener.

```typescript
const handler = (event) => console.log(event);
protocol.on('message_sent', handler);
protocol.off('message_sent', handler);
```

##### `once(eventType: EventType | 'all', listener: EventListener): this`

Registers a one-time event listener.

```typescript
protocol.once('message_delivered', (event) => {
  console.log('First delivery:', event);
});
```

##### `removeAllListeners(eventType?: EventType | 'all'): this`

Removes all listeners for an event type, or all listeners if no type specified.

```typescript
protocol.removeAllListeners('message_received');
protocol.removeAllListeners(); // Remove all
```

##### `destroy(): Promise<void>`

Destroys the protocol instance and cleans up resources.

```typescript
await protocol.destroy();
```

### Events

#### Message Events

**`message_sent`**
```typescript
{
  type: 'message_sent';
  message_id: string;
  timestamp: number;
}
```

**`message_received`**
```typescript
{
  type: 'message_received';
  message_id: string;
  sender: string;
  recipient: string;
  content: string;
  hop_count: number;
  transport: string;        // 'BLE' | 'WiFiDirect' | 'Internet'
  timestamp: number;
}
```

**`message_delivered`**
```typescript
{
  type: 'message_delivered';
  message_id: string;
  latency_ms: number;
  hop_count: number;
  transport: string;
}
```

**`message_failed`**
```typescript
{
  type: 'message_failed';
  message_id: string;
  reason: string;
  retry_count: number;
}
```

#### Transport Events

**`transport_switched`**
```typescript
{
  type: 'transport_switched';
  from: string | null;
  to: string;
  reason: string;
}
```

#### Relay Events

**`relay_promoted`**
```typescript
{
  type: 'relay_promoted';
  connection_count: number;
  battery_level: number;
}
```

**`relay_demoted`**
```typescript
{
  type: 'relay_demoted';
  reason: string;
}
```

#### Network Events

**`neighbor_discovered`**
```typescript
{
  type: 'neighbor_discovered';
  peer_id: string;
  transport: string;
  rssi?: number;
}
```

**`neighbor_lost`**
```typescript
{
  type: 'neighbor_lost';
  peer_id: string;
}
```

**`network_metrics`**
```typescript
{
  type: 'network_metrics';
  neighbor_count: number;
  relay_count: number;
  delivery_ratio: number;   // 0.0 - 1.0
  avg_latency_ms: number;
}
```

### Enums

#### `MessagePriority`

```typescript
enum MessagePriority {
  Low = 0,
  Medium = 1,
  High = 2,
  Critical = 3,
}
```

## Example Use Cases

### Chat Application

```typescript
import React, { useEffect, useState } from 'react';
import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react-native';

function ChatScreen({ userId, recipientId }) {
  const [protocol, setProtocol] = useState(null);
  const [messages, setMessages] = useState([]);

  useEffect(() => {
    const proto = new OfflineProtocol({
      appId: 'chat-app',
      userId,
    });

    proto.on('message_received', (event) => {
      if (event.sender === recipientId) {
        setMessages((prev) => [...prev, {
          id: event.message_id,
          text: event.content,
          sender: event.sender,
          timestamp: event.timestamp,
        }]);
      }
    });

    proto.start();
    setProtocol(proto);

    return () => {
      proto.destroy();
    };
  }, [userId, recipientId]);

  const sendMessage = async (text) => {
    if (protocol) {
      await protocol.sendMessage({
        recipient: recipientId,
        content: text,
        priority: MessagePriority.High,
      });
    }
  };

  return (
    // Your UI here
  );
}
```

### Offline-First Mode (Emergency App)

```typescript
const protocol = new OfflineProtocol({
  appId: 'emergency-app',
  userId: 'responder-123',
  transport: {
    bleEnabled: true,
    wifiDirectEnabled: true,
    internetEnabled: false,  // Offline only!
  },
  dors: {
    preferOnline: false,
  },
  relay: {
    allowRelay: true,
    minBatteryForRelay: 15,  // Lower threshold for emergencies
  },
  network: {
    initialTtl: 10,  // Higher TTL for wider coverage
  },
});
```

## Troubleshooting

### iOS

**Issue**: Build fails with "library not found"

**Solution**: Run `pod install` in the `ios` directory.

**Issue**: Multiple architecture errors

**Solution**: Clean build folder (`Cmd+Shift+K`) and rebuild.

### Android

**Issue**: "could not find liboffline_protocol_ffi.so"

**Solution**: Clean and rebuild:
```bash
cd android && ./gradlew clean
cd .. && react-native run-android
```

**Issue**: NDK version mismatch

**Solution**: The pre-built libraries are compatible with NDK 21-26. Update your `android/build.gradle` if needed.

## Building from Source (For SDK Maintainers)

If you need to rebuild the native libraries:

### Prerequisites

- Rust toolchain (`rustup`)
- For iOS: Xcode
- For Android: Android NDK

### Build Commands

```bash
# Build for iOS
npm run build:ios

# Build for Android
npm run build:android

# Build for all platforms
npm run build:all

# Validate before publishing
npm run prepublishOnly
```

## License

Dual-licensed under MIT OR Apache-2.0

## Contributing

Contributions welcome! Please see [CONTRIBUTING.md](../../CONTRIBUTING.md) in the main repository.

## Links

- [Main Repository](https://github.com/offline-protocol/sdk)
- [Documentation](https://github.com/offline-protocol/sdk/tree/main/docs)
- [Issues](https://github.com/offline-protocol/sdk/issues)

