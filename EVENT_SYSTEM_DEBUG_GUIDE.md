# Event System Debugging Guide

## Quick Test to Verify Events are Working

I've added two mechanisms to test if events are being emitted properly:

### 1. Automatic Event on Start

When you call `protocol.start()`, it now automatically emits a `network_metrics` event. This should fire immediately and confirm the event system is working.

### 2. Manual Test Event

You can manually trigger a test event anytime with:
```javascript
protocol.emitTestEvent();
```

## Step-by-Step Testing

### Test 1: Basic Event Listener

```javascript
import { OfflineProtocol } from '@offline-protocol/react-native';

// Create protocol
const protocol = new OfflineProtocol({
  appId: 'test-app',
  userId: 'user-123',
  bleEnabled: true,
  wifiDirectEnabled: false,
  internetEnabled: false,
  preferOnline: false,
  initialTtl: 8,
});

// Set up event listener BEFORE creating the protocol
protocol.on('all', (event) => {
  console.log('[EVENT RECEIVED]', event.type, JSON.stringify(event));
});

// Create the protocol instance
await protocol.create();

// This should emit a test event
console.log('[TEST] Calling emitTestEvent()...');
await protocol.emitTestEvent();

// Wait a moment for event to arrive
await new Promise(resolve => setTimeout(resolve, 100));

// Start the protocol (this should emit network_metrics event automatically)
console.log('[TEST] Starting protocol...');
await protocol.start();

// Wait a moment for event to arrive
await new Promise(resolve => setTimeout(resolve, 100));

console.log('[TEST] If you see event logs above, the event system is working!');
```

### Test 2: Specific Event Types

```javascript
// Listen for specific event types
protocol.on('network_metrics', (event) => {
  console.log('[NETWORK_METRICS]', event);
});

protocol.on('neighbor_discovered', (event) => {
  console.log('[NEIGHBOR_DISCOVERED]', event.peer_id, 'RSSI:', event.rssi);
});

protocol.on('message_sent', (event) => {
  console.log('[MESSAGE_SENT]', event.message_id);
});

// Test neighbor discovery
await protocol.blePeerDiscovered('test-peer-1', -55);

// Test message sending
const messageId = await protocol.sendMessage('user-456', 'Hello!', 1);
console.log('Sent message:', messageId);
```

### Test 3: Event Polling (Alternative to Callbacks)

If callbacks aren't working, you can also poll for events:

```javascript
// Poll for events manually
setInterval(async () => {
  const eventJson = await protocol.pollEvent();
  if (eventJson) {
    const event = JSON.parse(eventJson);
    console.log('[POLLED EVENT]', event.type, event);
  }
}, 100);
```

## Expected Event Flow

### When Protocol Starts
1. Call `protocol.start()`
2. **Expected Event:** `network_metrics` with all zeros
3. Console should show: `[EVENT RECEIVED] network_metrics {...}`

### When Test Event is Called
1. Call `protocol.emitTestEvent()`
2. **Expected Event:** `network_metrics` with all zeros
3. Console should show: `[EVENT RECEIVED] network_metrics {...}`

### When Peer is Discovered
1. Call `protocol.blePeerDiscovered('peer-1', -55)`
2. **Expected Event:** `neighbor_discovered` with peer_id and rssi
3. Console should show: `[EVENT RECEIVED] neighbor_discovered {...}`

### When Message is Sent
1. Call `protocol.sendMessage('user-456', 'Hello!', 1)`
2. **Expected Event:** `message_sent` with message_id
3. Console should show: `[EVENT RECEIVED] message_sent {...}`

## Common Issues and Solutions

### Issue 1: No Events at All

**Symptoms:**
- No console logs showing events
- `emitTestEvent()` doesn't trigger anything
- `start()` doesn't emit events

**Possible Causes:**
1. Event listeners not registered before `create()`
2. Native module not loaded properly
3. Event callback not set up in native code

**Solution:**
```javascript
// Make sure listeners are set up BEFORE create()
protocol.on('all', (event) => console.log('[EVENT]', event));

// Then create
await protocol.create();

// Test immediately
await protocol.emitTestEvent();
```

### Issue 2: Some Events Work, Others Don't

**Symptoms:**
- `emitTestEvent()` works
- `blePeerDiscovered()` emits events
- But `sendMessage()` doesn't emit events

**Possible Causes:**
- Transport not available
- Protocol not started
- Message send failing silently

**Solution:**
```javascript
// Check protocol is started
await protocol.start();

// Check transport is available
const transports = await protocol.getActiveTransports();
console.log('Available transports:', transports);

// Try sending with error handling
try {
  const messageId = await protocol.sendMessage('user-456', 'Hello!', 1);
  console.log('Message sent:', messageId);
} catch (error) {
  console.error('Send failed:', error);
}
```

### Issue 3: Events Delayed or Batched

**Symptoms:**
- Events arrive late
- Multiple events arrive at once

**Possible Causes:**
- Event queue is being polled slowly
- Process loop not running

**Solution:**
Make sure the process loop is running (it should start automatically when protocol is created).

### Issue 4: Events on iOS but not Android (or vice versa)

