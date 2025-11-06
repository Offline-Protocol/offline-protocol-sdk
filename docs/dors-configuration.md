# DORS Configuration Guide

## Overview

DORS (Dynamic Offline Relay Switch) is the intelligent transport selection engine at the heart of the Offline Protocol SDK. It automatically chooses and switches between Internet, BLE Mesh, and Wi-Fi Direct based on real-time network conditions, ensuring optimal message delivery.

## How DORS Works

### Multi-Factor Scoring System

DORS evaluates each available transport using multiple factors:

1. **Signal Strength** (RSSI for wireless transports)
2. **Proximity** (hop count - lower is better)
3. **Bandwidth** (throughput capability)
4. **Congestion** (queue depth and failure rate)
5. **Energy Efficiency** (battery impact)

Each factor is scored 0-100, then weighted according to the transport type's characteristics.

### Transport-Specific Scoring

**BLE Transport:**
```
Total Score = (Signal × 0.3) + (Energy × 0.3) + (Congestion × 0.2) + (Proximity × 0.2)
```
- Optimized for energy efficiency and signal quality
- Best for: Dense urban areas, low-power devices, short-range mesh

**WiFi Direct Transport:**
```
Total Score = (Bandwidth × 0.4) + (Proximity × 0.3) + (Congestion × 0.3)
```
- Optimized for high throughput and direct peer connections
- Best for: File transfers, video streaming, high-bandwidth needs

**Internet Transport:**
```
Total Score = 100 (if preferOnline: true) or 0 (if preferOnline: false)
```
- Binary decision based on online-first vs offline-first mode
- Best for: Hybrid apps with server infrastructure

### Switching Algorithm

DORS prevents rapid transport switching ("flapping") using three mechanisms:

1. **Hysteresis**: New transport must score significantly higher (default: 15 points)
2. **Cooldown**: Wait period after switching (default: 20 seconds)
3. **Stability Window**: New transport must be consistently better (default: 8 seconds)

Only when all three conditions are met will DORS switch transports.

### Escalation Logic

DORS can automatically escalate from BLE to WiFi Direct when:
- BLE retry failures reach threshold (default: 2 failures)
- RSSI drops below threshold (default: -85 dBm)
- Queue depth exceeds congestion threshold (default: 50 messages)

## Configuration Parameters

### Complete Configuration Example

```typescript
const config = {
  appId: 'my-app',
  userId: userId,
  
  // Transport configuration
  transports: {
    ble: {
      enabled: true,
    },
    internet: {
      enabled: true,
      serverAddress: 'wss://relay.example.com',
      autoReconnect: true,
      reconnectDelay: 5000,
    },
    wifiDirect: {
      enabled: true, // Android only
      deviceName: 'MyDevice',
      autoAccept: false,
      groupOwnerIntent: 7, // 0-15, higher = more likely to be group owner
    },
  },
  
  // DORS configuration
  dors: {
    preferOnline: false,
    switchHysteresis: 15.0,
    switchCooldownSecs: 20,
    bleToWifiRetryThreshold: 2,
    rssiSwitchThreshold: -85,
    congestionQueueThreshold: 50,
    stabilityWindowSecs: 8,
  },
};
```

### Parameter Reference

| Parameter | Type | Default | Range | Description |
|-----------|------|---------|-------|-------------|
| `preferOnline` | boolean | `false` | - | When `true`, always use Internet if available |
| `switchHysteresis` | number | `15.0` | 0-100 | Minimum score improvement required to switch |
| `switchCooldownSecs` | number | `20` | 0-300 | Seconds to wait after switching before allowing another switch |
| `bleToWifiRetryThreshold` | number | `2` | 1-10 | Number of BLE failures before suggesting WiFi Direct |
| `rssiSwitchThreshold` | number | `-85` | -100 to -40 | RSSI threshold (dBm) for BLE→WiFi escalation |
| `congestionQueueThreshold` | number | `50` | 10-500 | Queue depth that indicates high congestion |
| `stabilityWindowSecs` | number | `8` | 1-60 | Duration to verify new transport stability |

## Use Case Examples

### Emergency Response (Offline-First, Aggressive Switching)

**Requirements:**
- Must work without Internet
- Fast adaptation to changing conditions
- Prioritize message delivery over battery

**Configuration:**
```typescript
dors: {
  preferOnline: false,
  switchHysteresis: 5.0,          // Quick to switch
  switchCooldownSecs: 5,          // Short cooldown
  bleToWifiRetryThreshold: 1,     // Fast escalation
  rssiSwitchThreshold: -75,       // Switch early on weak signal
  congestionQueueThreshold: 20,   // Low tolerance for congestion
  stabilityWindowSecs: 3,         // Fast verification
}
```

**Behavior:**
- Switches transports quickly when conditions change
- Escalates to WiFi Direct after just 1 BLE failure
- Tolerates minimal congestion before switching
- Minimizes message delivery latency

