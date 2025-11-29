# @offline-protocol/mesh-sdk

Offline-first mesh networking SDK with intelligent transport switching for React Native. Built with Rust for maximum performance and reliability.

## Features

- **Offline-First**: Messages delivered even without internet connectivity
- **Intelligent Transport Switching**: DORS automatically selects the best transport (Internet, BLE, WiFi Direct)
- **Mesh Networking**: Multi-hop routing with automatic relay selection
- **Cross-Platform**: Works on iOS and Android
- **Type-Safe**: Full TypeScript support

## Installation

```bash
npm install @offline-protocol/mesh-sdk
```

### iOS Setup

```bash
cd ios && pod install
```

### Android Setup

No additional setup needed. Pre-built libraries are included.

## Quick Start

```typescript
import { OfflineProtocol, MessagePriority } from '@offline-protocol/mesh-sdk';

const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
});

protocol.on('message_received', (event) => {
  console.log(`From ${event.sender}: ${event.content}`);
});

await protocol.start();

const messageId = await protocol.sendMessage({
  recipient: 'user456',
  content: 'Hello!',
  priority: MessagePriority.High,
});
```

## Configuration

```typescript
interface ProtocolConfig {
  appId: string;
  userId: string;
  transport?: {
    bleEnabled?: boolean;        // default: true
    wifiDirectEnabled?: boolean;  // default: true 
    internetEnabled?: boolean;    // default: true
  };
  dors?: {
    preferOnline?: boolean;      // default: false
    switchHysteresis?: number;    // default: 15.0
    switchCooldownSecs?: number; // default: 20
  };
  relay?: {
    allowRelay?: boolean;         // default: true
    minBatteryForRelay?: number; // default: 30
    relayThreshold?: number;      // default: 3
  };
  network?: {
    initialTtl?: number;          // default: 8
  };
}
```

## API

### Methods

- `start(): Promise<void>` - Start the protocol
- `stop(): Promise<void>` - Stop the protocol
- `sendMessage(params): Promise<string>` - Send a message
- `on(eventType, listener)` - Register event listener
- `off(eventType, listener)` - Remove event listener
- `destroy(): Promise<void>` - Clean up resources

### Events

**Message Events:**
- `message_sent` - Message was sent
- `message_received` - Message was received
- `message_delivered` - Message was delivered (ACK received)
- `message_failed` - Message delivery failed

**Network Events:**
- `transport_switched` - Transport changed (BLE/WiFi/Internet)
- `neighbor_discovered` - New neighbor found
- `neighbor_lost` - Neighbor disconnected

### Message Priority

```typescript
enum MessagePriority {
  Low = 0,
  Medium = 1,
  High = 2,
  Critical = 3,
}
```

## How It Works

### DORS (Dynamic Offline Relay Switch)

DORS automatically selects the best transport (Internet, BLE, or WiFi Direct) based on:
- Signal strength (RSSI)
- Bandwidth and congestion
- Energy efficiency
- Reliability and proximity

### Mesh Network

The SDK implements a cluster-based mesh network where devices organize into clusters and form connections based on mesh topology:

**Cluster Architecture:**
- **MEMBER Role**: Devices within the same cluster (intra-cluster connections)
- **BRIDGE Role**: Devices connecting different clusters (inter-cluster connections)
- **Connection Budget**: Each device maintains up to 4 active connections (configurable)

**Connection Decision Process:**
1. Devices discover each other via BLE advertisements containing mesh metadata
2. MeshController evaluates connection candidates based on:
   - Available connection slots
   - Peer scores (RSSI, battery, uptime, stability, load)
   - Cluster membership and free slot estimates
3. Connection intent determines role:
   - `INTRA_CLUSTER` → MEMBER role (same cluster)
   - `INTER_CLUSTER` → BRIDGE role (different clusters)
4. When connection budget is full, the system can evict the worst peer to make room for better connections

**Message Routing:**
- Direct delivery when recipient is a connected peer (1 hop)
- Multi-hop routing through cluster members and bridges (up to TTL hops, default 8)
- Automatic path selection based on cluster topology and peer quality
- Messages traverse clusters via bridge connections when needed

## Example

```typescript
import React, { useEffect, useState } from 'react';
import { OfflineProtocol, MessagePriority } from '@offline-protocol/mesh-sdk';

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

    return () => proto.destroy();
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

  return (/* Your UI */);
}
```

## Troubleshooting

**iOS Build Issues:**
- Run `pod install` in the `ios` directory
- Clean build folder (`Cmd+Shift+K`) and rebuild

**Android Build Issues:**
- Clean and rebuild: `cd android && ./gradlew clean`
- Ensure NDK 21-26 is installed

## Documentation

- [DORS Configuration Guide](../../docs/dors-configuration.md)
- [Architecture Overview](../../docs/architecture.md)
- [API Reference](../../docs/api-reference.md)
- [SDK Integration Guide](../../docs/sdk-integration.md)
