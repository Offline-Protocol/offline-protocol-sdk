# Configuration Guide

Complete guide to configuring the Offline Protocol SDK for different use cases.

## Configuration Structure

```typescript
{
  // Required fields
  appId: string,
  userId: string,
  
  // Optional configurations
  transport?: TransportConfig,
  encryption?: EncryptionConfig,  // NEW: Auto-encryption settings
  dors?: DorsConfig,
  relay?: RelayConfig,
  path?: PathConfig,
  reliability?: ReliabilityConfig,
  network?: NetworkConfig,
}
```

## Use Case Configurations

### 1. Emergency Response App

**Requirements**: Maximum coverage, offline-only, high reliability.

```typescript
{
  appId: 'emergency-responder',
  userId: userId,
  
  transport: {
    bleEnabled: true,
    wifiDirectEnabled: true,
    internetEnabled: false,  // Offline only
  },
  
  dors: {
    preferOnline: false,
    switchHysteresis: 10,  // More aggressive switching
    ble_to_wifi_retry_threshold: 1,  // Switch faster
    congestionDurationSecs: 5,  // Require 5s sustained congestion before escalating
    ttlEscalationHoldSecs: 30,  // Keep TTL alarm active for 30s
  },
  
  relay: {
    allowRelay: true,
    relayPriority: 'always',  // Always try to be relay
    minBatteryForRelay: 15,   // Lower threshold for emergencies
    relayThreshold: 2,        // More relays
  },
  
  network: {
    initialTtl: 10,  // Higher TTL for wider coverage
  },
  
  reliability: {
    ack: {
      defaultTimeoutMs: 10000,  // Longer timeout
    },
    retry: {
      maxRetries: 5,  // More retries
      outboxMaxLifetimeMs: 86400000,  // 24 hours
    }
  }
}
```

### 2. Messaging App (Hybrid Mode)

**Requirements**: Online-first, automatic offline fallback, end-to-end encryption.

```typescript
{
  appId: 'chat-app',
  userId: userId,
  
  transport: {
    bleEnabled: true,
    wifiDirectEnabled: true,
    internetEnabled: true,
  },
  
  // Auto-encryption enabled by default
  encryption: {
    enabled: true,           // Messages automatically encrypted
    autoKeyExchange: true,   // Key packages exchanged on discovery
    storePending: true,      // Queue messages until session established
  },
  
  dors: {
    preferOnline: true,  // Online-first
    switchHysteresis: 15,
    switchCooldownSecs: 20,
    historyWindowSize: 12,
  },
  
  relay: {
    allowRelay: true,
    relayPriority: 'auto',
    minBatteryForRelay: 30,
  },
  
  network: {
    initialTtl: 8,  // Standard TTL
  }
}
```

### 3. File Sharing App

**Requirements**: High bandwidth, efficient chunking.

```typescript
{
  appId: 'file-share',
  userId: userId,
  
  transport: {
    bleEnabled: false,  // BLE too slow for large files
    wifiDirectEnabled: true,  // Prefer high bandwidth
    internetEnabled: true,
  },
  
  dors: {
    preferOnline: true,
    ble_to_wifi_retry_threshold: 1,  // Quick escalation to WiFi
    queueRecoveryRatio: 0.4,  // De-escalate when queues recover to 40%
  },
  
  relay: {
    allowRelay: true,
    minBatteryForRelay: 40,  // Higher for heavy traffic
  },
  
  fileTransfer: {
    chunkSize: 64 * 1024,  // 64KB chunks for faster transfer
    maxFileSize: 500 * 1024 * 1024,  // 500MB max
  }
}
```

### 4. Battery-Conscious App

**Requirements**: Minimize power consumption.

```typescript
{
  appId: 'battery-saver-app',
  userId: userId,
  
  transport: {
    bleEnabled: true,  // Low power
    wifiDirectEnabled: false,  // Avoid high power WiFi
    internetEnabled: true,
  },
  
  dors: {
    preferOnline: true,  // Internet when available
  },
  
  relay: {
    allowRelay: false,  // Don't relay to save battery
    relayPriority: 'never',
  },
  
  // Or if relay needed:
  relay: {
    allowRelay: true,
    relayPriority: 'auto',
    minBatteryForRelay: 50,  // Only relay with good battery
  }
}
```

### 5. Crowded Event (Dense Network)

**Requirements**: High congestion, many devices.

```typescript
{
  appId: 'event-app',
  userId: userId,
  
  transport: {
    bleEnabled: true,
    wifiDirectEnabled: true,
    internetEnabled: true,
  },
  
  dors: {
    congestionQueueThreshold: 30,  // Lower threshold
    rssiSwitchThreshold: -80,  // Switch earlier on poor signal
  },
  
  path: {
    forwardToTopK: 2,  // Fewer relays to reduce congestion
    maxCongestionLevel: 0.6,  // Stricter congestion filtering
  },
  
  relay: {
    relayThreshold: 5,  // Higher threshold (reduce relay count)
  },
  
  reliability: {
    retry: {
      maxRetries: 2,  // Fewer retries to reduce traffic
    },
    dedup: {
      maxTrackedMessages: 20000,  // Track more in dense network
    }
  }
}
```

## Configuration Parameters

### Transport Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `bleEnabled` | boolean | true | Enable BLE mesh |
| `wifiDirectEnabled` | boolean | true | Enable Wi-Fi Direct (Android only) |
| `internetEnabled` | boolean | true | Enable Internet |