**Possible Causes:**
- Platform-specific binding issue
- Native module not linked properly

**Solution:**
1. Check that the native module is properly linked
2. Rebuild the app from scratch
3. Check native logs for errors

## Debugging Checklist

- [ ] Event listeners registered before `protocol.create()`
- [ ] Protocol created successfully without errors
- [ ] `emitTestEvent()` is called and logs appear
- [ ] `protocol.start()` is called and emits `network_metrics`
- [ ] `blePeerDiscovered()` emits `neighbor_discovered`
- [ ] `sendMessage()` returns message ID without error
- [ ] Transport is available (`getActiveTransports()` includes BLE)
- [ ] Process loop is running (check native logs)

## Native Code Verification

### iOS (Swift)

Check that the event callback is being set:

```swift
// In OfflineProtocolModule.swift
proto.setEventCallback(callback: EventCallbackImpl(emitter: self))
```

And that events are being sent:

```swift
func sendEventToJS(_ eventName: String, body: Any?) {
    if hasListeners {
        sendEvent(withName: eventName, body: body)
    }
}
```

### Android (Kotlin)

Check that the event callback is being set:

```kotlin
// In OfflineProtocolModule.kt
proto.setEventCallback(object : EventCallback {
    override fun onEvent(eventJson: String) {
        val params = Arguments.createMap().apply {
            putString("eventJson", eventJson)
        }
        sendEvent(EVENT_NAME, params)
    }
})
```

## Testing in Example App

```bash
cd examples/react-native-app

# For iOS
npm run ios

# For Android  
npm run android
```

Then in the app, add this test code:

```javascript
import { OfflineProtocol } from '@offline-protocol/react-native';

const TestEvents = () => {
  const [events, setEvents] = useState([]);
  
  useEffect(() => {
    const protocol = new OfflineProtocol({
      appId: 'test-app',
      userId: 'user-123',
      bleEnabled: true,
    });
    
    // Log all events
    protocol.on('all', (event) => {
      console.log('[EVENT]', event.type, event);
      setEvents(prev => [...prev, event]);
    });
    
    // Initialize
    const init = async () => {
      await protocol.create();
      console.log('[TEST] Created protocol');
      
      // Test 1: Manual test event
      await protocol.emitTestEvent();
      console.log('[TEST] Emitted test event');
      
      // Test 2: Start protocol (should emit event)
      await protocol.start();
      console.log('[TEST] Started protocol');
      
      // Test 3: Peer discovery
      await protocol.blePeerDiscovered('test-peer', -60);
      console.log('[TEST] Discovered peer');
      
      // Test 4: Send message
      try {
        const msgId = await protocol.sendMessage('user-456', 'Hello!', 1);
        console.log('[TEST] Sent message:', msgId);
      } catch (error) {
        console.error('[TEST] Send failed:', error);
      }
    };
    
    init();
  }, []);
  
  return (
    <View>
      <Text>Events Received: {events.length}</Text>
      {events.map((event, i) => (
        <Text key={i}>{event.type}</Text>
      ))}
    </View>
  );
};
```

## Expected Output

If everything is working correctly, you should see:

```
[EVENT] network_metrics { type: 'network_metrics', neighbor_count: 0, ... }
[TEST] Emitted test event

[EVENT] network_metrics { type: 'network_metrics', neighbor_count: 0, ... }
[TEST] Started protocol

[EVENT] neighbor_discovered { type: 'neighbor_discovered', peer_id: 'test-peer', rssi: -60, ... }
[TEST] Discovered peer

[EVENT] message_sent { type: 'message_sent', message_id: '...', ... }
[TEST] Sent message: ...
```

## Still Not Working?

If events still aren't working after all these tests:

1. **Check native logs:**
   - iOS: Open Xcode console
   - Android: `adb logcat | grep OfflineProtocol`

2. **Verify native module is loaded:**
   ```javascript
   import { NativeModules } from 'react-native';
   console.log('Native module:', NativeModules.OfflineProtocolModule);
   ```

3. **Try polling instead of callbacks:**
   ```javascript
   setInterval(async () => {
     const event = await protocol.pollEvent();
     if (event) console.log('[POLLED]', JSON.parse(event));
   }, 100);
   ```

4. **Check if it's a timing issue:**
   ```javascript
   // Add delays between operations
   await protocol.create();
   await new Promise(r => setTimeout(r, 1000));
   await protocol.emitTestEvent();
   await new Promise(r => setTimeout(r, 1000));
   ```

5. **Rebuild everything from scratch:**
   ```bash
   # Clean
   cd ios && pod deintegrate && pod install && cd ..
   cd android && ./gradlew clean && cd ..
   
   # Rebuild
   npm run ios
   # or
   npm run android
   ```

## Summary

The event system has three layers:
1. **Rust**: Emits events via `emit_event()`
2. **Native Bridge**: Receives via `EventCallback` trait and forwards to React Native
3. **JavaScript**: Receives via event emitter and calls registered listeners

All three layers should now be properly connected. The `emitTestEvent()` method and automatic event on `start()` allow you to test each layer independently.

