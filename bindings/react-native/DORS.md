# DORS (Dynamic Offline Relay Switch)

## Overview

DORS is the intelligent transport selection engine at the heart of the Offline Protocol SDK. It automatically evaluates, selects, and switches between available transport layers — Internet, BLE Mesh, Wi-Fi Direct, Reticulum, and Nostr relays — based on real-time network conditions. The goal is to ensure optimal message delivery while balancing performance, reliability, and energy consumption.

## How DORS Works

DORS operates as a continuous decision-making system that monitors network conditions and dynamically adapts routing decisions. Rather than relying on a single transport, DORS evaluates all available options and selects the best one for each message based on multiple weighted factors.

### Transport availability

DORS only chooses among transports whose status is **Available**. The platform must report connection state so the core can update status:

- **Internet:** Call `internetStatusChanged(true)` only after the WebSocket is connected and authenticated; call `internetStatusChanged(false)` whenever the connection is closed or fails (including initial connection failure). If the platform does not call `internetStatusChanged(false)` when the relay is unreachable or the connection drops, Internet stays in the available set and DORS will keep selecting it instead of switching to BLE.
- **BLE:** Call `bleStatusChanged(true)` when BLE is ready; call `bleStatusChanged(false)` when BLE becomes unavailable.

When the current transport is no longer available (e.g. Internet disconnected), DORS switches immediately to the best remaining transport (e.g. BLE) without waiting for cooldown or hysteresis.

### The Decision Process

When a message needs to be sent, DORS follows this process:

1. **Gather Metrics**: Collect real-time data about each available transport (signal strength, queue depth, latency, success rates, battery level, etc.)
2. **Calculate Scores**: Compute a weighted score for each transport based on multiple factors
3. **Apply Hysteresis**: Check if the new best transport is significantly better than the current one
4. **Verify Stability**: Ensure the new transport has been consistently better over a time window
5. **Check Cooldown**: Prevent rapid switching by enforcing a minimum time between switches
6. **Select Transport**: Route the message through the selected transport

## Multi-Factor Scoring System

DORS evaluates each transport using seven key factors, each scored from 0 to 100:

### Signal Strength
Measures the quality of the wireless connection using RSSI (Received Signal Strength Indicator) for BLE and Wi-Fi Direct. Stronger signals indicate more reliable links with lower packet loss.

- Excellent (-50 dBm or better): 100 points
- Good (-70 to -50 dBm): 70-100 points
- Fair (-85 to -70 dBm): 40-70 points
- Poor (below -85 dBm): 0-40 points

### Proximity
Reflects how close the message is to its destination, measured by hop count. Fewer hops mean faster delivery and less network load.

- Direct connection (0 hops): 100 points
- Score decreases proportionally with hop count
- Messages that have traveled many hops get lower scores

### Bandwidth
Measures the throughput capability of each transport. Higher bandwidth transports are preferred for larger messages or time-sensitive data.

- Reticulum (LoRa): ~0.7 KB/s typical, ~2.7 KB/s peak (20 points default)
- BLE: ~150 KB/s baseline (40 points default)
- Wi-Fi Direct: ~2 MB/s baseline (90 points default)
- Internet: 50 points by default, or 70 when `preferOnline` is enabled

### Congestion
Indicates how backed up the transport queue is. Less congested paths receive higher scores to distribute load and reduce latency.

- Queue depth is compared against a configurable threshold
- Historical averages smooth out transient spikes
- Higher congestion results in lower scores

### Energy Efficiency
Considers the battery impact of each transport. On battery-constrained devices, energy-efficient options are preferred.

- BLE: Low power (90 points baseline)
- Reticulum: Low-medium power (75 points baseline)
- Internet: Medium power (60 points baseline)
- Wi-Fi Direct: High power (40 points baseline)
- Devices that are charging get a bonus
- Low battery devices strongly prefer BLE or Reticulum

### Reliability
Tracks the historical success rate of message delivery for each transport. Transports with higher delivery rates are preferred.

- Based on recent delivery success ratio
- Factors in both ACK-confirmed deliveries and failures
- Uses historical averages to smooth out transient issues

### Load Capacity
Measures the available capacity on the transport considering current queue utilization and relay connection counts.

- Considers average queue depth over time
- Factors in drop rates if messages are being discarded
- Active relays that are near saturation get penalized

## Transport-Specific Weighting

Each transport type has a different weighting formula that reflects its characteristics:

### BLE Transport
Optimized for energy efficiency and signal quality in short-range mesh scenarios.

| Factor | Weight |
|--------|--------|
| Signal | 30% |
| Energy | 30% |
| Congestion | 15% |
| Proximity | 15% |
| Reliability | 5% |
| Load | 5% |

### Wi-Fi Direct Transport
Optimized for high throughput and direct peer connections.

| Factor | Weight |
|--------|--------|
| Bandwidth | 35% |
| Proximity | 20% |
| Congestion | 20% |
| Reliability | 15% |
| Load | 10% |

### Internet Transport
Optimized for server connectivity when available, with optional preference boost.

| Factor | Weight |
|--------|--------|
| Bandwidth | 35% |
| Reliability | 30% |
| Congestion | 15% |
| Energy | 10% |
| Load | 10% |

The Internet transport receives a baseline bonus (10 points by default, or 25 points if `preferOnline` is enabled) to ensure it's competitive when connected.

### Reticulum Transport
Optimized for resilience and long-range delivery via LoRa and other Reticulum mediums.