### Social Messaging (Hybrid, Balanced)

**Requirements:**
- Use server when online for features like delivery receipts
- Fall back to mesh when offline
- Balance performance with battery life

**Configuration:**
```typescript
dors: {
  preferOnline: true,             // Server-first
  switchHysteresis: 15.0,         // Standard switching
  switchCooldownSecs: 20,         // Standard cooldown
  bleToWifiRetryThreshold: 2,     // Moderate escalation
  rssiSwitchThreshold: -85,       // Standard signal threshold
  congestionQueueThreshold: 50,   // Standard congestion
  stabilityWindowSecs: 8,         // Standard stability
}
```

**Behavior:**
- Always uses Internet when connected
- Falls back to BLE mesh when offline
- Balanced switching to avoid flapping
- Standard battery conservation

### Background Sync (Conservative, Battery-Conscious)

**Requirements:**
- Batch sync when conditions are good
- Minimize battery impact
- Tolerate delays for better efficiency

**Configuration:**
```typescript
dors: {
  preferOnline: true,
  switchHysteresis: 25.0,         // Very conservative
  switchCooldownSecs: 60,         // Long cooldown
  bleToWifiRetryThreshold: 5,     // Reluctant escalation
  rssiSwitchThreshold: -90,       // Stay on BLE longer
  congestionQueueThreshold: 100,  // High tolerance
  stabilityWindowSecs: 20,        // Long verification
}
```

**Behavior:**
- Sticks with current transport unless clearly better option
- Rarely escalates to WiFi Direct
- Tolerates congestion and poor signal
- Optimizes for battery life

### Live Collaboration (High-Bandwidth, Low-Latency)

**Requirements:**
- Real-time data synchronization
- High throughput for large documents
- Fast switching to maintain interactivity

**Configuration:**
```typescript
dors: {
  preferOnline: true,
  switchHysteresis: 8.0,          // Responsive switching
  switchCooldownSecs: 10,         // Quick cooldown
  bleToWifiRetryThreshold: 1,     // Fast WiFi Direct escalation
  rssiSwitchThreshold: -70,       // Aggressive signal threshold
  congestionQueueThreshold: 15,   // Very low congestion tolerance
  stabilityWindowSecs: 4,         // Quick verification
}
```

**Behavior:**
- Quickly escalates to WiFi Direct for bandwidth
- Low tolerance for congestion or poor signal
- Prefers high-throughput transports
- Optimizes for latency and throughput

## Monitoring DORS Behavior

### Transport Switch Events

Listen for `transport_switched` events to understand DORS decisions:

```typescript
protocol.on('transport_switched', (event) => {
  console.log(`Transport switched: ${event.from} → ${event.to}`);
  console.log(`Reason: ${event.reason}`);
});
```

**Event Structure:**
```typescript
{
  type: 'transport_switched',
  from: 'ble' | 'internet' | 'wifiDirect' | null,
  to: 'ble' | 'internet' | 'wifiDirect',
  reason: string,
  timestamp: number
}
```

### Common Switch Reasons

| Reason | Meaning | Action |
|--------|---------|--------|
| "DORS selected better transport" | Normal switching based on scoring | No action needed |
| "DORS suggests escalating to WiFi Direct due to BLE failures" | Escalation triggered | Consider enabling WiFi Direct if not already active |
| "BLE transport became available" | BLE initialized | Normal startup |
| "Internet connection established" | Server connected | Normal in hybrid mode |

## Advanced Tuning

### Calculating Optimal Hysteresis

Hysteresis prevents flapping. Calculate based on your variance:

```
Hysteresis = (Average Score Variance) × 1.5
```

For example, if your transport scores typically vary by ±10 points, set hysteresis to 15.

### Tuning Cooldown Period

Cooldown should match your network dynamics:

- **Fast-changing (mobile users)**: 5-10 seconds
- **Moderate (pedestrians)**: 15-30 seconds
- **Stable (stationary devices)**: 30-60 seconds

### Congestion Threshold

Based on your message rate and latency tolerance:

```
Threshold = (Messages per minute) × (Acceptable latency in minutes)
```

For example:
- 10 messages/min, 5-minute tolerance → threshold = 50
- 100 messages/min, 1-minute tolerance → threshold = 100

### RSSI Thresholds by Use Case

| Use Case | Threshold | Reasoning |
|----------|-----------|-----------|
| Real-time voice/video | -70 dBm | Need strong signal for low latency |
| Chat/messaging | -85 dBm | Can tolerate some packet loss |
| Background sync | -90 dBm | Delay-tolerant, maximize coverage |

## Troubleshooting

### Problem: Transport switches too frequently

**Symptoms:** Seeing `transport_switched` events every few seconds

**Solutions:**
1. Increase `switchHysteresis` (try 20-30)
2. Increase `switchCooldownSecs` (try 30-60)
3. Increase `stabilityWindowSecs` (try 15-20)

