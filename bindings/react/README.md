# @offlineprotocol/react

React bindings for the Offline Protocol SDK - enabling offline-first messaging in web browsers using WebAssembly.

## Features

- 🔄 **Automatic Transport Switching**: DORS intelligently manages Internet transport (only available in browsers)
- 📡 **Offline-First**: Messages delivered even with poor connectivity
- 🔐 **Reliable Delivery**: ACK-based reliability with exponential backoff retry
- ⚡ **High Performance**: WebAssembly-powered Rust core provides near-native performance
- ⚛️ **React Hooks**: Convenient React hooks for easy integration
- 🌐 **Browser Support**: Works in all modern web browsers

## Installation

```bash
npm install @offlineprotocol/react @offlineprotocol/web
# or
yarn add @offlineprotocol/react @offlineprotocol/web
```

**Note**: `@offlineprotocol/web` is required as a peer dependency and contains the WebAssembly bindings.

## Usage

### Using React Hooks (Recommended)

```typescript
import React, { useEffect } from 'react';
import { 
  useOfflineProtocol, 
  useProtocolEvent, 
  useSendMessage,
  MessagePriority 
} from '@offlineprotocol/react';

function ChatApp() {
  const { protocol, isStarted, start, stop } = useOfflineProtocol({
    appId: 'my-app',
    userId: 'user123',
    transport: {
      internetEnabled: true,  // Only Internet available in browsers
    },
  });

  const sendMessage = useSendMessage(protocol);

  // Listen for received messages
  useProtocolEvent(protocol, 'message:received', (event) => {
    console.log(`Received from ${event.sender}: ${event.content}`);
  });

  // Auto-start on mount
  useEffect(() => {
    start();
    return () => stop();
  }, [start, stop]);

  const handleSend = async () => {
    try {
      const messageId = await sendMessage({
        recipient: 'user456',
        content: 'Hello!',
        priority: MessagePriority.High,
      });
      console.log('Sent:', messageId);
    } catch (error) {
      console.error('Failed to send:', error);
    }
  };

  return (
    <div>
      <div>Status: {isStarted ? '🟢 Online' : '🔴 Offline'}</div>
      <button onClick={handleSend} disabled={!isStarted}>
        Send Message
      </button>
    </div>
  );
}
```

### Using the Class Directly

```typescript
import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react';

const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'user123',
  transport: {
    internetEnabled: true,
  },
});

// Start the protocol
await protocol.start();

// Send a message
const messageId = await protocol.sendMessage({
  recipient: 'user456',
  content: 'Hello from offline!',
  priority: MessagePriority.High,
});

// Listen for events
protocol.on('message:received', (event) => {
  console.log(`Received message from ${event.sender}: ${event.content}`);
  console.log(`Delivered via ${event.transport} with ${event.hopCount} hops`);
});
```

### Monitoring Network Status

```typescript
// Transport switching
protocol.on('transport:switched', (event) => {
  console.log(`Switched from ${event.from} to ${event.to}`);
  console.log(`Reason: ${event.reason}`);
});

// Relay status (for future P2P support)
protocol.on('relay:promoted', (event) => {
  console.log(`Became relay with ${event.connectionCount} connections`);
});

protocol.on('relay:demoted', (event) => {
  console.log(`Demoted from relay: ${event.reason}`);
});

// Network metrics
protocol.on('network:metrics', (event) => {
  console.log(`Neighbors: ${event.neighborCount}`);
  console.log(`Delivery ratio: ${event.deliveryRatio * 100}%`);
});
```

### Lifecycle Management

```typescript
// In a React component
useEffect(() => {
  const protocol = new OfflineProtocol({
    appId: 'my-app',
    userId: 'user123',
  });

  protocol.start().catch(console.error);

  // Cleanup on unmount
  return () => {
    protocol.stop().catch(console.error);
  };
}, []);
```

## API Reference

### Hooks

#### `useOfflineProtocol(config: ProtocolConfig)`

Creates and manages an OfflineProtocol instance.

**Returns:**
- `protocol: OfflineProtocol | null` - Protocol instance
- `isStarted: boolean` - Whether protocol is started
- `error: Error | null` - Any error that occurred
- `start: () => Promise<void>` - Function to start the protocol
- `stop: () => Promise<void>` - Function to stop the protocol

#### `useProtocolEvent<T>(protocol, event, listener)`

Listens to protocol events with automatic cleanup.

**Parameters:**
- `protocol: OfflineProtocol | null` - Protocol instance
- `event: string` - Event name
- `listener: EventListener<T>` - Event handler

#### `useSendMessage(protocol)`

Returns a function to send messages.

**Parameters:**
- `protocol: OfflineProtocol | null` - Protocol instance

**Returns:**
- `sendMessage: (params) => Promise<string>` - Function to send messages

### Class: `OfflineProtocol`

#### Constructor

```typescript
new OfflineProtocol(config: ProtocolConfig)
```

#### Methods

- `start(): Promise<void>` - Starts the protocol
- `stop(): Promise<void>` - Stops the protocol
- `pause(): Promise<void>` - Pauses the protocol (no-op in browsers)
- `resume(): Promise<void>` - Resumes the protocol (no-op in browsers)
- `sendMessage(params): Promise<string>` - Sends a message
- `getState(): string` - Gets current protocol state
- `on(event, listener): void` - Registers event listener
- `off(event, listener): void` - Removes event listener

#### Properties

- `started: boolean` - Whether protocol is started

### Configuration Options

See `types.ts` for complete TypeScript definitions.

#### Transport Configuration

- `internetEnabled`: Enable Internet transport (default: true)
- `bleEnabled`: Not available in browsers (ignored)
- `wifiDirectEnabled`: Not available in browsers (ignored)

#### DORS Configuration

- `preferOnline`: Prefer Internet when available (default: false)
- `switchHysteresis`: Prevent rapid switching (default: 15.0)
- `switchCooldownSecs`: Wait time after switch (default: 20)

#### Relay Configuration

- `allowRelay`: Allow device to act as relay (default: true)
- `minBatteryForRelay`: Min battery % to relay (default: 30)
- `relayThreshold`: Min connections to become relay (default: 3)

## Platform Support

- ✅ **Modern Browsers**: Chrome, Firefox, Safari, Edge (with WebAssembly support)
- ⚠️ **Transport Limitations**: 
  - Only Internet transport is available in web browsers
  - BLE and Wi-Fi Direct are not available in browsers due to security restrictions
  - For full transport support, use the React Native binding on mobile platforms

## Architecture

```
React Web App (TypeScript/JavaScript)
    ↓
React Bindings (@offlineprotocol/react)
    ↓
WebAssembly Bindings (@offlineprotocol/web)
    ↓
Rust Core (DORS + Routing + Reliability)
```

The Rust core is compiled to WebAssembly, providing high performance and memory safety in the browser while sharing the same codebase with native platforms.

## Browser Compatibility

Requires:
- WebAssembly support (available in all modern browsers)
- ES2020 support for async/await
- Modern JavaScript features (supported by bundlers like webpack, vite, etc.)

## Bundling

When using with bundlers (webpack, vite, etc.), make sure to configure them to handle WebAssembly files:

### Vite

No additional configuration needed - Vite handles WASM automatically.

### Webpack

```javascript
module.exports = {
  experiments: {
    asyncWebAssembly: true,
  },
};
```

### Next.js

WASM is supported out of the box in Next.js 13+.

## TypeScript

Full TypeScript support is included with type definitions for all APIs and events.

## License

MIT OR Apache-2.0