| Factor | Weight |
|--------|--------|
| Reliability | 30% |
| Energy | 25% |
| Proximity | 20% |
| Congestion | 15% |
| Signal | 5% |
| Bandwidth | 5% |

Reticulum has no base score bonus and a low tie-break priority. The full order is Internet > WiFi Direct > BLE > Reticulum > Nostr, so Nostr (not Reticulum) is the lowest-priority transport. Reticulum acts as a resilience fallback when other transports are unavailable or degraded.

## Switching Safeguards

DORS includes three mechanisms to prevent rapid transport switching ("flapping"), which can degrade performance and waste resources:

### Hysteresis
The new transport must score significantly higher than the current transport before switching. By default, this threshold is 15 points. This prevents switching on minor score fluctuations.

### Cooldown Period
After switching transports, DORS waits for a cooldown period (default: 20 seconds) before allowing another switch. This gives the new transport time to stabilize and prevents oscillation.

### Stability Window
Before switching, DORS verifies that the new transport has been consistently better over a time window (default: 8 seconds). This ensures the score improvement isn't just a momentary spike.

## Escalation Logic

DORS can automatically escalate from BLE to Wi-Fi Direct when BLE performance degrades. Escalation triggers include:

### Retry Failures
When BLE message delivery fails repeatedly (default: 2 consecutive failures), DORS suggests escalating to Wi-Fi Direct for more reliable delivery.

### Poor Signal Duration
If BLE RSSI stays below the threshold (default: -85 dBm) for an extended period (default: 10 seconds), DORS recommends switching to a higher-power transport.

### Queue Congestion
When the BLE queue depth exceeds the threshold (default: 50 messages) for a sustained period (default: 10 seconds), DORS escalates to handle the backlog.

### TTL Exhaustion
If messages are approaching TTL exhaustion (default: TTL ≤ 2), DORS escalates to ensure delivery before the message expires.

### Battery-Aware Escalation
DORS respects battery constraints. If the device battery is below the minimum relay level (default: 30%) and not charging, high-power transport escalation is blocked to preserve battery life. However, critical priority messages can bypass this restriction.

## Emergency Switching

In severe degradation scenarios, DORS can bypass normal hysteresis and cooldown for emergency switching:

- Success rate drops below 30%
- Retry failures exceed the threshold
- Very poor signal (below -90 dBm) persists for extended periods

This ensures messages still get delivered even when normal conditions aren't met.

## Configuration in React Native

Configure DORS when initializing the protocol:

```typescript
const config = {
  appId: 'my-app',
  userId: userId,
  
  dors: {
    preferOnline: false,           // Use mesh first, Internet as backup
    switchHysteresis: 15.0,        // Score improvement needed to switch
    switchCooldownSecs: 20,        // Wait time between switches
    bleToWifiRetryThreshold: 2,    // BLE failures before Wi-Fi Direct
    rssiSwitchThreshold: -85,      // Poor signal threshold (dBm)
    congestionQueueThreshold: 50,  // Queue depth for congestion
    stabilityWindowSecs: 8,        // Stability verification window
  },
};
```

## Configuration Reference

| Parameter | Default | Description |
|-----------|---------|-------------|
| `switchHysteresis` | 15.0 | Minimum score improvement to trigger switch |
| `switchCooldownSecs` | 20 | Wait time after switching before another switch |
| `bleToWifiRetryThreshold` | 2 | BLE failures before suggesting Wi-Fi Direct |
| `rssiSwitchThreshold` | -85 dBm | RSSI threshold for poor signal detection |
| `congestionQueueThreshold` | 50 | Queue depth indicating high congestion |
| `stabilityWindowSecs` | 8 | Duration to verify transport stability |
| `poorSignalDurationSecs` | 10 | Seconds RSSI must stay low before escalating |
| `ttlEscalationThreshold` | 2 | TTL value considered near exhaustion |
| `congestionDurationSecs` | 10 | Congestion persistence before escalating |
| `ttlEscalationHoldSecs` | 20 | How long TTL escalation flag stays active |
| `historyWindowSize` | 10 | Number of historical samples for smoothing |
| `queueRecoveryRatio` | 0.5 | Queue ratio that clears congestion flag |
| `preferOnline` | false | Prefer Internet transport when available |

## Listening to Transport Events

Monitor DORS decisions in your React Native app:

```typescript
protocol.on('transport_switched', (event) => {
  console.log(`Transport: ${event.from} → ${event.to}`);
  console.log(`Reason: ${event.reason}`);
});
```

## Use Cases

### Offline-First Applications
Set `preferOnline: false` and tune for aggressive switching to adapt quickly to changing mesh conditions. Lower hysteresis and cooldown for emergency scenarios.

### Hybrid Applications
Set `preferOnline: true` to use server infrastructure when available while seamlessly falling back to mesh when offline.

### Battery-Constrained Devices
Increase hysteresis and cooldown to minimize transport switching overhead. The energy scoring will naturally prefer BLE over Wi-Fi Direct.

### High-Throughput Scenarios
Lower congestion thresholds and enable faster escalation to Wi-Fi Direct for bandwidth-intensive applications.

## Performance Characteristics

- **Scoring Overhead**: Less than 0.5ms per transport
- **Full Selection**: Less than 2ms for evaluating 3 transports
- **Memory Usage**: Approximately 1KB per transport for historical metrics
- **Battery Impact**: DORS itself is lightweight; transport choices have the primary impact

DORS is designed to be a transparent, intelligent layer that optimizes message routing without requiring application-level intervention.