### Encryption Configuration

Controls automatic MLS end-to-end encryption. See [MLS Integration Guide](./mls-integration.md) for details.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | boolean | true | Enable automatic encryption/decryption |
| `autoKeyExchange` | boolean | true | Auto-exchange key packages on peer discovery |
| `storePending` | boolean | true | Queue messages when no session exists |

**Example: Disable auto-encryption (use manual MLS APIs)**:
```typescript
{
  encryption: {
    enabled: false,
  }
}
```

**Example: Auto-encrypt but require explicit key exchange**:
```typescript
{
  encryption: {
    enabled: true,
    autoKeyExchange: false,  // Must manually exchange key packages
    storePending: true,
  }
}
```

### DORS Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `preferOnline` | boolean | false | Prefer Internet when available |
| `switchHysteresis` | number | 15.0 | Min score improvement to switch |
| `switchCooldownSecs` | number | 20 | Cooldown after switching (seconds) |
| `bleToWifiRetryThreshold` | number | 2 | Retries before escalating |
| `rssiSwitchThreshold` | number | -85 | RSSI threshold (dBm) |
| `congestionQueueThreshold` | number | 50 | Queue depth for congestion |
| `stabilityWindowSecs` | number | 8 | Stability check window |
| `poorSignalDurationSecs` | number | 10 | Seconds RSSI must remain poor before escalating |
| `ttlEscalationThreshold` | number | 2 | TTL value considered near exhaustion |

### Relay Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `allowRelay` | boolean | true | Allow device to act as relay |
| `relayThreshold` | number | 3 | Min connections to be relay |
| `minBatteryForRelay` | number | 30 | Min battery % for relay |
| `relayPriority` | string | 'auto' | 'auto', 'always', or 'never' |

### Path Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `forwardToTopK` | number | 3 | Number of relays to forward to |
| `maxCongestionLevel` | number | 0.7 | Max congestion threshold (0-1) |

### Reliability Configuration

**ACK Config**:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `defaultTimeoutMs` | number | 5000 | ACK timeout (milliseconds) |
| `maxPendingAcks` | number | 1000 | Max pending ACKs |

**Retry Config**:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `maxRetries` | number | 3 | Max retry attempts |
| `initialDelayMs` | number | 1000 | Initial retry delay |
| `maxDelayMs` | number | 30000 | Max retry delay |
| `backoffMultiplier` | number | 2.0 | Backoff multiplier |
| `outboxMaxLifetimeMs` | number | 3600000 | Max message lifetime (1 hour) |

**Dedup Config**:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `maxTrackedMessages` | number | 10000 | Max message IDs to track |
| `retentionTimeSecs` | number | 3600 | Retention time (1 hour) |

### Network Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `initialTtl` | number | 8 | Initial TTL for messages |

## Validation Rules

The SDK validates configuration on creation:

1. **appId**: Must not be empty
2. **userId**: Must not be empty
3. **initialTtl**: Must be > 0
4. **Transports**: At least one must be enabled
5. **Battery**: `minBatteryForRelay` must be 0-100
6. **Thresholds**: `relayThreshold` must be > 0

## Platform-Specific Considerations

### Android

**Available Transports**: Internet, BLE, Wi-Fi Direct

**Permissions Required**:
- `BLUETOOTH`, `BLUETOOTH_SCAN`, `BLUETOOTH_CONNECT`
- `ACCESS_FINE_LOCATION` (for BLE scanning)
- `ACCESS_WIFI_STATE`, `NEARBY_WIFI_DEVICES`

### iOS

**Available Transports**: Internet, BLE (no Wi-Fi Direct)

**Permissions Required**:
- `NSBluetoothAlwaysUsageDescription`

**Recommended Config**:
```typescript
{
  transport: {
    bleEnabled: true,
    wifiDirectEnabled: false,  // Not available on iOS
    internetEnabled: true,
  }
}
```

### Web

**Available Transports**: Internet only

**Recommended Config**:
```typescript
{
  transport: {
    bleEnabled: false,        // Not available in browsers
    wifiDirectEnabled: false, // Not available in browsers
    internetEnabled: true,
  }
}
```

## Advanced Tuning

### Low Battery Optimization

```typescript
{
  relay: {
    minBatteryForRelay: 50,  // Only relay with good battery
  },
  dors: {
    // Prefer low-power transports
  }
}
```

### High Reliability

```typescript
{
  reliability: {
    retry: {
      maxRetries: 5,
      outboxMaxLifetimeMs: 86400000,  // 24 hours
    }
  },
  path: {
    forwardToTopK: 5,  // More redundancy
  }
}
```

### Low Latency

```typescript
{
  dors: {
    switchHysteresis: 5,  // Switch faster
    bleToWifiRetryThreshold: 1,  // Escalate immediately
  },
  reliability: {
    ack: {
      defaultTimeoutMs: 2000,  // Shorter timeout
    }
  }
}
```

## Environment-Specific Configs

### Dense Urban Area

- More relays, lower TTL
- Stricter congestion management
- BLE preferred (short range sufficient)

### Open Rural Area

- Fewer relays, higher TTL
- Wi-Fi Direct preferred (longer range)
- Higher battery thresholds

### Indoor Building

- Medium TTL
- BLE mesh optimal
- Many relays (walls attenuate signal)

### High-Speed Movement

- Quick transport switching
- Lower hysteresis
- Shorter stability window

