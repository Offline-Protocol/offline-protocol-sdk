# Transport Architecture

## Overview

The Offline Protocol SDK uses an extensible transport architecture that allows multiple communication channels (BLE, WiFi Direct, Internet) to coexist and be managed independently. This document explains the architecture and how to add new transports.

## Architecture Layers

```
┌─────────────────────────────────────────────────────┐
│              React Native Application               │
│         (App.tsx, UI Components, Logic)            │
└────────────────────┬────────────────────────────────┘
                     │
                     │ JavaScript API
                     ↓
┌─────────────────────────────────────────────────────┐
│           React Native Bindings Layer              │
│                (index.ts, types.ts)                 │
└────────────────────┬────────────────────────────────┘
                     │
                     │ Native Bridge
                     ↓
┌─────────────────────────────────────────────────────┐
│            Native Module Layer                      │
│      (OfflineProtocolModule.swift/.kt)             │
│                                                     │
│  ┌─────────────────────────────────────┐          │
│  │    Transport Manager Interface      │          │
│  │   (TransportManager protocol/interface) │      │
│  └────────────┬────────────────────────┘          │
│               │                                     │
│    ┌──────────┴────────┬─────────────┐            │
│    │                   │              │            │
│ ┌──▼────────┐  ┌──────▼──────┐  ┌───▼──────┐    │
│ │BleManager │  │WifiManager  │  │NetManager│    │
│ │(Impl)     │  │(Future)     │  │(Future)  │    │
│ └───────────┘  └─────────────┘  └──────────┘    │
└────────────────────┬────────────────────────────────┘
                     │
                     │ UniFFI Bridge
                     ↓
┌─────────────────────────────────────────────────────┐
│                Rust Protocol Core                   │
│                                                     │
│  ┌──────────────┐  ┌──────────────┐  ┌─────────┐ │
│  │   Router     │  │ Reliability  │  │  DORS   │ │
│  │   (Path)     │  │(ACK/Retry)   │  │(Select) │ │
│  └──────────────┘  └──────────────┘  └─────────┘ │
│                                                     │
│  ┌───────────────────────────────────────────────┐│
│  │         Transport Abstraction Layer           ││
│  │  (BleTransport, WiFiTransport, NetTransport)  ││
│  └───────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────┘
```

## TransportManager Pattern

### Design Principles

1. **Separation of Concerns**: Platform-specific BLE/network code stays in native modules, not in the app
2. **Extensibility**: Easy to add new transports (WiFi Direct, Internet, etc.)
3. **Uniform Interface**: All transports implement the same interface
4. **Independent Lifecycle**: Each transport manages its own start/stop/pause/resume
5. **Cross-Platform**: Same interface on iOS and Android

### Interface Definition

#### iOS (Swift)

```swift
public protocol TransportManager: AnyObject {
    var transportId: String { get }
    var transportName: String { get }
    var state: TransportState { get }
    var delegate: TransportManagerDelegate? { get set }
    
    func isAvailable() -> Bool
    func start() throws
    func stop()
    func pause()
    func resume()
    func getMetrics() -> [String: Any]
}
```

#### Android (Kotlin)

```kotlin
interface TransportManager {
    val transportId: String
    val transportName: String
    val state: TransportState
    var listener: TransportManagerListener?
    
    fun isAvailable(): Boolean
    fun start()
    fun stop()
    fun pause()
    fun resume()
    fun getMetrics(): Map<String, Any>
}
```

## Implemented Transports

### BLE (Bluetooth Low Energy)

**Status**: ✅ Fully Implemented

**Files**:
- iOS: `bindings/react-native/ios/BleManager.swift`
- Android: `bindings/react-native/android/.../BleManager.kt`

**Features**:
- Automatic scanning for nearby devices
- Automatic advertising to be discoverable
- GATT server (peripheral role) for receiving data
- GATT client (central role) for sending data
- Fragment polling and transmission
- Peer discovery and loss detection
- Cross-platform compatible (iOS ↔ Android)

**UUIDs**:
- Service: `6E400001-B5A3-F393-E0A9-E50E24DCCA9E`
- Message Characteristic: `6E400002-B5A3-F393-E0A9-E50E24DCCA9E`
- Device ID Characteristic: `6E400003-B5A3-F393-E0A9-E50E24DCCA9E`

**Lifecycle**:
1. `start()` → Begin scanning and advertising
2. Discover peers → Connect and setup GATT
3. Poll for fragments → Send via BLE characteristics
4. Receive fragments → Pass to Rust core
5. `stop()` → Disconnect all peers, stop scanning/advertising

### WiFi Direct

**Status**: 🚧 Planned

**Use Case**: Higher bandwidth peer-to-peer communication for larger files or video