### Problem: Stuck on poor BLE connection

**Symptoms:** High retry counts, poor delivery rate, but no escalation

**Solutions:**
1. Lower `bleToWifiRetryThreshold` (try 1)
2. Raise `rssiSwitchThreshold` (try -75 to -80)
3. Lower `congestionQueueThreshold` (try 20-30)

### Problem: WiFi Direct not engaging

**Symptoms:** BLE failures but no escalation to WiFi Direct

**Solutions:**
1. Verify WiFi Direct transport is enabled in config
2. Check `bleToWifiRetryThreshold` - may need to be lower
3. Monitor escalation events - may be triggering but platform not responding

### Problem: Internet preferred but using mesh when online

**Symptoms:** Using BLE/WiFi even though Internet is connected

**Solutions:**
1. Set `preferOnline: true` in DORS config
2. Verify Internet transport is enabled and connected
3. Check Internet transport status via `getActiveTransports()`

## Best Practices

### 1. Start with Defaults

Use default DORS configuration first, then tune based on observed behavior:

```typescript
dors: {
  preferOnline: false, // Only change this based on your app architecture
}
```

### 2. Monitor Before Tuning

Collect metrics for at least a week before adjusting:
- Transport switch frequency
- Delivery success rate per transport
- Average latency per transport
- Battery impact

### 3. Tune One Parameter at a Time

Isolate the effect of each change:
1. Adjust one parameter
2. Deploy to test group
3. Monitor for 3-7 days
4. Evaluate impact
5. Repeat or revert

### 4. Test Edge Cases

Verify behavior in:
- Low signal areas (RSSI < -85 dBm)
- High congestion (many devices)
- Rapid movement (changing signal)
- Battery critical (< 20%)
- Network transitions (online → offline)

### 5. Document Your Tuning

Keep a log of changes and their effects:

```typescript
// DORS Config History
// 2024-01-15: Increased hysteresis to 20.0 - reduced flapping by 60%
// 2024-01-20: Lowered retry threshold to 1 - improved WiFi engagement by 40%
dors: {
  switchHysteresis: 20.0,
  bleToWifiRetryThreshold: 1,
}
```

## API Reference

### Getting Current Transport

```typescript
// Query active transports
const transports = await protocol.getActiveTransports();
console.log('Active:', transports); // ['ble', 'internet']
```

### Checking Escalation Status

The FFI exposes `should_escalate_to_wifi` which can be queried via the protocol's process loop. Escalation events are automatically emitted when DORS detects the need to switch to WiFi Direct.

### Updating Metrics Manually

Platform code automatically updates metrics, but you can trigger updates:

```kotlin
// Android - Update BLE metrics
nativeUpdateTransportMetrics(
    handle,
    transportType = 1, // BLE
    rssi = currentRssi,
    latencyMs = measuredLatency,
    bandwidthBps = estimatedBandwidth,
    congestion = queuePressure,
    queueDepth = pendingMessages,
    successCount = successfulSends,
    failureCount = failedSends
)
```

```swift
// iOS - Update BLE metrics
offline_protocol_update_transport_metrics(
    handle,
    1, // BLE
    currentRssi,
    measuredLatency,
    estimatedBandwidth,
    queuePressure,
    pendingMessages,
    successfulSends,
    failedSends
)
```

## Performance Considerations

### DORS Overhead

DORS scoring is fast:
- **Per-transport scoring**: < 0.5ms
- **Full selection**: < 2ms for 3 transports
- **Memory**: ~1KB per transport for history

### Battery Impact

DORS itself is battery-efficient, but transport choices affect overall consumption:

| Transport | Power Draw | DORS Recommendation |
|-----------|------------|---------------------|
| BLE | Low (10-50mW) | Preferred for energy efficiency |
| WiFi Direct | High (200-400mW) | Only when needed for bandwidth |
| Internet | Medium (50-200mW) | Depends on `preferOnline` setting |

DORS automatically factors energy efficiency into scoring (30% weight for BLE).

### Network Overhead

- Transport switching incurs no protocol overhead
- Metrics are cached and updated incrementally
- No extra messages sent for DORS coordination

## Integration Checklist

- [ ] Configure transports based on app needs (BLE, Internet, WiFi Direct)
- [ ] Set `preferOnline` based on architecture (hybrid vs pure mesh)
- [ ] Tune hysteresis/cooldown based on mobility patterns
- [ ] Set escalation threshold based on latency tolerance
- [ ] Monitor `transport_switched` events in production
- [ ] Adjust parameters based on real-world metrics
- [ ] Document final configuration for team

## Further Reading

- [Architecture Overview](architecture.md)
- [Transport Layer Details](../crates/offline-protocol-transport/README.md)
- [React Native Integration](../bindings/react-native/README.md)
- [Configuration Reference](configuration.md)