**Implementation Requirements**:
1. Create `WifiDirectManager.swift` (iOS) and `WifiDirectManager.kt` (Android)
2. Implement `TransportManager` interface
3. Handle WiFi Direct connection setup
4. Integrate with protocol's fragment system
5. Add to `OfflineProtocolModule` initialization

**Platform Notes**:
- Android: Use `WifiP2pManager` API
- iOS: Use `MultipeerConnectivity` framework (Apple's WiFi Direct equivalent)

### Internet (Relay Server)

**Status**: ✅ Implemented

**Use Case**: Communication when devices are far apart via a relay server

**Architecture**:
```
Device A ←→ WebSocket ←→ Relay Server ←→ WebSocket ←→ Device B
```

**Send confirmation loop**: The platform drains the SDK's outgoing queue via
`internetGetNextMessage()`, sends bytes over the WebSocket, and **must** report
the outcome via `internetConfirmSent(messageId)` or `internetSendFailed(messageId)`.
This feeds real delivery data into DORS so it can make accurate routing decisions.

## Adding a New Transport

### Step 1: Create the Transport Class

#### iOS

Create `NewTransportManager.swift`:

```swift
import Foundation

class NewTransportManager: NSObject, TransportManager {
    let transportId = "new_transport"
    let transportName = "New Transport"
    var state: TransportState = .unavailable
    weak var delegate: TransportManagerDelegate?
    
    private let protocol: OfflineProtocol
    private let deviceId: String
    
    init(protocol: OfflineProtocol, deviceId: String) {
        self.protocol = `protocol`
        self.deviceId = deviceId
        super.init()
    }
    
    func isAvailable() -> Bool {
        // Check if transport is available on this device
        return true
    }
    
    func start() throws {
        // 1. Check availability
        guard isAvailable() else {
            throw TransportError.notAvailable("Transport not available")
        }
        
        // 2. Initialize transport
        updateState(.starting)
        
        // 3. Start discovery/connection
        startDiscovery()
        
        // 4. Start data transfer
        startDataTransfer()
        
        updateState(.running)
    }
    
    func stop() {
        // Clean up resources
        updateState(.stopped)
    }
    
    private func updateState(_ newState: TransportState) {
        state = newState
        delegate?.transportManager(self, didChangeState: newState)
    }
    
    // Implement discovery, data transfer, etc.
}
```

#### Android

Create `NewTransportManager.kt`:

```kotlin
class NewTransportManager(
    private val context: Context,
    private val protocol: OfflineProtocol,
    private val deviceId: String
) : TransportManager {
    
    override val transportId = "new_transport"
    override val transportName = "New Transport"
    override var state: TransportState = TransportState.UNAVAILABLE
    override var listener: TransportManagerListener? = null
    
    override fun isAvailable(): Boolean {
        // Check if transport is available on this device
        return true
    }
    
    override fun start() {
        // 1. Check availability
        if (!isAvailable()) {
            throw TransportException.NotAvailable("Transport not available")
        }
        
        // 2. Initialize transport
        updateState(TransportState.STARTING)
        
        // 3. Start discovery/connection
        startDiscovery()
        
        // 4. Start data transfer
        startDataTransfer()
        
        updateState(TransportState.RUNNING)
    }
    
    override fun stop() {
        // Clean up resources
        updateState(TransportState.STOPPED)
    }
    
    private fun updateState(newState: TransportState) {
        state = newState
        listener?.onTransportStateChanged(this, newState)
    }
    
    // Implement discovery, data transfer, etc.
}
```

### Step 2: Integrate with Native Module

#### iOS

Update `OfflineProtocolModule.swift`:

```swift
class OfflineProtocolModule: RCTEventEmitter {
    private var bleManager: BleManager?
    private var newTransportManager: NewTransportManager? // Add this
    
    @objc func create(_ configJson: String, ...) {
        // ... existing code ...
        
        // Initialize new transport if enabled
        if config.newTransportEnabled {
            newTransportManager = NewTransportManager(protocol: proto, deviceId: config.userId)
        }
    }
    
    @objc func start(...) {
        // ... existing code ...
        
        // Start new transport
        try? newTransportManager?.start()
    }
    
    @objc func stop(...) {
        // ... existing code ...
        
        // Stop new transport
        newTransportManager?.stop()
    }
}
```

#### Android

Update `OfflineProtocolModule.kt`:

```kotlin
class OfflineProtocolModule(reactContext: ReactApplicationContext) :
    ReactContextBaseJavaModule(reactContext) {
    
    private var bleManager: BleManager? = null
    private var newTransportManager: NewTransportManager? = null // Add this
    
    @ReactMethod
    fun create(configJson: String, promise: Promise) {
        // ... existing code ...
        
        // Initialize new transport if enabled
        if (config.newTransportEnabled) {
            newTransportManager = NewTransportManager(reactApplicationContext, proto, config.userId)
        }
    }
    
    @ReactMethod
    fun start(promise: Promise) {
        // ... existing code ...
        
        // Start new transport
        newTransportManager?.start()
    }
    
    @ReactMethod
    fun stop(promise: Promise) {
        // ... existing code ...
        
        // Stop new transport
        newTransportManager?.stop()
    }
}
```

### Step 3: Update Configuration

Add configuration options in `bindings/react-native/src/types.ts`:

```typescript
export interface ProtocolConfig {
  appId: string;
  userId: string;
  transports?: {
    ble?: {
      enabled: boolean;
    };
    newTransport?: {  // Add this
      enabled: boolean;
      // Add transport-specific config
    };
  };
}
```

### Step 4: Add Permissions

#### iOS

Update `Info.plist` with required permissions for the new transport.

#### Android

Update `AndroidManifest.xml` with required permissions.

## Data Flow

### Outgoing Messages

```
1. App calls sendMessage()
2. DORS selects the best transport (with fallback on failure)
3. Rust core creates message and stores in transport queue
4. Native transport managers poll:
   - BLE: bleGetNextFragment()
   - Internet: internetGetNextMessage() → returns messageId + bytes
5. Transport sends data over platform-specific channel
6. Platform reports outcome:
   - BLE: implicit (fragment send is synchronous)
   - Internet: internetConfirmSent(messageId) or internetSendFailed(messageId)
7. Remote device receives data and passes to Rust core
8. Rust core delivers message and sends ACK
```

### Incoming Messages

```
1. Platform receives data on transport channel
2. Native manager extracts sender ID and fragment data
3. Manager calls protocol.bleFragmentReceived(senderId, data)
4. Rust core reassembles fragments
5. Rust core emits message_received event
6. Event flows to JavaScript app
7. App displays message
```

## Transport Selection (DORS)

The Rust core uses the **Dynamic Opportunistic Routing Selection (DORS)** algorithm to choose the best transport for each message based on:

- **Signal strength** (RSSI for BLE)
- **Bandwidth** (BLE < WiFi Direct < Internet)
- **Latency** (BLE ≈ WiFi Direct << Internet)
- **Reliability** (historical delivery rates)
- **Congestion** (queue depths)
- **Battery** (energy consumption)
- **User preferences** (prefer online vs offline-first)

Transports don't need to implement routing logic - they just report availability and handle sending/receiving fragments.

## Best Practices

### 1. Keep Transports Independent
Each transport should work independently without knowledge of other transports.

### 2. Report Metrics
Implement `getMetrics()` to report:
- Bytes sent/received
- Connection count
- Error rates
- Latency samples

### 3. Handle Lifecycle Properly
- Clean up resources in `stop()`
- Support `pause()` for background mode
- Reconnect automatically in `resume()`

### 4. Error Handling
- Don't crash on errors - report via delegate/listener
- Retry transient errors
- Fall back gracefully when unavailable

### 5. Cross-Platform Consistency
- Use identical UUIDs/identifiers on both platforms
- Same data format and encoding
- Compatible MTU/fragment sizes

### 6. Permissions
- Check permissions before starting
- Request permissions at appropriate times
- Provide clear error messages

## Testing New Transports

### Unit Tests
Test transport manager independently:
```swift
func testTransportLifecycle() {
    let transport = NewTransportManager(...)
    XCTAssertTrue(transport.isAvailable())
    XCTAssertNoThrow(try transport.start())
    XCTAssertEqual(transport.state, .running)
    transport.stop()
    XCTAssertEqual(transport.state, .stopped)
}
```

### Integration Tests
Test with mock protocol:
- Verify fragments are polled
- Verify fragments are sent
- Verify received data is passed to protocol

### Cross-Platform Tests
- iOS ↔ Android communication
- Multiple devices simultaneously
- Failover between transports

## Future Enhancements

### Multi-Transport Simultaneous Use
Currently transports operate independently. Future enhancement: send same message over multiple transports for reliability.

### Transport Bonding
Combine multiple transports for increased bandwidth (channel bonding).

### Smart Transport Switching
Automatic transparent switching between transports based on availability and performance.

### Transport Priorities
Allow apps to specify preferred transport order.

## Reference Implementation

See `BleManager.swift` and `BleManager.kt` for complete, production-ready examples of the TransportManager pattern.

## Summary

The TransportManager pattern provides:
- ✅ Clean separation of concerns
- ✅ Easy extensibility
- ✅ Platform-specific optimizations
- ✅ Consistent interface across transports
- ✅ Independent lifecycle management

This architecture makes it straightforward to add WiFi Direct, Internet, or any other transport without modifying existing code.

