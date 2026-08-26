/**
 * Offline Protocol SDK for React Native
 *
 * @packageDocumentation
 */

import {
  AppRegistry,
  NativeModules,
  NativeEventEmitter,
  Platform,
  EmitterSubscription,
} from 'react-native';
import type {
  ProtocolConfig,
  SendMessageParams,
  ForwardMessageParams,
  ForwardMessageToGroupParams,
  SendConnectionRequestParams,
  AcceptConnectionRequestParams,
  RejectConnectionRequestParams,
  CancelConnectionRequestParams,
  SendFileParams,
  SendMediaParams,
  MediaMetadata,
  MeshWakeTaskData,
  ProtocolEvent,
  EventListener,
  EventType,
  NetworkTopology,
  BleDiagnostics,
  MessageDeliveryStats,
  TransportType,
  InternetTransportConfig,
  WifiDirectTransportConfig,
  FileProgress,
  ProtocolState,
  MessageReceivedEvent,
  BleTransportConfig,
  NostrTransportConfig,
  ReticulumTransportConfig,
  AckConfig,
  RetryConfig,
  DedupConfig,
  DedupStats,
  MeshRelayConfig,
  MeshRelayStats,
  MeshRelayTunables,
  MlsKeyPackage,
  MlsEncryptedMessage,
  MlsWelcome,
  MlsSessionInfo,
  MlsGroupInfo,
  GroupRichReadiness,
  RelaySyncState,
  GroupRelaySyncChangedEvent,
  EstablishmentState,
  TelemetryConfig,
  TelemetryListener,
  TelemetryRecord,
  TransportMetrics,
  RelayConfig,
  RelayPriority,
  InviteInfo,
} from './types';
import { ContentType, MessagePriority } from './types';
import {
  LINKING_ERROR,
  MESH_WAKE_TASK_KEY,
  ONE_SHOT_EVENT_TYPES,
} from './constants';

export * from './types';
export * from './constants';

const OfflineProtocolNativeModule = (NativeModules.OfflineProtocolModule
  ? NativeModules.OfflineProtocolModule
  : new Proxy(
      {},
      {
        get() {
          throw new Error(LINKING_ERROR);
        },
      }
    )) as any; // Type assertion to allow all native module methods including group management

/**
 * The vocabulary the native bridges speak. Matches the engine's own
 * `RelayPriority` since 0.22; the legacy `'low' | 'medium' | 'high'` spelling
 * is still accepted on input by {@link normalizeRelayPriority} and by both
 * bridges.
 */
type NativeRelayPriority = 'never' | 'auto' | 'always';

/**
 * Membership test for {@link ONE_SHOT_EVENT_TYPES}, built once rather than
 * scanned per event: `emitEvent` is on the path of every event that crosses
 * this bridge.
 */
const ONE_SHOT_EVENT_TYPE_SET: ReadonlySet<string> = new Set(
  ONE_SHOT_EVENT_TYPES
);

interface InitialRuntimeConfig {
  dors?: {
    preferOnline: boolean;
    switchHysteresis: number;
    switchCooldownSecs: number;
    bleToWifiRetryThreshold: number;
    rssiSwitchThreshold: number;
    congestionQueueThreshold: number;
    stabilityWindowSecs: number;
  };
  relay?: {
    allowRelay?: boolean;
    minBatteryForRelay?: number;
    relayPriority?: string;
  };
  reliability?: {
    ack?: AckConfig;
    retry?: RetryConfig;
    dedup?: DedupConfig;
  };
}

/**
 * Native configuration object structure expected by native modules.
 * This is the transformed version of ProtocolConfig optimized for native consumption.
 */
interface NativeConfig {
  appId: string;
  profile: string;
  bleEnabled: boolean;
  wifiDirectEnabled: boolean;
  internetEnabled: boolean;
  reticulumEnabled: boolean;
  nostrEnabled: boolean;
  nostrSealingEnabled: boolean;
  nostrColdContactEnabled: boolean;
  nostrUsernameDiscoveryEnabled: boolean;
  preferOnline: boolean;
  initialTtl: number;
  binaryWireEnabled: boolean;
  encryptionEnabled?: boolean;
  autoKeyExchange?: boolean;
  storePending?: boolean;
  requireEncryption?: boolean;
  compactEnvelopeEnabled: boolean;
  richPayloadEnabled: boolean;
  cryptoRecoveryEnabled: boolean;
  maxPendingPerPeer?: number;
  maxPendingGlobal?: number;
  pendingTtlMs?: number;
  overflowPolicy?: 'drop_oldest' | 'drop_newest';
  encryption: {
    enabled: boolean;
    autoKeyExchange: boolean;
    storePending: boolean;
    requireEncryption: boolean;
    compactEnvelopeEnabled: boolean;
    richPayloadEnabled: boolean;
    cryptoRecoveryEnabled: boolean;
    pendingQueue: {
      maxPendingPerPeer: number;
      maxPendingGlobal: number;
      pendingTtlMs: number;
      overflowPolicy: 'drop_oldest' | 'drop_newest';
    };
  };
  group?: {
    maxGroupMembers?: number;
    relayEnabled?: boolean;
    relayBroadcastEnabled?: boolean;
    enforceAdminCommits?: boolean;
  };
  meshRelay?: MeshRelayConfig;
  data?: {
    enabled?: boolean;
  };
  security?: {
    controlFreshnessEnforced?: boolean;
  };
  dors?: {
    preferOnline: boolean;
    switchHysteresis: number;
    switchCooldownSecs: number;
    bleToWifiRetryThreshold: number;
    minSuccessRateBeforeEscalation?: number;
    minBleSamplesBeforeSuccessRateEscalation?: number;
    rssiSwitchThreshold: number;
    congestionQueueThreshold: number;
    stabilityWindowSecs: number;
    poorSignalDurationSecs?: number;
    ttlEscalationThreshold?: number;
    congestionDurationSecs?: number;
    ttlEscalationHoldSecs?: number;
    historyWindowSize?: number;
    queueRecoveryRatio?: number;
  };
  relay?: {
    allowRelay?: boolean;
    minBatteryForRelay?: number;
    relayPriority?: string;
  };
  transports?: {
    ble?: BleTransportConfig;
    internet?: InternetTransportConfig;
    wifiDirect?: WifiDirectTransportConfig;
  };
  fileTransfer?: {
    chunkSize?: number;
    maxFileSize?: number;
  };
  reliability?: {
    ack?: AckConfig;
    retry?: RetryConfig;
    dedup?: DedupConfig;
  };
}

function sanitize<T extends object>(value: T | undefined | null): T | undefined {
  if (!value) {
    return undefined;
  }
  const cleanedEntries = Object.entries(value as Record<string, unknown>).filter(
    ([, entryValue]) => entryValue !== undefined && entryValue !== null
  );
  if (cleanedEntries.length === 0) {
    return undefined;
  }
  return Object.fromEntries(cleanedEntries) as T;
}

/**
 * Main Offline Protocol class
 *
 * **BLE Transport Management:**
 * BLE scanning, advertising, and peer communication are now managed automatically
 * at the native bindings level. When you call `start()`, BLE operations begin
 * automatically if BLE is enabled in the configuration. No manual BLE setup required.
 *
 * **Supported Transports:**
 * - BLE (Bluetooth Low Energy) - Automatically managed
 * - WiFi Direct - Peer-to-peer LAN transport
 * - Internet - WebSocket relay transport
 * - Reticulum - Long-range LoRa/packet-radio transport
 * - Nostr - Relay transport over the Nostr protocol
 *
 * DORS (the transport selector) scores and switches between whichever of these
 * are enabled in the configuration; you do not pick one manually.
 *
 * @example
 * ```typescript
 * const protocol = new OfflineProtocol({
 *   appId: 'my-app',
 *   // Selects which stored identity to run as. Local only — never on the
 *   // wire. Your identity to peers is the address derived below.
 *   profile: 'user123',
 * });
 *
 * // Listen for events
 * protocol.on('message_received', (event) => {
 *   console.log(`From ${event.sender}: ${event.content}`);
 * });
 *
 * // Start protocol (BLE automatically starts scanning and advertising)
 * await protocol.start();
 *
 * // This device's identity, derived from a key it generates for itself.
 * const myAddress = await protocol.localAddress();   // "off1q…"
 *
 * // Send message (routing handled automatically by DORS).
 * // `recipient` is the peer's address — get it from an invite/QR exchange
 * // or from `neighbor_discovered`.
 * const messageId = await protocol.sendMessage({
 *   recipient: 'off1qxqmvd7clnfvdknrt8nfvvgn5ytsmeu4us5lrr0g',
 *   content: 'Hello!',
 *   priority: MessagePriority.High,
 * });
 *
 * // Stop protocol (BLE automatically stops)
 * await protocol.stop();
 * ```
 */
export class OfflineProtocol {
  private eventEmitter: NativeEventEmitter;
  private eventSubscription: EmitterSubscription | null = null;
  private telemetrySubscription: EmitterSubscription | null = null;
  private telemetryListeners: Set<TelemetryListener> = new Set();
  private eventListeners: Map<EventType | "all", Set<EventListener>> =
    new Map();
  /**
   * One-shot events that reached this instance while no listener was
   * registered for them, held for the first listener that is.
   *
   * Keyed by event type, so a repeat collapses last-wins and the map is
   * bounded by {@link ONE_SHOT_EVENT_TYPES} — two entries — by construction.
   * See {@link OfflineProtocol.on} for the window this closes and why the
   * native Android buffer cannot close it.
   */
  private pendingOneShotEvents: Map<EventType, ProtocolEvent> = new Map();
  /**
   * Event types already reported as dropped-with-no-listeners, so the warning
   * in {@link OfflineProtocol.emitEvent} fires once per type rather than once
   * per event. A misconfigured integration produces a steady stream of these.
   */
  private droppedEventTypesWarned: Set<string> = new Set();
  private config: ProtocolConfig;

  /**
   * This device's derived address, once known.
   *
   * Cached from the `identity_ready` event so callers that need to compare
   * against "us" do not have to await a native round-trip.
   */
  private cachedLocalAddress: string | null = null;
  private isCreated: boolean = false;
  private initialRuntimeConfig: InitialRuntimeConfig | null = null;
  private initialRuntimeConfigApplied: boolean = false;

  /**
   * Creates a new OfflineProtocol instance
   *
   * @param config - Protocol configuration
   */
  constructor(config: ProtocolConfig) {
    this.config = config;
    this.eventEmitter = new NativeEventEmitter(OfflineProtocolNativeModule);
    this.setupEventSubscription();
    this.setupTelemetrySubscription();
  }

  /**
   * Transforms the TypeScript config structure to the format expected by native modules
   */
  private transformConfigForNative(): NativeConfig {
    const dorsSource = this.config.dors;
    const relaySource = this.config.relay;

    const dorsConfig = dorsSource
      ? sanitize({
          preferOnline: dorsSource.preferOnline ?? false,
          switchHysteresis: dorsSource.switchHysteresis ?? 15.0,
          switchCooldownSecs: dorsSource.switchCooldownSecs ?? 20,
          bleToWifiRetryThreshold: dorsSource.bleToWifiRetryThreshold ?? 2,
          minSuccessRateBeforeEscalation: dorsSource.minSuccessRateBeforeEscalation ?? 0.3,
          minBleSamplesBeforeSuccessRateEscalation:
            dorsSource.minBleSamplesBeforeSuccessRateEscalation ?? 5,
          rssiSwitchThreshold: dorsSource.rssiSwitchThreshold ?? -85,
          congestionQueueThreshold: dorsSource.congestionQueueThreshold ?? 50,
          stabilityWindowSecs: dorsSource.stabilityWindowSecs ?? 8,
          poorSignalDurationSecs: dorsSource.poorSignalDurationSecs ?? 10,
          ttlEscalationThreshold: dorsSource.ttlEscalationThreshold ?? 2,
          congestionDurationSecs: dorsSource.congestionDurationSecs ?? 10,
          ttlEscalationHoldSecs: dorsSource.ttlEscalationHoldSecs ?? 20,
          historyWindowSize: Math.max(1, dorsSource.historyWindowSize ?? 10),
          queueRecoveryRatio: Math.min(
            1,
            Math.max(0, dorsSource.queueRecoveryRatio ?? 0.5)
          ),
          lowBatteryThreshold: dorsSource.lowBatteryThreshold ?? 20,
          relayMinBatteryLevel: dorsSource.relayMinBatteryLevel ?? 30,
          relayOptimalConnectionCount:
            dorsSource.relayOptimalConnectionCount ?? 4,
        })
      : undefined;

    const relayConfig = relaySource
      ? sanitize({
          allowRelay: relaySource.allowRelay,
          minBatteryForRelay: relaySource.minBatteryForRelay,
          relayPriority: relaySource.relayPriority,
        })
      : undefined;

    const reliabilityConfig = this.config.reliability
      ? sanitize({
          ack: sanitize(this.config.reliability.ack ?? {}),
          retry: sanitize(this.config.reliability.retry ?? {}),
          dedup: sanitize(this.config.reliability.dedup ?? {}),
        })
      : undefined;

    this.initialRuntimeConfig =
      dorsConfig || relayConfig || reliabilityConfig
        ? {
            dors: dorsConfig,
            relay: relayConfig,
            reliability: reliabilityConfig,
          }
        : null;
    this.initialRuntimeConfigApplied = false;

    // Effective encryption posture. The sibling flags default to the value
    // of `enabled` so that `encryption: { enabled: false }` alone yields the
    // fully-disabled posture (mirroring Rust's EncryptionConfig::disabled());
    // with unconditional all-true defaults it would instead trip the Rust
    // create()-time validation that rejects requireEncryption without
    // enabled. Explicit values always win over these derived defaults.
    const encryptionSource = this.config.encryption;
    const encryptionOn = encryptionSource?.enabled ?? true;
    const encryption = {
      enabled: encryptionOn,
      autoKeyExchange: encryptionSource?.autoKeyExchange ?? encryptionOn,
      storePending: encryptionSource?.storePending ?? encryptionOn,
      // Fail-closed default (SEC-M3): plaintext operation is an explicit opt-out.
      requireEncryption: encryptionSource?.requireEncryption ?? encryptionOn,
      compactEnvelopeEnabled: encryptionSource?.compactEnvelopeEnabled ?? true,
      richPayloadEnabled: encryptionSource?.richPayloadEnabled ?? true,
      cryptoRecoveryEnabled: encryptionSource?.cryptoRecoveryEnabled ?? true,
      pendingQueue: {
        maxPendingPerPeer: encryptionSource?.pendingQueue?.maxPendingPerPeer ?? 64,
        maxPendingGlobal: encryptionSource?.pendingQueue?.maxPendingGlobal ?? 4096,
        pendingTtlMs: encryptionSource?.pendingQueue?.pendingTtlMs ?? 1800000,
        overflowPolicy:
          encryptionSource?.pendingQueue?.overflowPolicy ?? 'drop_oldest',
      },
    };

    const nativeConfig: NativeConfig = {
      appId: this.config.appId,
      profile: this.config.profile,
      bleEnabled: this.config.transports?.ble?.enabled ?? true,
      wifiDirectEnabled: this.config.transports?.wifiDirect?.enabled ?? false,
      internetEnabled: this.config.transports?.internet?.enabled ?? false,
      reticulumEnabled: this.config.transports?.reticulum?.enabled ?? false,
      nostrEnabled: this.config.transports?.nostr?.enabled ?? false,
      // Nested `transports.nostr.sealingEnabled` is the documented home; the
      // flat `nostrSealingEnabled` the bridges read carries the same value.
      nostrSealingEnabled: this.config.transports?.nostr?.sealingEnabled ?? true,
      // Same nested-is-the-documented-home shape as sealingEnabled above.
      nostrColdContactEnabled:
        this.config.transports?.nostr?.coldContactEnabled ?? true,
      // Off by default, unlike the two above: publishing a username claim is
      // materially more disclosure than publishing a key-package record.
      nostrUsernameDiscoveryEnabled:
        this.config.transports?.nostr?.usernameDiscoveryEnabled ?? false,
      preferOnline: dorsSource?.preferOnline ?? false,
      initialTtl: this.config.network?.initialTtl ?? 8,
      binaryWireEnabled: this.config.binaryWireEnabled ?? true,
      // The nested `encryption` object is the documented home for these
      // fields (what the native bridges read first); the flat keys carry the
      // same effective values for readers of the historical flat shape. Keep
      // the two in lockstep — they must never diverge.
      encryptionEnabled: encryption.enabled,
      autoKeyExchange: encryption.autoKeyExchange,
      storePending: encryption.storePending,
      requireEncryption: encryption.requireEncryption,
      compactEnvelopeEnabled: encryption.compactEnvelopeEnabled,
      richPayloadEnabled: encryption.richPayloadEnabled,
      cryptoRecoveryEnabled: encryption.cryptoRecoveryEnabled,
      maxPendingPerPeer: encryption.pendingQueue.maxPendingPerPeer,
      maxPendingGlobal: encryption.pendingQueue.maxPendingGlobal,
      pendingTtlMs: encryption.pendingQueue.pendingTtlMs,
      overflowPolicy: encryption.pendingQueue.overflowPolicy,
      encryption,
    };

    if (dorsConfig) {
      nativeConfig.dors = dorsConfig;
    }

    if (relayConfig) {
      nativeConfig.relay = relayConfig;
    }

    const transportsConfig = sanitize({
      ble: this.config.transports?.ble,
      internet: this.config.transports?.internet,
      wifiDirect: this.config.transports?.wifiDirect,
      nostr: this.config.transports?.nostr,
    });
    if (transportsConfig) {
      nativeConfig.transports = transportsConfig;
    }

    if (this.config.fileTransfer) {
      const fileTransferConfig = sanitize({
        chunkSize: this.config.fileTransfer.chunkSize,
        maxFileSize: this.config.fileTransfer.maxFileSize,
      });
      if (fileTransferConfig) {
        nativeConfig.fileTransfer = fileTransferConfig;
      }
    }

    // Group section: the nested `group` object is the documented home (what
    // the native bridges read first). Only forwarded when the app set
    // something — the native parsers default every field, and the broadcast
    // default (on, capability-gated) lives in the core config.
    if (this.config.group) {
      const groupConfig = sanitize({
        maxGroupMembers: this.config.group.maxGroupMembers,
        relayEnabled: this.config.group.relayEnabled,
        relayBroadcastEnabled: this.config.group.relayBroadcastEnabled,
        enforceAdminCommits: this.config.group.enforceAdminCommits,
      });
      if (groupConfig) {
        nativeConfig.group = groupConfig;
      }
    }

    // Mesh forwarding section. Nested only — there are no legacy flat
    // spellings to keep in step, because nothing has ever read these from
    // JavaScript. Forwarded field-for-field with no defaults filled in: an
    // omitted field must stay omitted all the way to the core, which is what
    // keeps the defaults in one place. `sanitize` drops the undefined entries
    // and collapses an all-empty section, so "set nothing" arrives as an
    // absent section rather than an object that overwrites with nulls.
    if (this.config.meshRelay) {
      const meshRelayConfig = sanitize({
        maxTtl: this.config.meshRelay.maxTtl,
        denseMaxTtl: this.config.meshRelay.denseMaxTtl,
        denseDegree: this.config.meshRelay.denseDegree,
        fanout: this.config.meshRelay.fanout,
        jitterMinMs: this.config.meshRelay.jitterMinMs,
        jitterMaxMs: this.config.meshRelay.jitterMaxMs,
        ratePerSec: this.config.meshRelay.ratePerSec,
        burst: this.config.meshRelay.burst,
        peerRatePerSec: this.config.meshRelay.peerRatePerSec,
        peerBurst: this.config.meshRelay.peerBurst,
        queueCapacity: this.config.meshRelay.queueCapacity,
        biasMinScale: this.config.meshRelay.biasMinScale,
        biasMaxHandicapMs: this.config.meshRelay.biasMaxHandicapMs,
        activityWindowMs: this.config.meshRelay.activityWindowMs,
        activityMinForwards: this.config.meshRelay.activityMinForwards,
        activityIdleWindows: this.config.meshRelay.activityIdleWindows,
      });
      if (meshRelayConfig) {
        nativeConfig.meshRelay = meshRelayConfig;
      }
    }

    // Data layer section. Same rule as meshRelay above: forwarded
    // field-for-field with no defaults filled in, so an omitted field stays
    // omitted all the way to the core and the default lives in exactly one
    // place. Writing `?? false` here would look harmless and would make the
    // Rust default unreachable.
    if (this.config.data) {
      const dataConfig = sanitize({
        enabled: this.config.data.enabled,
      });
      if (dataConfig) {
        nativeConfig.data = dataConfig;
      }
    }

    // Security section, forwarded the same way and for the same reason: an
    // omitted field stays omitted all the way to the core, so the default
    // lives in exactly one place. `controlFreshnessEnforced` defaults to true
    // in Rust, and writing that literal here is precisely how a default stops
    // being changeable for every app that never set it.
    //
    // Read from the nested home *or* the top-level spelling, and normalized
    // into the nested one on the way out. The native bridges honour both
    // because this flag is the lever an app reaches for when its fleet's
    // clocks are wrong and its control plane has gone quiet; that tolerance
    // has to start here, since a value this layer drops never reaches them to
    // be honoured. The section is built unconditionally for the same reason:
    // gating it on `config.security` existing is exactly what made the flat
    // spelling a no-op.
    const securityConfig = sanitize({
      controlFreshnessEnforced:
        this.config.security?.controlFreshnessEnforced ??
        this.config.controlFreshnessEnforced,
    });
    if (securityConfig) {
      nativeConfig.security = securityConfig;
    }

    if (reliabilityConfig) {
      nativeConfig.reliability = reliabilityConfig;
    }

    console.log(
      "[OfflineProtocol] Native config:",
      JSON.stringify(nativeConfig)
    );
    return nativeConfig;
  }

  private normalizeRelayPriority(
    priority?: string | null
  ): NativeRelayPriority | null {
    if (!priority) {
      return null;
    }
    const normalized = priority.toLowerCase();
    switch (normalized) {
      case "never":
      case "auto":
      case "always":
        return normalized as NativeRelayPriority;
      // Legacy spelling of the same three values, still accepted on input.
      case "low":
        return "never";
      case "medium":
        return "auto";
      case "high":
        return "always";
      default:
        return null;
    }
  }

  private async applyInitialRuntimeConfig(): Promise<void> {
    if (this.initialRuntimeConfigApplied) {
      return;
    }

    if (!this.initialRuntimeConfig) {
      this.initialRuntimeConfigApplied = true;
      return;
    }

    const { dors, relay, reliability } = this.initialRuntimeConfig;

    if (dors) {
      try {
        await OfflineProtocolNativeModule.updateDorsConfig(
          JSON.stringify(dors)
        );
      } catch (error) {
        console.warn(
          "[OfflineProtocol] Failed to apply DORS configuration",
          error
        );
      }
    }

    if (relay) {
      // The whole relay section, not just the priority: `allowRelay` and
      // `minBatteryForRelay` used to be dropped here, which left
      // `config.relay` on mobile permanently at its defaults.
      const normalizedPriority = this.normalizeRelayPriority(
        relay.relayPriority
      );
      const payload = {
        ...(relay.allowRelay !== undefined
          ? { allowRelay: relay.allowRelay }
          : {}),
        ...(relay.minBatteryForRelay !== undefined
          ? { minBatteryForRelay: relay.minBatteryForRelay }
          : {}),
        ...(normalizedPriority ? { relayPriority: normalizedPriority } : {}),
      };
      if (Object.keys(payload).length > 0) {
        try {
          await OfflineProtocolNativeModule.updateRelayConfig(
            JSON.stringify(payload)
          );
        } catch (error) {
          console.warn(
            "[OfflineProtocol] Failed to apply relay configuration",
            error
          );
        }
      }
    }

    if (reliability?.ack) {
      try {
        await OfflineProtocolNativeModule.updateAckConfig(
          JSON.stringify(reliability.ack)
        );
      } catch (error) {
        console.warn(
          "[OfflineProtocol] Failed to apply ACK configuration",
          error
        );
      }
    }

    if (reliability?.retry) {
      try {
        await OfflineProtocolNativeModule.updateRetryConfig(
          JSON.stringify(reliability.retry)
        );
      } catch (error) {
        console.warn(
          "[OfflineProtocol] Failed to apply retry configuration",
          error
        );
      }
    }

    if (reliability?.dedup) {
      try {
        await OfflineProtocolNativeModule.updateDedupConfig(
          JSON.stringify(reliability.dedup)
        );
      } catch (error) {
        console.warn(
          "[OfflineProtocol] Failed to apply dedup configuration",
          error
        );
      }
    }

    this.initialRuntimeConfigApplied = true;
  }

  /**
   * Sets up the native event subscription
   */
  private setupEventSubscription(): void {
    this.eventSubscription = this.eventEmitter.addListener(
      "OfflineProtocol_Event",
      (data: { eventJson: string }) => {
        try {
          const event = JSON.parse(data.eventJson) as ProtocolEvent;
          this.emitEvent(event);
        } catch (error) {
          console.error("Failed to parse event JSON:", error);
        }
      }
    );
  }

  /**
   * Sets up the native telemetry subscription. Telemetry events arrive with
   * a `category` discriminator; this normalizes them to the TelemetryRecord
   * union and fans them out to registered listeners.
   */
  private setupTelemetrySubscription(): void {
    this.telemetrySubscription = this.eventEmitter.addListener(
      "OfflineProtocol_Telemetry",
      (data: unknown) => {
        try {
          this.dispatchTelemetry(data as TelemetryRecord);
        } catch (error) {
          console.error("Failed to dispatch telemetry record:", error);
        }
      }
    );
  }

  private dispatchTelemetry(record: TelemetryRecord): void {
    this.telemetryListeners.forEach((listener) => {
      try {
        listener(record);
      } catch (error) {
        console.error("Error in telemetry listener:", error);
      }
    });
  }

  /**
   * Emits an event to all registered listeners, holding it for redelivery if
   * it is *one-shot* and nothing was listening.
   *
   * This is the only place in the SDK that can tell whether an event reached
   * the **app**. Everything upstream — the native `canEmitToJs` gate, the
   * Android sticky buffer's redelivery, the iOS latch restatement — answers
   * the narrower question of whether the event reached *JavaScript*, which is
   * a different registration: the SDK subscribes to the native emitter in its
   * own constructor, before the app can possibly have called {@link on}. So
   * an event can pass every native check, arrive here intact, and be dropped
   * with both listener maps empty. See {@link on} for the redelivery half.
   */
  private emitEvent(event: ProtocolEvent): void {
    if (event.type === 'identity_ready') {
      this.cachedLocalAddress = event.address;
    }

    let delivered = false;

    // Call event-specific listeners
    const specificListeners = this.eventListeners.get(event.type);
    if (specificListeners) {
      specificListeners.forEach((listener) => {
        delivered = true;
        try {
          listener(event);
        } catch (error) {
          console.error(`Error in event listener for ${event.type}:`, error);
        }
      });
    }

    // Call 'all' event listeners
    const allListeners = this.eventListeners.get("all");
    if (allListeners) {
      allListeners.forEach((listener) => {
        delivered = true;
        try {
          listener(event);
        } catch (error) {
          console.error("Error in event listener for all events:", error);
        }
      });
    }

    // A listener that *threw* still counts as delivered: the event was handed
    // to the app, and holding it for redelivery would hand a throwing handler
    // the same event again on the next registration. This mirrors the native
    // dispatcher, where the emit — not the outcome — is what discards the
    // hold.
    if (delivered) {
      // Anything held for this type is now stale news; left behind it would
      // redeliver after the event that superseded it.
      this.pendingOneShotEvents.delete(event.type);
      return;
    }

    if (ONE_SHOT_EVENT_TYPE_SET.has(event.type)) {
      // Delete first so a re-hold moves to the tail: replay order should be
      // the order the events were emitted, and the newest information about a
      // type is also the newest information overall.
      this.pendingOneShotEvents.delete(event.type);
      this.pendingOneShotEvents.set(event.type, event);
      return;
    }

    // Everything else is periodic, re-derivable, or followed by another event
    // carrying the same state, so dropping it is correct — but dropping it
    // *silently* while the app has registered nothing at all is
    // indistinguishable from no event having arrived, which is the failure
    // this warning exists to make visible. Only fires while the app has no
    // listeners whatsoever; an app that listens selectively is making a
    // choice, not a mistake.
    if (
      this.eventListeners.size === 0 &&
      !this.droppedEventTypesWarned.has(event.type)
    ) {
      this.droppedEventTypesWarned.add(event.type);
      console.warn(
        `[OfflineProtocol] Dropped a '${event.type}' event: it arrived before ` +
          `any listener was registered. Register listeners with on(...) ` +
          `immediately after construction, before calling start(). ` +
          `(Further drops of this type are not reported.)`
      );
    }
  }

  /**
   * Registers an event listener
   *
   * **One-shot events registered for late are replayed.** An event that
   * reached the SDK before any listener for it existed is held and delivered
   * to the first listener that registers — but only for the *one-shot* tags
   * (`internet_session_superseded`, `mesh_stopped_by_user`), which nothing
   * else ever restates. Everything else is dropped, as it should be: a
   * periodic event replayed after the fact would report a state that has
   * since changed.
   *
   * That window is not an edge case, it is the default. The SDK subscribes to
   * the native emitter inside its own constructor, so between
   * `new OfflineProtocol(...)` and your first `on(...)` there is a stretch in
   * which events arrive with nothing registered — and Android's native
   * redelivery of held one-shot events fires on exactly that constructor-time
   * subscribe, landing squarely inside it. Registering synchronously right
   * after construction keeps the window at zero and is still the right habit;
   * this hold is what makes an `await` in between survivable.
   *
   * Replay is **asynchronous** (a microtask), so a handler never runs before
   * the `on(...)` call that registered it has returned, and every listener
   * registered in the same tick — including an `'all'` listener added after a
   * specific one — receives it. Delivery stays **at-least-once**: these events
   * are state, not edges, and handlers must be idempotent. See
   * `docs/react-native-integration.md` §6.1.
   *
   * @param eventType - Event type to listen for, or 'all' for all events
   * @param listener - Callback function
   * @returns This instance for chaining
   *
   * @example
   * ```typescript
   * protocol.on('message_received', (event) => {
   *   console.log('Message received:', event);
   * });
   *
   * protocol.on('all', (event) => {
   *   console.log('Any event:', event);
   * });
   * ```
   */
  on<T extends ProtocolEvent = ProtocolEvent>(
    eventType: EventType | "all",
    listener: EventListener<T>
  ): this {
    if (!this.eventListeners.has(eventType)) {
      this.eventListeners.set(eventType, new Set());
    }
    this.eventListeners.get(eventType)!.add(listener as EventListener);
    this.replayHeldOneShotEvents(eventType);
    return this;
  }

  /**
   * Hands any held one-shot event matching [eventType] to the listeners
   * registered for it, on the next microtask.
   *
   * Entries are removed from the hold *now*, when the replay is scheduled,
   * rather than when it runs: several `on(...)` calls in the same tick would
   * otherwise each schedule a replay of the same entry and the app would see
   * it once per registration.
   *
   * Delivery goes back through {@link emitEvent} rather than calling the new
   * listener directly, which buys two things. Every listener registered by
   * the time the microtask runs is served, not just this one — so the common
   * `on('mesh_stopped_by_user', ...)` followed by `on('all', ...)` does not
   * leave the second one short. And if the app removed its listeners again in
   * the interim, `emitEvent` simply re-holds the event instead of losing it,
   * which is the property that makes scheduling-time removal safe.
   */
  private replayHeldOneShotEvents(eventType: EventType | "all"): void {
    if (this.pendingOneShotEvents.size === 0) {
      return;
    }

    const replay: ProtocolEvent[] = [];
    if (eventType === "all") {
      replay.push(...this.pendingOneShotEvents.values());
      this.pendingOneShotEvents.clear();
    } else {
      const held = this.pendingOneShotEvents.get(eventType);
      if (held) {
        replay.push(held);
        this.pendingOneShotEvents.delete(eventType);
      }
    }

    if (replay.length === 0) {
      return;
    }

    // A microtask rather than a timer: it runs after the current synchronous
    // block — so a listener never fires before the `on(...)` that registered
    // it returns, and same-tick registrations all land first — while still
    // being the earliest point at which that is true.
    Promise.resolve().then(() => {
      replay.forEach((event) => this.emitEvent(event));
    });
  }

  /**
   * Removes an event listener
   *
   * @param eventType - Event type
   * @param listener - Callback function to remove
   * @returns This instance for chaining
   */
  off<T extends ProtocolEvent = ProtocolEvent>(
    eventType: EventType | "all",
    listener: EventListener<T>
  ): this {
    const listeners = this.eventListeners.get(eventType);
    if (listeners) {
      listeners.delete(listener as EventListener);
      if (listeners.size === 0) {
        this.eventListeners.delete(eventType);
      }
    }
    return this;
  }

  /**
   * Registers a one-time event listener
   *
   * @param eventType - Event type to listen for
   * @param listener - Callback function
   * @returns This instance for chaining
   */
  once<T extends ProtocolEvent = ProtocolEvent>(
    eventType: EventType | "all",
    listener: EventListener<T>
  ): this {
    const onceWrapper: EventListener<T> = (event) => {
      this.off(eventType, onceWrapper as EventListener);
      listener(event);
    };
    this.on(eventType, onceWrapper as EventListener);
    return this;
  }

  /**
   * Removes all listeners for a specific event type, or all listeners if no type specified
   *
   * @param eventType - Optional event type. If not provided, removes all listeners.
   * @returns This instance for chaining
   */
  removeAllListeners(eventType?: EventType | "all"): this {
    if (eventType) {
      this.eventListeners.delete(eventType);
    } else {
      this.eventListeners.clear();
    }
    return this;
  }

  /**
   * Starts the protocol
   *
   * **Automatic BLE Management:**
   * When called, this method automatically starts BLE operations if BLE is enabled:
   * - Starts scanning for nearby devices advertising the Offline Protocol service
   * - Starts advertising this device so others can discover it
   * - Begins polling for fragments to send
   * - Handles incoming fragments from peers
   *
   * **Automatic MLS Initialization:**
   * If encryption is enabled (default: true), MLS is automatically initialized with
   * platform-specific secure storage (iOS Keychain / Android EncryptedSharedPreferences).
   * To disable auto-initialization, set `encryption.enabled: false` in the config.
   *
   * **Permissions Required:**
   * - iOS: Bluetooth permissions (NSBluetoothAlwaysUsageDescription in Info.plist)
   * - Android: BLUETOOTH_SCAN, BLUETOOTH_ADVERTISE, BLUETOOTH_CONNECT (Android 12+)
   *           or BLUETOOTH, BLUETOOTH_ADMIN, ACCESS_FINE_LOCATION (Android 11 and below)
   *
   * @throws Error if protocol is already started or fails to start
   */
  async start(): Promise<void> {
    // Create protocol instance if not already created
    if (!this.isCreated) {
      const nativeConfig = this.transformConfigForNative();
      await OfflineProtocolNativeModule.create(JSON.stringify(nativeConfig));
      this.isCreated = true;
      await this.applyInitialRuntimeConfig();
    }

    // Initialize MLS before start() so key package exchange can run when peers are discovered.
    // If we start transports first, neighbor_discovered may fire before MLS is ready and no
    // key packages are sent, breaking the handshake.
    const encryptionEnabled = this.config.encryption?.enabled ?? true;
    if (encryptionEnabled) {
      try {
        await OfflineProtocolNativeModule.initializeMlsWithSecureStorage();
        console.log(
          "[OfflineProtocol] MLS auto-initialized with secure storage"
        );
        // Pull the address across now rather than waiting for the
        // `identity_ready` event to make its way back through the native
        // emitter. Anything that compares a peer against "us" — session
        // attribution especially — can run as soon as `start()` resolves, and
        // an empty cache in that window reads as "no id matches us".
        this.cachedLocalAddress =
          (await OfflineProtocolNativeModule.localAddress()) ?? null;
      } catch (error) {
        console.warn(
          "[OfflineProtocol] MLS initialization failed — secure sessions and handshake will not work:",
          error
        );
      }
    }

    await OfflineProtocolNativeModule.start();

    // A session is starting, so nothing held from before it can still be
    // true. Redelivering a stale one-shot event is not a milder version of
    // dropping it — it is the same failure inverted: `mesh_stopped_by_user`
    // handed to a listener that registers after this call tells the app the
    // mesh is down while it is coming up, and being one-shot, nothing will
    // ever correct it. This is the TypeScript half of the native
    // `beginSession()`; the native buffer's generation stamp cannot see this
    // window because by here the event has already left it.
    //
    // Nothing legitimate is swallowed. Anything an app *did* claim is gone
    // from the hold already — `replayHeldOneShotEvents` removes at scheduling
    // time, and a replay scheduled by a synchronous `on(...)` before this call
    // runs during the first `await` above. What remains is only what no
    // listener ever asked for. And neither enrolled event can be produced
    // ahead of this point in a fresh session: both need a transport that
    // `start()` is what brings up (the relay auto-enable below included, which
    // is why this sits above it).
    this.pendingOneShotEvents.clear();

    if (encryptionEnabled) {
      const mlsReady = await OfflineProtocolNativeModule.isMlsInitialized();
      if (!mlsReady) {
        console.warn(
          "[OfflineProtocol] Encryption enabled but MLS is not initialized — key exchange and secure sessions will not work"
        );
      }
    }

    // Auto-enable internet transport if configured with a server address
    const internetConfig = this.config.transports?.internet;
    if (internetConfig?.enabled && internetConfig?.serverAddress) {
      try {
        const enableConfig: InternetTransportConfig = {
          enabled: true,
          serverAddress: internetConfig.serverAddress,
          autoReconnect: internetConfig.autoReconnect ?? true,
        };
        // Include authToken if provided in config
        if (internetConfig.authToken) {
          enableConfig.authToken = internetConfig.authToken;
        }
        await this.enableTransport("internet", enableConfig);
        console.log("[OfflineProtocol] Internet transport auto-enabled");
      } catch (error) {
        console.warn(
          "[OfflineProtocol] Failed to auto-enable internet transport:",
          error
        );
      }
    }

    // Auto-enable nostr transport if configured with relay URLs
    const nostrConfig = this.config.transports?.nostr;
    if (nostrConfig?.enabled && nostrConfig?.relayUrls?.length) {
      try {
        await this.enableTransport("nostr", nostrConfig);
        console.log("[OfflineProtocol] Nostr transport auto-enabled");
      } catch (error) {
        // error, not warn: Nostr was asked for, is not running, and nothing
        // retries this — the app is offline over Nostr for the whole session
        // with no other signal that it happened.
        console.error(
          "[OfflineProtocol] Nostr transport is configured but was NOT enabled:",
          error
        );
        if (!encryptionEnabled) {
          // Almost certainly the cause, and the rejection cannot say so: it
          // reports the missing identity, not the config line that removed it.
          // Nostr's routing tag is derived from the identity MLS
          // initialization creates, and `encryption.enabled: false` skips that
          // initialization entirely.
          console.error(
            "[OfflineProtocol] Nostr requires the protocol identity created by MLS " +
              "initialization, which `encryption.enabled: false` skips. Set " +
              "`encryption.enabled: true` to use Nostr."
          );
        }
      }
    }

    // Auto-enable reticulum transport if configured
    const reticulumConfig = this.config.transports?.reticulum;
    if (reticulumConfig?.enabled) {
      try {
        await this.enableTransport("reticulum", reticulumConfig);
        console.log("[OfflineProtocol] Reticulum transport auto-enabled");
      } catch (error) {
        console.warn(
          "[OfflineProtocol] Failed to auto-enable reticulum transport:",
          error
        );
      }
    }
  }

  /**
   * Stops the protocol
   *
   * **Automatic BLE Management:**
   * When called, this method automatically stops all BLE operations:
   * - Stops scanning for devices
   * - Stops advertising
   * - Disconnects from all connected peers
   * - Cleans up BLE resources
   *
   * @throws Error if protocol is not started or fails to stop
   */
  async stop(): Promise<void> {
    await OfflineProtocolNativeModule.stop();
  }

  /**
   * Emits a test event to verify the event system is working
   *
   * This is a debugging method that emits a network_metrics event with all zeros.
   * Use this to verify that events are being delivered from Rust through the
   * native bridge to JavaScript.
   */
  async emitTestEvent(): Promise<void> {
    if (!this.isCreated) {
      const nativeConfig = this.transformConfigForNative();
      await OfflineProtocolNativeModule.create(JSON.stringify(nativeConfig));
      this.isCreated = true;
      await this.applyInitialRuntimeConfig();
    }
    await OfflineProtocolNativeModule.emitTestEvent();
  }

  /**
   * Sends a message
   *
   * @param params - Message parameters
   * @returns Message ID
   * @throws Error if message fails to send
   */
  async sendMessage(params: SendMessageParams): Promise<string> {
    const priority = params.priority ?? MessagePriority.Medium;

    // Rich params route to the rich native method; the plain path is left
    // untouched. Rich fields only ever travel inside the MLS-sealed rich
    // payload (recipients that support it), or are dropped — never cleartext.
    const hasRichOptions =
      params.replyContext !== undefined ||
      params.mediaMetadata !== undefined ||
      params.forwardInfo !== undefined ||
      params.contentType !== undefined;
    if (hasRichOptions) {
      const meta = params.mediaMetadata;
      const options = {
        content_type: params.contentType ?? null,
        reply_context: params.replyContext
          ? {
              sender: params.replyContext.sender,
              text: params.replyContext.text,
              timestamp: params.replyContext.timestamp ?? null,
              reply_media_label: params.replyContext.reply_media_label ?? null,
              reply_content_type:
                params.replyContext.reply_content_type ?? null,
            }
          : null,
        media_metadata: meta
          ? {
              mime_type: meta.mimeType,
              file_name: meta.fileName,
              file_size: meta.fileSize,
              duration_ms: meta.durationMs ?? null,
              width: meta.width ?? null,
              height: meta.height ?? null,
              thumbnail_base64: meta.thumbnailBase64 ?? null,
              media_id: meta.mediaId ?? null,
              download_url: meta.downloadUrl ?? null,
              thumbnail_url: meta.thumbnailUrl ?? null,
              encryption_key: meta.encryptionKey ?? null,
              iv: meta.iv ?? null,
              ciphertext_hash: meta.ciphertextHash ?? null,
              sticker_provider: meta.stickerProvider ?? null,
              sticker_remote_id: meta.stickerRemoteId ?? null,
              sticker_kind: meta.stickerKind ?? null,
            }
          : null,
        forward_info: params.forwardInfo
          ? {
              original_sender: params.forwardInfo.original_sender,
              original_message_id: params.forwardInfo.original_message_id,
              original_timestamp: params.forwardInfo.original_timestamp,
              forward_count: params.forwardInfo.forward_count,
            }
          : null,
      };
      return await OfflineProtocolNativeModule.sendMessageRich(
        params.recipient,
        params.content,
        priority,
        params.replyToMsg ?? null,
        options
      );
    }

    const messageId = await OfflineProtocolNativeModule.sendMessage(
      params.recipient,
      params.content,
      priority,
      params.replyToMsg ?? null
    );
    return messageId;
  }

  /**
   * Forwards a message to a new recipient with original sender attribution.
   *
   * Creates a new message with the original content and attaches forwarding
   * metadata tracking the original sender, message ID, timestamp, and forward count.
   *
   * @param params - Forward message parameters
   * @returns New message ID
   * @throws Error if forwarding fails
   */
  async forwardMessage(params: ForwardMessageParams): Promise<string> {
    const priority = params.priority ?? MessagePriority.Medium;
    const messageId = await OfflineProtocolNativeModule.forwardMessage(
      params.originalMessageJson,
      params.newRecipient,
      priority
    );
    return messageId;
  }

  /**
   * Sends a connection request
   *
   * `params.recipient` must be the target's canonical address (`off1…`) —
   * the value they derived from their own identity key, which is also what
   * `neighbor_discovered` reports as `peer_id`.
   *
   * The returned message id is the correlation key for the request's
   * outcome events: `connection_request_undeliverable` (recipient offline
   * or retry budget exhausted), `message_delivered` (reached the
   * recipient's device), and `message_failed` (generic retry exhaustion,
   * fires alongside the typed event). The recipient's answer arrives as
   * `connection_accepted` / `connection_rejected`, which correlate by peer
   * id (`accepted_by` / `rejected_by`), not by message id.
   *
   * @param params - Connection request parameters
   * @returns Message ID
   * @throws Error if request fails to send
   */
  async sendConnectionRequest(params: SendConnectionRequestParams): Promise<string> {
    const messageId = await OfflineProtocolNativeModule.sendConnectionRequest(
      params.recipient,
      params.senderName,
      params.keyPackage ?? null,
      params.initialMessage ?? null
    );
    return messageId;
  }

  /**
   * Accepts a connection request
   *
   * @param params - Connection acceptance parameters
   * @returns Message ID
   * @throws Error if acceptance fails to send
   */
  async acceptConnectionRequest(
    params: AcceptConnectionRequestParams
  ): Promise<string> {
    const messageId = await OfflineProtocolNativeModule.acceptConnectionRequest(
      params.recipient,
      params.accepterName,
      params.keyPackage ?? null
    );
    return messageId;
  }

  /**
   * Rejects a connection request
   *
   * @param params - Connection rejection parameters
   * @returns Message ID
   * @throws Error if rejection fails to send
   */
  async rejectConnectionRequest(
    params: RejectConnectionRequestParams
  ): Promise<string> {
    const messageId = await OfflineProtocolNativeModule.rejectConnectionRequest(
      params.recipient
    );
    return messageId;
  }

  /**
   * Cancels a previously sent connection request
   *
   * @param params - Connection cancellation parameters
   * @returns Message ID
   * @throws Error if cancellation fails to send
   */
  async cancelConnectionRequest(
    params: CancelConnectionRequestParams
  ): Promise<string> {
    const messageId = await OfflineProtocolNativeModule.cancelConnectionRequest(
      params.recipient
    );
    return messageId;
  }

  /**
   * Gets the list of active transports
   *
   * @returns Array of active transport type names
   */
  async getActiveTransports(): Promise<TransportType[]> {
    return await OfflineProtocolNativeModule.getActiveTransports();
  }

  /**
   * Enables a transport with optional configuration
   *
   * Re-enabling `'internet'` is also the recovery from a relay supersede: it
   * clears the transport's latch, so any `internet_session_superseded` this
   * instance is still holding for a late listener is dropped here. Held, it
   * would tell an app with a freshly reconnected relay socket that it is
   * connected elsewhere, with nothing to correct it. Mirrors the same discard
   * on the native side.
   *
   * @param type - Transport type to enable
   * @param config - Optional transport configuration
   * @throws Error if transport fails to enable
   */
  async enableTransport(
    type: TransportType,
    config?: InternetTransportConfig | WifiDirectTransportConfig | NostrTransportConfig | ReticulumTransportConfig
  ): Promise<void> {
    const result = await OfflineProtocolNativeModule.enableTransport(
      type,
      config
    );
    // Only on the success path: a failed enable leaves the latch set, so the
    // held event is still the truth.
    if (type === "internet") {
      this.pendingOneShotEvents.delete("internet_session_superseded");
    }
    return result;
  }

  /**
   * Disables a transport
   *
   * @param type - Transport type to disable
   * @throws Error if transport fails to disable
   */
  async disableTransport(type: TransportType): Promise<void> {
    return await OfflineProtocolNativeModule.disableTransport(type);
  }

  /**
   * Checks if Bluetooth is enabled on the device
   *
   * @returns True if Bluetooth is enabled, false otherwise
   */
  async isBluetoothEnabled(): Promise<boolean> {
    return await OfflineProtocolNativeModule.isBluetoothEnabled();
  }

  /**
   * Requests the user to enable Bluetooth
   *
   * On Android, this shows a system dialog to enable Bluetooth.
   * On iOS, this returns false as iOS doesn't allow programmatic Bluetooth enabling.
   *
   * @returns True if Bluetooth was enabled, false otherwise
   */
  async requestEnableBluetooth(): Promise<boolean> {
    return await OfflineProtocolNativeModule.requestEnableBluetooth();
  }

  /**
   * Gets the current network topology
   *
   * @returns Network topology snapshot including nodes, links, and stats
   * @throws Error if topology retrieval fails
   */
  async getTopology(): Promise<NetworkTopology> {
    const topologyJson = await OfflineProtocolNativeModule.getTopology();
    return JSON.parse(topologyJson) as NetworkTopology;
  }

  /**
   * Gets message delivery statistics
   *
   * @returns Array of message delivery statistics
   * @throws Error if stats retrieval fails
   */
  async getMessageStats(): Promise<MessageDeliveryStats[]> {
    const statsJson = await OfflineProtocolNativeModule.getMessageStats();
    return JSON.parse(statsJson) as MessageDeliveryStats[];
  }

  /**
   * Gets the delivery success rate
   *
   * @returns Success rate as a number between 0 and 1
   * @throws Error if retrieval fails
   */
  async getDeliverySuccessRate(): Promise<number> {
    return await OfflineProtocolNativeModule.getDeliverySuccessRate();
  }

  /**
   * Gets the median message delivery latency
   *
   * @returns Median latency in milliseconds, or null if no data available
   * @throws Error if retrieval fails
   */
  async getMedianLatency(): Promise<number | null> {
    return await OfflineProtocolNativeModule.getMedianLatency();
  }

  /**
   * Gets the median hop count for delivered messages
   *
   * @returns Median hop count, or null if no data available
   * @throws Error if retrieval fails
   */
  async getMedianHops(): Promise<number | null> {
    return await OfflineProtocolNativeModule.getMedianHops();
  }

  /**
   * Sends a media attachment (image, video, audio, file, etc.) to a recipient.
   *
   * The platform reads the file and passes the raw bytes as a base64 string.
   * The SDK chunks the data and sends each chunk via internet-preferred transport.
   *
   * @param params - Media sending parameters
   * @returns File ID for tracking progress
   */
  async sendMedia(params: SendMediaParams): Promise<string> {
    // Rich params — and any of the extended (cloud/sticker) metadata
    // fields, which the plain native method does not map — route to the
    // rich native method; the plain path is left untouched. Rich fields
    // only ever travel sealed inside the chunk-0 MLS ciphertext (toward
    // recipients that support it), or are dropped — never cleartext.
    const meta = params.mediaMetadata;
    const hasExtendedMetadata =
      meta !== undefined &&
      (meta.mediaId !== undefined ||
        meta.downloadUrl !== undefined ||
        meta.thumbnailUrl !== undefined ||
        meta.encryptionKey !== undefined ||
        meta.iv !== undefined ||
        meta.ciphertextHash !== undefined ||
        meta.stickerProvider !== undefined ||
        meta.stickerRemoteId !== undefined ||
        meta.stickerKind !== undefined);
    const hasRichOptions =
      params.caption !== undefined ||
      params.replyToMsg !== undefined ||
      params.replyContext !== undefined ||
      params.forwardInfo !== undefined ||
      params.fileId !== undefined ||
      hasExtendedMetadata;

    if (hasRichOptions) {
      const options = {
        media_metadata: meta
          ? {
              mime_type: meta.mimeType,
              file_name: meta.fileName,
              file_size: meta.fileSize,
              duration_ms: meta.durationMs ?? null,
              width: meta.width ?? null,
              height: meta.height ?? null,
              thumbnail_base64: meta.thumbnailBase64 ?? null,
              media_id: meta.mediaId ?? null,
              download_url: meta.downloadUrl ?? null,
              thumbnail_url: meta.thumbnailUrl ?? null,
              encryption_key: meta.encryptionKey ?? null,
              iv: meta.iv ?? null,
              ciphertext_hash: meta.ciphertextHash ?? null,
              sticker_provider: meta.stickerProvider ?? null,
              sticker_remote_id: meta.stickerRemoteId ?? null,
              sticker_kind: meta.stickerKind ?? null,
            }
          : null,
        caption: params.caption ?? null,
        reply_to_msg: params.replyToMsg ?? null,
        reply_context: params.replyContext
          ? {
              sender: params.replyContext.sender,
              text: params.replyContext.text,
              timestamp: params.replyContext.timestamp ?? null,
              reply_media_label: params.replyContext.reply_media_label ?? null,
              reply_content_type:
                params.replyContext.reply_content_type ?? null,
            }
          : null,
        forward_info: params.forwardInfo
          ? {
              original_sender: params.forwardInfo.original_sender,
              original_message_id: params.forwardInfo.original_message_id,
              original_timestamp: params.forwardInfo.original_timestamp,
              forward_count: params.forwardInfo.forward_count,
            }
          : null,
        file_id: params.fileId ?? null,
      };
      return await OfflineProtocolNativeModule.sendMediaRich(
        params.recipient,
        params.fileData,
        params.fileName,
        params.contentType,
        options,
      );
    }

    const nativeMeta = meta
      ? {
          mime_type: meta.mimeType,
          file_name: meta.fileName,
          file_size: meta.fileSize,
          duration_ms: meta.durationMs,
          width: meta.width,
          height: meta.height,
          thumbnail_base64: meta.thumbnailBase64,
        }
      : null;

    return await OfflineProtocolNativeModule.sendMedia(
      params.recipient,
      params.fileData,
      params.fileName,
      params.contentType,
      nativeMeta,
    );
  }

  /**
   * Sends a generic file to a recipient (convenience for sendMedia with ContentType.File).
   *
   * @param params - File sending parameters
   * @returns File ID for tracking progress
   */
  async sendFile(params: SendFileParams): Promise<string> {
    return this.sendMedia({
      recipient: params.recipient,
      fileData: params.fileData,
      fileName: params.fileName,
      contentType: ContentType.File,
    });
  }

  /**
   * Sends an image to a recipient.
   *
   * @param recipient - Recipient's user ID
   * @param fileData - Image data as base64
   * @param fileName - File name
   * @param metadata - Optional media metadata (dimensions, thumbnail)
   * @returns File ID for tracking progress
   */
  async sendImage(
    recipient: string,
    fileData: string,
    fileName: string,
    metadata?: MediaMetadata,
  ): Promise<string> {
    return this.sendMedia({
      recipient,
      fileData,
      fileName,
      contentType: ContentType.Image,
      mediaMetadata: metadata,
    });
  }

  /**
   * Sends a voice note to a recipient.
   *
   * @param recipient - Recipient's user ID
   * @param fileData - Audio data as base64
   * @param fileName - File name
   * @param metadata - Optional media metadata (duration)
   * @returns File ID for tracking progress
   */
  async sendVoiceNote(
    recipient: string,
    fileData: string,
    fileName: string,
    metadata?: MediaMetadata,
  ): Promise<string> {
    return this.sendMedia({
      recipient,
      fileData,
      fileName,
      contentType: ContentType.VoiceNote,
      mediaMetadata: metadata,
    });
  }

  /**
   * Sends a video note to a recipient.
   *
   * @param recipient - Recipient's user ID
   * @param fileData - Video data as base64
   * @param fileName - File name
   * @param metadata - Optional media metadata (duration, dimensions, thumbnail)
   * @returns File ID for tracking progress
   */
  async sendVideoNote(
    recipient: string,
    fileData: string,
    fileName: string,
    metadata?: MediaMetadata,
  ): Promise<string> {
    return this.sendMedia({
      recipient,
      fileData,
      fileName,
      contentType: ContentType.VideoNote,
      mediaMetadata: metadata,
    });
  }

  /**
   * Sends a video to a recipient.
   *
   * @param recipient - Recipient's user ID
   * @param fileData - Video data as base64
   * @param fileName - File name
   * @param metadata - Optional media metadata (duration, dimensions, thumbnail)
   * @returns File ID for tracking progress
   */
  async sendVideo(
    recipient: string,
    fileData: string,
    fileName: string,
    metadata?: MediaMetadata,
  ): Promise<string> {
    return this.sendMedia({
      recipient,
      fileData,
      fileName,
      contentType: ContentType.Video,
      mediaMetadata: metadata,
    });
  }

  /**
   * Gets the progress of a file transfer
   *
   * @param fileId - File identifier
   * @returns File progress information, or null if not found
   */
  async getFileProgress(fileId: string): Promise<FileProgress | null> {
    return await OfflineProtocolNativeModule.getFileProgress(fileId);
  }

  /**
   * Cancels an active file transfer
   *
   * @param fileId - File identifier
   * @returns True if cancelled, false if not found
   */
  async cancelFileTransfer(fileId: string): Promise<boolean> {
    return await OfflineProtocolNativeModule.cancelFileTransfer(fileId);
  }

  /**
   * Polls for the next received message
   *
   * @returns Message object if available, null otherwise
   * @throws Error if polling fails
   */
  async receiveMessage(): Promise<MessageReceivedEvent | null> {
    const result = await OfflineProtocolNativeModule.receiveMessage();
    if (result === null) {
      return null;
    }
    // Type assertion is safe here because we know the native module returns MessageReceivedEvent structure
    return result as MessageReceivedEvent;
  }

  /**
   * Pauses the protocol (for background mode)
   *
   * @throws Error if protocol is not running or fails to pause
   */
  async pause(): Promise<void> {
    await OfflineProtocolNativeModule.pause();
  }

  /**
   * Resumes the protocol from pause
   *
   * @throws Error if protocol is not paused or fails to resume
   */
  async resume(): Promise<void> {
    await OfflineProtocolNativeModule.resume();
  }

  /**
   * Gets the current protocol state
   *
   * @returns Protocol state (Stopped, Running, or Paused)
   * @throws Error if retrieval fails
   */
  async getState(): Promise<ProtocolState> {
    return await OfflineProtocolNativeModule.getState();
  }

  /**
   * Reports the device's battery level to the protocol engine.
   *
   * This is the feed for every battery-dependent policy in the SDK: DORS
   * energy scoring, relay promotion/demotion (`relay_promoted` /
   * `relay_demoted`), the message-forwarding battery floor, and the telemetry
   * device-capability snapshot. No transport can observe the host's battery,
   * so until this is called each of those policies runs in its unknown-level
   * branch. Call it on start and on each platform battery notification.
   *
   * Prefer {@link setBatteryState} where charging state is available: a
   * charging device is deliberately excused the soft relay battery floor, so
   * reporting the level alone strips relay duty from plugged-in devices that
   * should keep it.
   *
   * @param level - Battery level (0-100)
   */
  async setBatteryLevel(level: number): Promise<void> {
    return await OfflineProtocolNativeModule.setBatteryLevel(level);
  }

  /**
   * Reports the device's battery level and charging state to the protocol
   * engine. See {@link setBatteryLevel} for what depends on it.
   *
   * @param level - Battery level (0-100)
   * @param isCharging - Whether the device is currently charging
   */
  async setBatteryState(level: number, isCharging: boolean): Promise<void> {
    return await OfflineProtocolNativeModule.setBatteryState(level, isCharging);
  }

  /**
   * Gets the last reported battery level
   *
   * @returns Battery level (0-100) or null if the host has not reported one
   */
  async getBatteryLevel(): Promise<number | null> {
    return await OfflineProtocolNativeModule.getBatteryLevel();
  }

  /**
   * Gets the last reported charging state (false if none reported).
   */
  async getIsCharging(): Promise<boolean> {
    return await OfflineProtocolNativeModule.getIsCharging();
  }

  /**
   * Sets the relay priority.
   *
   * A shorthand for updating only `relayPriority`; see
   * {@link updateRelayConfig} for the rest. The legacy `'low' | 'medium' |
   * 'high'` spelling is still accepted and maps to `never` / `auto` /
   * `always`.
   *
   * @param priority - Relay priority
   * @throws Error if setting fails
   */
  async setRelayPriority(
    priority: RelayPriority | "low" | "medium" | "high"
  ): Promise<void> {
    const normalized = this.normalizeRelayPriority(priority);
    if (!normalized) {
      throw new Error(`Invalid relay priority: ${priority}`);
    }
    return await OfflineProtocolNativeModule.setRelayPriority(normalized);
  }

  /**
   * Gets the current relay priority
   */
  async getRelayPriority(): Promise<RelayPriority> {
    return await OfflineProtocolNativeModule.getRelayPriority();
  }

  /**
   * Updates the relay configuration at runtime.
   *
   * Governs whether this device carries other people's traffic and under what
   * conditions it takes the relay role. Applies to the next role evaluation
   * and the next forwarding decision — no restart needed. Omitted fields keep
   * their current values.
   *
   * The battery-dependent parts need a battery feed to do anything — see
   * {@link setBatteryState}.
   */
  async updateRelayConfig(config: RelayConfig): Promise<void> {
    // Rejected rather than dropped, matching `setRelayPriority`. Silently
    // discarding it would apply the rest of the update and leave the priority
    // at its old value, which reads from the call site as though it had been
    // set.
    if (
      config.relayPriority !== undefined &&
      !this.normalizeRelayPriority(config.relayPriority)
    ) {
      throw new Error(`Invalid relay priority: ${config.relayPriority}`);
    }
    const normalizedPriority = this.normalizeRelayPriority(
      config.relayPriority
    );
    const payload = {
      ...(config.allowRelay !== undefined
        ? { allowRelay: config.allowRelay }
        : {}),
      ...(config.minBatteryForRelay !== undefined
        ? { minBatteryForRelay: config.minBatteryForRelay }
        : {}),
      ...(normalizedPriority ? { relayPriority: normalizedPriority } : {}),
    };
    return await OfflineProtocolNativeModule.updateRelayConfig(
      JSON.stringify(payload)
    );
  }

  /**
   * Gets the current relay configuration.
   */
  async getRelayConfig(): Promise<Required<RelayConfig>> {
    const json = await OfflineProtocolNativeModule.getRelayConfig();
    return typeof json === "string" ? JSON.parse(json) : json;
  }

  /**
   * Checks if this device is currently acting as a relay
   *
   * @returns True if device is a relay
   */
  async isRelay(): Promise<boolean> {
    return await OfflineProtocolNativeModule.isRelay();
  }

  /**
   * Gets the current number of discovered BLE peers
   *
   * @returns Number of BLE peers currently tracked
   */
  async getBLePeerCount(): Promise<number> {
    return await OfflineProtocolNativeModule.bleGetPeerCount();
  }

  /**
   * Gets the BLE diagnostic counters.
   *
   * These are the rollout alarm for the self-certifying-address migration.
   * Each counter records a frame that took a *degraded* path rather than
   * failing outright, so none of them show up as a delivery error — a fleet
   * can be quietly falling back on every send while its success metrics look
   * healthy. Read them together and watch the trend, not the absolute value:
   * small counts are normal, sustained growth after a release means peers
   * disagree about identity or MTU.
   *
   * - `fragmentFallbacks` — a frame for a directly-connected peer had to be
   *   broadcast to every peer instead of addressed to one. Rising means
   *   recipients are not being recognised as the connected peer they are.
   * - `recipientNotAmongPeers` — a send named a peer that is connected over
   *   BLE but not under that id. This is the sharpest identity-mismatch
   *   signal: it is what a peer announcing one id while framing another
   *   looks like from the sender's side.
   * - `undersizedMtuReports` — a peer reported an MTU too small to carry a
   *   fragment header, so a conservative default was used.
   *
   * All three read zero when BLE is not enabled or not yet started.
   *
   * @returns The three counters as of now
   */
  async getBleDiagnostics(): Promise<BleDiagnostics> {
    return await OfflineProtocolNativeModule.bleGetDiagnostics();
  }

  /**
   * Gets detailed metrics for a specific transport
   *
   * @param transportType - Transport type
   * @returns Transport metrics or null if not available
   */
  async getTransportMetrics(transportType: TransportType): Promise<TransportMetrics | null> {
    return await OfflineProtocolNativeModule.getTransportMetrics(transportType);
  }

  /**
   * Installs a unified telemetry sink. Replaces any previously installed
   * sink. Config fields left undefined fall back to the privacy-preserving
   * defaults on the Rust side (scrubIds=true, mlsVerbosity='lifecycle',
   * metricsCadenceMs=5000, routingDiagnostic=false, enablePollQueue=true).
   *
   * Telemetry records are dispatched via `onTelemetry` (push) and buffered
   * for `pollTelemetry` (pull). The legacy `on(...)` event path is unaffected.
   *
   * **Listener race**: the bridge call is async, so registering an
   * `onTelemetry(listener)` *after* `installTelemetrySink(...)` resolves
   * leaves a window where records emitted in the gap are fanned out to an
   * empty listener set and dropped on the push channel (they still reach
   * the pull queue when `enablePollQueue` is true). To close the race,
   * pass the listener directly to this call — it is registered
   * synchronously *before* the underlying native install is dispatched,
   * so no emission can slip through. The returned unsubscribe removes the
   * listener; further listeners can still be added via `onTelemetry(...)`.
   *
   * **Poll queue opt-out**: push-only integrations should pass
   * `{ enablePollQueue: false }` to skip the per-emit JSON envelope build
   * inside the Rust adapter. `pollTelemetry()` will return null for any
   * record emitted while the opt-out is in effect.
   *
   * **Queue retention across replacement**: calling this method a second
   * time replaces the sink but does NOT drain the pull queue. A consumer
   * polling immediately after replace will see the previous sink's
   * buffered records first (FIFO). Drain `pollTelemetry()` in a loop
   * until it returns null before re-installing if you need a clean slate,
   * or call `uninstallTelemetrySink()` which atomically detaches the
   * sink and drains the queue in one shot.
   *
   * @returns An unsubscribe function for the optional `listener`, or a
   * no-op when no listener was provided.
   */
  async installTelemetrySink(
    config: TelemetryConfig = {},
    listener?: TelemetryListener
  ): Promise<() => void> {
    // Register the listener synchronously BEFORE awaiting the bridge so
    // records emitted between native-side install completion and the next
    // JS microtask cannot slip past an empty listener set.
    let unsubscribe: () => void = () => {};
    if (listener) {
      unsubscribe = this.onTelemetry(listener);
    }
    try {
      await OfflineProtocolNativeModule.installTelemetrySink(config);
    } catch (err) {
      // If the install failed, drop the pre-registered listener so a
      // retrying caller doesn't accumulate dangling listeners.
      unsubscribe();
      throw err;
    }
    return unsubscribe;
  }

  /**
   * Detaches the installed telemetry sink. After this resolves, no further
   * telemetry records reach `onTelemetry` listeners or the pull queue —
   * the Rust adapter replaces the core sink with a no-op and drains the
   * pull queue in a single call, so a subsequent
   * `installTelemetrySink(...)` starts with an empty queue.
   *
   * Idempotent — calling without a prior install is a no-op.
   *
   * Does NOT remove TS-side listeners registered via `onTelemetry(...)`
   * or via the optional-listener form of `installTelemetrySink(...)`;
   * they remain bound but will simply never fire again unless a new sink
   * is installed. Drop them explicitly via their unsubscribe if that is
   * the intent.
   */
  async uninstallTelemetrySink(): Promise<void> {
    await OfflineProtocolNativeModule.uninstallTelemetrySink();
  }

  /**
   * Returns a stable, opaque per-install telemetry identifier (32 hex
   * characters), derived from the SDK-managed persistent scrub secret. The
   * secret itself never crosses the bridge and cannot be recovered from
   * the id, so the id is safe to attach to telemetry as a device-grain
   * key (e.g. distinct-device counting in analytics backends).
   *
   * Resolves `null` until the persistent secret is available — i.e.
   * before secure storage is wired on the native side (MLS initialization
   * or message persistence), or when persisting the secret failed this
   * session. In that state the id would not be stable across launches,
   * so none is exposed.
   *
   * Stable across app restarts and `installTelemetrySink(...)` calls;
   * unaffected by an app-supplied `scrubIds` / scrub-secret config.
   *
   * Note: while the id reveals nothing about the user or device, it is
   * still a persistent per-install identifier — using it may need to be
   * declared under your app's privacy disclosures (e.g. Apple privacy
   * manifest / Google Play data safety, "device or other IDs").
   */
  async telemetryInstallId(): Promise<string | null> {
    return await OfflineProtocolNativeModule.telemetryInstallId();
  }

  /**
   * Registers a listener that receives every TelemetryRecord emitted by the
   * SDK. Requires a prior `installTelemetrySink(...)` — without an installed
   * sink the Rust side emits nothing on either the push channel or the poll
   * buffer.
   *
   * To close the install→register race window, prefer passing the listener
   * directly to `installTelemetrySink(config, listener)`; that form
   * registers synchronously before the native install is dispatched.
   *
   * @returns An unsubscribe function.
   */
  onTelemetry(listener: TelemetryListener): () => void {
    this.telemetryListeners.add(listener);
    return () => {
      this.telemetryListeners.delete(listener);
    };
  }

  /**
   * Polls the next buffered telemetry record. Returns `null` when the
   * internal queue is empty. The queue is bounded (1024 slots); overflow
   * drops the oldest entry.
   *
   * Useful when an app prefers polling over push delivery. Requires a
   * prior `installTelemetrySink(...)` with `enablePollQueue` left at its
   * default (`true` / omitted). With `enablePollQueue: false` the Rust
   * adapter never enqueues, so this method always returns null for
   * records emitted under that config.
   *
   * The pull queue survives sink replacement — records enqueued by a
   * previous sink stay readable until drained. See
   * `installTelemetrySink` for details.
   *
   * Throws if the native layer returns a malformed envelope — callers can
   * then distinguish "queue empty" (`null`) from "bridge corruption"
   * (thrown) and surface the latter in their own telemetry.
   */
  async pollTelemetry(): Promise<TelemetryRecord | null> {
    const json: string | null = await OfflineProtocolNativeModule.pollTelemetryFrame();
    // Null/undefined is "queue empty". Anything else (including the empty
    // string) would indicate a bridge bug — fall through to JSON.parse,
    // which will then throw and let the caller distinguish corruption from
    // "no data".
    if (json === null || json === undefined) {
      return null;
    }
    try {
      return JSON.parse(json) as TelemetryRecord;
    } catch (error) {
      throw new Error(
        `pollTelemetry: malformed envelope from native bridge (${(error as Error).message})`
      );
    }
  }

  /**
   * Forces the protocol to use a specific transport (overrides DORS)
   *
   * @param transportType - Transport type to force
   * @throws Error if forcing fails
   */
  async forceTransport(transportType: TransportType): Promise<void> {
    return await OfflineProtocolNativeModule.forceTransport(transportType);
  }

  /**
   * Releases the transport lock and lets DORS make decisions again
   */
  async releaseTransportLock(): Promise<void> {
    return await OfflineProtocolNativeModule.releaseTransportLock();
  }

  /**
   * Updates DORS configuration at runtime.
   *
   * Omitted fields keep their current values — the same partial-update
   * contract as {@link updateRelayConfig}. Every field must therefore be
   * expressible here: a field this signature omits cannot be set, and (before
   * the bridges merged from the live config) was silently reset by any update
   * that changed something else.
   *
   * @param config - DORS configuration
   * @throws Error if update fails
   */
  async updateDorsConfig(config: {
    preferOnline?: boolean;
    switchHysteresis?: number;
    switchCooldownSecs?: number;
    bleToWifiRetryThreshold?: number;
    minSuccessRateBeforeEscalation?: number;
    minBleSamplesBeforeSuccessRateEscalation?: number;
    rssiSwitchThreshold?: number;
    congestionQueueThreshold?: number;
    stabilityWindowSecs?: number;
    poorSignalDurationSecs?: number;
    ttlEscalationThreshold?: number;
    congestionDurationSecs?: number;
    ttlEscalationHoldSecs?: number;
    historyWindowSize?: number;
    queueRecoveryRatio?: number;
    lowBatteryThreshold?: number;
    relayMinBatteryLevel?: number;
    relayOptimalConnectionCount?: number;
  }): Promise<void> {
    const payload = { ...config };
    if (payload.switchHysteresis !== undefined) {
      payload.switchHysteresis = Math.max(0, payload.switchHysteresis);
    }
    if (payload.switchCooldownSecs !== undefined) {
      payload.switchCooldownSecs = Math.max(0, payload.switchCooldownSecs);
    }
    if (payload.congestionDurationSecs !== undefined) {
      payload.congestionDurationSecs = Math.max(
        0,
        payload.congestionDurationSecs
      );
    }
    if (payload.ttlEscalationHoldSecs !== undefined) {
      payload.ttlEscalationHoldSecs = Math.max(
        1,
        payload.ttlEscalationHoldSecs
      );
    }
    if (payload.historyWindowSize !== undefined) {
      payload.historyWindowSize = Math.max(
        1,
        Math.min(100, Math.round(payload.historyWindowSize))
      );
    }
    if (payload.queueRecoveryRatio !== undefined) {
      payload.queueRecoveryRatio = Math.min(
        1,
        Math.max(0, payload.queueRecoveryRatio)
      );
    }
    if (payload.lowBatteryThreshold !== undefined) {
      payload.lowBatteryThreshold = Math.min(
        100,
        Math.max(0, Math.round(payload.lowBatteryThreshold))
      );
    }
    if (payload.relayMinBatteryLevel !== undefined) {
      payload.relayMinBatteryLevel = Math.min(
        100,
        Math.max(0, Math.round(payload.relayMinBatteryLevel))
      );
    }
    if (payload.relayOptimalConnectionCount !== undefined) {
      payload.relayOptimalConnectionCount = Math.min(
        255,
        Math.max(0, Math.round(payload.relayOptimalConnectionCount))
      );
    }
    return await OfflineProtocolNativeModule.updateDorsConfig(
      JSON.stringify(payload)
    );
  }

  /**
   * Gets the current DORS configuration
   *
   * @returns DORS configuration
   */
  async getDorsConfig(): Promise<{
    preferOnline: boolean;
    switchHysteresis: number;
    switchCooldownSecs: number;
    bleToWifiRetryThreshold: number;
    minSuccessRateBeforeEscalation: number;
    minBleSamplesBeforeSuccessRateEscalation: number;
    rssiSwitchThreshold: number;
    congestionQueueThreshold: number;
    stabilityWindowSecs: number;
    poorSignalDurationSecs: number;
    ttlEscalationThreshold: number;
    congestionDurationSecs: number;
    ttlEscalationHoldSecs: number;
    historyWindowSize: number;
    queueRecoveryRatio: number;
    lowBatteryThreshold: number;
    relayMinBatteryLevel: number;
    relayOptimalConnectionCount: number;
  }> {
    return await OfflineProtocolNativeModule.getDorsConfig();
  }

  /**
   * Updates ACK configuration at runtime
   *
   * @param config - ACK configuration
   * @throws Error if update fails
   */
  async updateAckConfig(config: AckConfig): Promise<void> {
    return await OfflineProtocolNativeModule.updateAckConfig(
      JSON.stringify(config)
    );
  }

  /**
   * Updates retry configuration at runtime
   *
   * @param config - Retry configuration
   * @throws Error if update fails
   */
  async updateRetryConfig(config: RetryConfig): Promise<void> {
    return await OfflineProtocolNativeModule.updateRetryConfig(
      JSON.stringify(config)
    );
  }

  /**
   * Updates deduplication configuration at runtime
   *
   * @param config - Deduplication configuration
   * @throws Error if update fails
   */
  async updateDedupConfig(config: DedupConfig): Promise<void> {
    return await OfflineProtocolNativeModule.updateDedupConfig(
      JSON.stringify(config)
    );
  }

  /**
   * Gets deduplicator statistics for monitoring
   *
   * @returns Deduplication statistics
   */
  async getDedupStats(): Promise<DedupStats> {
    return await OfflineProtocolNativeModule.getDedupStats();
  }

  /**
   * Reports how much traffic this device is carrying for other people.
   *
   * Counters are cumulative for the lifetime of this instance and never reset,
   * so a rate is a difference between two reads. The exception is
   * `awaitingTransmission`, a gauge that goes down as well as up. `forwarded`
   * is the contribution figure to show a user; `transmissions` is the one the
   * per-second budget bounds, since it counts each link separately and
   * includes this device's own sends.
   *
   * Two readings worth knowing: `rateDeferred` rising means forwarding is
   * hitting its ceiling and those frames are delayed rather than dropped, and
   * `coveredByANeighbor` is the mesh working as intended — a neighbor was
   * heard carrying the frame first, so this device stood down.
   *
   * For whether back-pressure is actually costing anything, read
   * `refusedQueueFull` and `abandonedOverdue`. Those are the two that count
   * frames genuinely lost, and a device shedding traffic can otherwise show
   * nothing but healthy-looking deferrals.
   *
   * Note that a device with a working relay connection forwards nothing: the
   * mesh is only offered frames no other carrier can deliver. Zero counters on
   * an online device are the honest answer, not a fault.
   *
   * @returns Cumulative mesh forwarding counters
   */
  async getMeshRelayStats(): Promise<MeshRelayStats> {
    return await OfflineProtocolNativeModule.getMeshRelayStats();
  }

  /**
   * Reports the mesh forwarding tunables actually in force.
   *
   * Read from the governor in the Rust core, so this is what forwarding
   * decisions really use rather than an echo of what was passed to
   * `create()` — including every default this app never set.
   *
   * Every field is present. Nothing here needs a `??` fallback, and writing
   * one would be inventing a second copy of a default that can drift.
   *
   * @returns The mesh forwarding tunables in force
   */
  async getMeshRelayTunables(): Promise<MeshRelayTunables> {
    return await OfflineProtocolNativeModule.getMeshRelayTunables();
  }

  /**
   * Gets the number of pending ACKs waiting for confirmation
   *
   * @returns Number of pending ACKs
   */
  async getPendingAckCount(): Promise<number> {
    return await OfflineProtocolNativeModule.getPendingAckCount();
  }

  /**
   * Gets the current retry queue size
   *
   * @returns Number of messages in retry queue
   */
  async getRetryQueueSize(): Promise<number> {
    return await OfflineProtocolNativeModule.getRetryQueueSize();
  }

  // ============================================================================
  // DORS DECISION SUPPORT
  // ============================================================================

  /**
   * Checks if DORS recommends escalating to WiFi.
   * Use this to query whether the protocol should switch from BLE to WiFi Direct.
   *
   * @returns True if escalation to WiFi is recommended
   */
  async shouldEscalateToWifi(): Promise<boolean> {
    return await OfflineProtocolNativeModule.shouldEscalateToWifi();
  }

  // ============================================================================
  // FILE TRANSFER OPERATIONS
  // ============================================================================

  /**
   * Processes a file chunk.
   * Use this for custom file transfer handling.
   *
   * @param fileId - File identifier
   * @param chunkIndex - Zero-based chunk index
   * @param totalChunks - Total number of chunks in the file
   * @param fileSize - Total file size in bytes
   * @param fileName - File name
   * @param fileChecksum - File checksum
   * @param data - Chunk data as array of bytes
   */
  async processFileChunk(
    fileId: string,
    chunkIndex: number,
    totalChunks: number,
    fileSize: number,
    fileName: string,
    fileChecksum: string,
    data: number[],
  ): Promise<void> {
    return await OfflineProtocolNativeModule.processFileChunk(
      fileId,
      chunkIndex,
      totalChunks,
      fileSize,
      fileName,
      fileChecksum,
      data,
    );
  }

  /**
   * Finalizes a file transfer.
   * Call this after all chunks have been processed.
   *
   * @param fileId - File identifier
   */
  async finalizeFile(fileId: string): Promise<void> {
    return await OfflineProtocolNativeModule.finalizeFile(fileId);
  }

  // ============================================================================
  // WIFI DIRECT TRANSPORT METHODS (Low-Level)
  // ============================================================================

  /**
   * Notifies the protocol of WiFi Direct connection state change.
   *
   * @param isConnected - Whether WiFi Direct is connected
   */
  async wifiDirectStatusChanged(isConnected: boolean): Promise<void> {
    return await OfflineProtocolNativeModule.wifiDirectStatusChanged(
      isConnected
    );
  }

  /**
   * Handles an incoming WiFi Direct message.
   *
   * @deprecated The bundled Wi-Fi Direct managers no longer call this, and no
   * application should. `senderId` is treated by the core as the peer's
   * *proven* user-level id: it becomes the frame's transport peer identity and
   * is matched against `Message.sender`, so a value the peer did not prove is
   * either rejected or — worse — accepted into routing state under a name
   * anyone could claim. Wi-Fi Direct has no handshake that yields such a
   * value; the only identity on that wire is the `Message.sender` inside the
   * frame, which is the very thing this parameter exists to cross-check.
   * `WifiDirectTransport` is also not registered, so frames passed here are
   * dropped. Restoring the transport requires a signed identity preamble
   * cross-checked the way BLE's IDENTITY characteristic is.
   *
   * @param senderId - Sender peer ID. Must be a verified `off1…` address.
   * @param data - Message data as array of bytes
   */
  async wifiDirectMessageReceived(
    senderId: string,
    data: number[]
  ): Promise<void> {
    return await OfflineProtocolNativeModule.wifiDirectMessageReceived(
      senderId,
      data
    );
  }

  /**
   * Gets the next outgoing WiFi Direct message.
   *
   * @returns Message to send or null if queue is empty
   */
  async wifiDirectGetNextMessage(): Promise<{
    recipientId: string;
    data: number[];
  } | null> {
    return await OfflineProtocolNativeModule.wifiDirectGetNextMessage();
  }

  /**
   * Notifies the protocol that a WiFi Direct peer has connected.
   *
   * @deprecated See {@link OfflineProtocol.wifiDirectMessageReceived}. An
   * unproven `peerId` here is entered into the core's capacity-bounded
   * `known_peers` — evicting genuine neighbours — and starts an automatic key
   * exchange toward a peer that cannot answer it. The bundled managers no
   * longer call this.
   *
   * @param peerId - Peer ID. Must be a verified `off1…` address.
   */
  async wifiDirectPeerConnected(peerId: string): Promise<void> {
    return await OfflineProtocolNativeModule.wifiDirectPeerConnected(peerId);
  }

  /**
   * Notifies the protocol that a WiFi Direct peer has disconnected.
   *
   * @deprecated See {@link OfflineProtocol.wifiDirectMessageReceived}. The
   * bundled managers no longer call this.
   *
   * @param peerId - Peer ID. Must be a verified `off1…` address.
   */
  async wifiDirectPeerDisconnected(peerId: string): Promise<void> {
    return await OfflineProtocolNativeModule.wifiDirectPeerDisconnected(peerId);
  }

  // ============================================================================
  // INTERNET TRANSPORT METHODS (Low-Level)
  // ============================================================================

  /**
   * Notifies the protocol of internet connection state change.
   *
   * @param isConnected - Whether internet transport is connected
   */
  async internetStatusChanged(isConnected: boolean): Promise<void> {
    return await OfflineProtocolNativeModule.internetStatusChanged(isConnected);
  }

  /**
   * Handles an incoming internet message.
   *
   * @param senderId - Sender ID
   * @param data - Message data as array of bytes
   */
  async internetMessageReceived(
    senderId: string,
    data: number[]
  ): Promise<void> {
    return await OfflineProtocolNativeModule.internetMessageReceived(
      senderId,
      data
    );
  }

  /**
   * Gets the next outgoing internet message.
   *
   * After sending over the wire, you **must** call either
   * `internetConfirmSent(messageId)` or `internetSendFailed(messageId)`.
   *
   * @returns Message to send (with messageId) or null if queue is empty
   */
  async internetGetNextMessage(): Promise<{
    messageId: string;
    recipientId: string;
    data: number[];
  } | null> {
    return await OfflineProtocolNativeModule.internetGetNextMessage();
  }

  /**
   * Confirms that a message was successfully sent over the wire (e.g., WebSocket).
   *
   * Call this after the WebSocket `send()` completes successfully.
   * This feeds real delivery data into transport metrics for DORS routing.
   *
   * @param messageId - The messageId from `internetGetNextMessage()`
   */
  async internetConfirmSent(messageId: string): Promise<void> {
    return await OfflineProtocolNativeModule.internetConfirmSent(messageId);
  }

  /**
   * Reports that a message failed to send over the wire.
   *
   * Call this when the WebSocket `send()` fails or the connection drops.
   *
   * @param messageId - The messageId from `internetGetNextMessage()`
   */
  async internetSendFailed(messageId: string): Promise<void> {
    return await OfflineProtocolNativeModule.internetSendFailed(messageId);
  }

  // ============================================================================
  // MLS (END-TO-END ENCRYPTION) METHODS
  // ============================================================================

  /**
   * Initializes MLS with built-in secure storage.
   * Uses iOS Keychain or Android EncryptedSharedPreferences.
   *
   * **Note:** This is called automatically by `start()` when `encryption.enabled` is true (default).
   * You only need to call this manually if you disabled encryption initially and want to enable it later.
   *
   * @throws Error if initialization fails
   */
  async initializeMlsWithSecureStorage(): Promise<void> {
    return await OfflineProtocolNativeModule.initializeMlsWithSecureStorage();
  }

  /**
   * Checks if MLS is initialized.
   *
   * @returns True if MLS is ready for use
   */
  async isMlsInitialized(): Promise<boolean> {
    return await OfflineProtocolNativeModule.isMlsInitialized();
  }

  // ========================================================================
  // IDENTITY AND SIGNING OPERATIONS
  // ========================================================================

  /**
   * Gets the identity public key (Ed25519, 32 bytes).
   * This is the public key derived from the MLS credential and can be shared
   * with others for identity verification and secure communication.
   *
   * @returns The public key as an array of bytes
   * @throws Error if MLS is not initialized
   */
  async getIdentityPublicKey(): Promise<number[]> {
    return await OfflineProtocolNativeModule.getIdentityPublicKey();
  }

  /**
   * Derives the canonical self-certifying address of an Ed25519 identity key:
   * `off1…` (44 characters), the bech32m encoding of
   * `0x01 || SHA-256(publicKey)[0:20]`.
   *
   * The address is a hash of the key, so it authenticates itself: a peer
   * claiming an address is checked by re-deriving it from the key they
   * present. The same key always yields the same address, and every address
   * has exactly one valid string form.
   *
   * Needs no protocol instance — safe to call before `create()`, e.g. to
   * verify an invite or QR code.
   *
   * @param publicKey - The Ed25519 public key bytes (exactly 32)
   * @returns The derived `off1…` address
   * @throws If `publicKey` is not 32 bytes
   */
  async deriveAddress(publicKey: number[]): Promise<string> {
    return await OfflineProtocolNativeModule.deriveAddress(publicKey);
  }

  /**
   * Decodes and verifies an invite blob.
   *
   * Needs no protocol instance — safe to call before `create()`, which is the
   * whole point: a scanner verifies a QR code before deciding to act on it.
   *
   * Verification is mandatory and total. The address must be the one its
   * public key derives to, and any signature present must verify under that
   * key, so a resolved `InviteInfo` is always self-certified.
   *
   * **What it does not prove:** that the invite came from who you think. An
   * attacker's own correctly-signed invite is indistinguishable from a
   * legitimate stranger's — only the out-of-band context (this QR was on
   * *this* person's screen) carries that.
   *
   * @param blob - The base64url payload, e.g. the `c` query parameter
   * @returns The verified invite
   * @throws If the blob is malformed, the address is not the key's, or a
   *   signature does not verify. Every case means refuse, not warn.
   */
  async parseInvite(blob: string): Promise<InviteInfo> {
    return await OfflineProtocolNativeModule.parseInvite(blob);
  }

  /**
   * Builds an invite blob for this identity.
   *
   * The result is opaque base64url. Apps own the container; the recommended
   * form is one parameter, `<app-scheme>://connect?c=<blob>`, so it composes
   * with an existing scheme and route.
   *
   * Sign it when the invite may travel **without its issuer** — a link
   * forwarded through a third party — because the signature binds the petname
   * to the key, so a forwarded invite cannot save Alice's key under the name
   * "Bob". Leave it unsigned for a QR shown phone to phone: the physical
   * channel already authenticates it, and an app that lets the user confirm
   * the name has made the user the authority over it. Signing costs about 90
   * characters.
   *
   * Carries no key package by design (an MLS init key is single-use and a QR
   * code is static, so pairing them guarantees a collision as soon as two
   * people scan the same code) and no expiry (a printed QR that stops working
   * is a bug).
   *
   * @param petname - Suggested display name, ≤ 64 bytes
   * @param signed - Whether to bind the petname to the key
   */
  async createInvite(petname?: string, signed = false): Promise<string> {
    return await OfflineProtocolNativeModule.createInvite(
      petname ?? null,
      signed
    );
  }

  /**
   * Resolves a username to the set of devices claiming it.
   *
   * Requires `transports.nostr.usernameDiscoveryEnabled`. Resolves `true` if
   * this call started the lookup and `false` if it joined one already in
   * flight. **Both mean an answer is coming**: exactly one
   * `username_resolved` event follows either way, so awaiting that event after
   * either result is safe.
   *
   * Every case where no event will ever arrive **rejects** instead, so a
   * `false` can never leave a spinner running forever. The rejection code says
   * which, and they call for different handling:
   *
   * - `InvalidConfiguration` — discovery is off (it also requires
   *   `coldContactEnabled`). Retrying unchanged can never succeed.
   * - `InvalidState` — too many lookups in flight. Transient; retry shortly.
   * - `NotStarted` — the protocol is not running, so nothing would pump relay
   *   traffic or sweep the deadline for this lookup. Transient; retry after
   *   `start()`.
   * - `InvalidArgument` — not a claimable username (empty, over 64 bytes once
   *   normalized, carrying a control or format character, or address-shaped).
   *
   * Subscribe to `username_resolved` **before** calling this. The answer is a
   * single event with no replay, so a listener attached afterwards can miss it.
   *
   * The answer carries **every** verified claim. There is deliberately no
   * "best" claim and no ordering: anyone may publish any name, so what comes
   * back is a set of assertions for a human to arbitrate, not a lookup result.
   *
   * **Do not auto-select.** Taking the first entry turns a non-authoritative
   * directory into an authoritative-looking one — the user then believes the
   * *name* was verified when only a key ever was. Present the claims, have the
   * user confirm out of band, and store the address, never the name: a name
   * can be re-claimed by anyone tomorrow, an address is self-certifying.
   *
   * @param username - The name to look up; normalized to NFC and lowercase
   * @returns `true` if this call started the lookup, `false` if it joined one
   * @throws `InvalidConfiguration` if discovery is off, `InvalidState` if too
   *   many lookups are in flight, `NotStarted` if the protocol is not running,
   *   `InvalidArgument` if the name is not claimable
   */
  async resolveUsername(username: string): Promise<boolean> {
    return await OfflineProtocolNativeModule.resolveUsername(username);
  }

  /**
   * This device's own address (`off1…`), or `null` before startup completes.
   *
   * Derived from the identity key held in this profile's storage — the app
   * does not choose it, and it is stable across restarts of the same
   * `profile`. This is the string to show the user, put in an invite or QR
   * code, and what peers pass as the recipient to reach this device.
   *
   * `null` until MLS is initialized (which `start()` does), because the key
   * that defines it lives in storage that is not open before then. The
   * `identity_ready` event carries the same value at the moment it is known.
   */
  async localAddress(): Promise<string | null> {
    if (this.cachedLocalAddress !== null) {
      return this.cachedLocalAddress;
    }
    const address =
      (await OfflineProtocolNativeModule.localAddress()) ?? null;
    this.cachedLocalAddress = address;
    return address;
  }

  /**
   * Derives a deterministic user ID from a public key.
   *
   * @deprecated Use {@link deriveAddress}. This returns the same `off1…`
   * address, but requires an initialized protocol instance and accepts any
   * input length rather than rejecting keys that are not 32 bytes.
   *
   * @param publicKey - The public key bytes (32 bytes for Ed25519)
   * @returns The derived address string
   */
  async deriveUserIdFromPublicKey(publicKey: number[]): Promise<string> {
    return await OfflineProtocolNativeModule.deriveUserIdFromPublicKey(
      publicKey
    );
  }

  /**
   * Signs arbitrary data with the identity private key (Ed25519).
   * Use this to prove ownership of your identity or to sign messages.
   *
   * @param data - The data to sign
   * @returns The signature as an array of bytes (64 bytes)
   * @throws Error if MLS is not initialized
   */
  async signData(data: number[]): Promise<number[]> {
    return await OfflineProtocolNativeModule.signData(data);
  }

  /**
   * Verifies a signature against a public key.
   * Use this to verify that data was signed by the owner of a public key.
   *
   * @param publicKey - The signer's public key (32 bytes)
   * @param data - The data that was signed
   * @param signature - The signature to verify (64 bytes)
   * @returns True if the signature is valid
   * @throws Error if verification fails due to invalid input
   */
  async verifySignature(
    publicKey: number[],
    data: number[],
    signature: number[]
  ): Promise<boolean> {
    return await OfflineProtocolNativeModule.verifySignature(
      publicKey,
      data,
      signature
    );
  }

  /**
   * Generates a new MLS key package.
   * Key packages are used by others to establish encrypted sessions with you.
   *
   * @returns Generated key package
   * @throws Error if generation fails
   */
  async mlsGenerateKeyPackage(): Promise<MlsKeyPackage> {
    const result = await OfflineProtocolNativeModule.mlsGenerateKeyPackage();
    return {
      packageId: result.packageId,
      userId: result.userId,
      keyPackageData: result.keyPackageData,
      createdAt: result.createdAt,
      isSynced: result.isSynced,
    };
  }

  /**
   * Gets an existing key package or creates a new one.
   *
   * @returns Key package
   * @throws Error if operation fails
   */
  async mlsGetOrCreateKeyPackage(): Promise<MlsKeyPackage> {
    const result = await OfflineProtocolNativeModule.mlsGetOrCreateKeyPackage();
    return {
      packageId: result.packageId,
      userId: result.userId,
      keyPackageData: result.keyPackageData,
      createdAt: result.createdAt,
      isSynced: result.isSynced,
    };
  }

  /**
   * Gets pending key packages that haven't been synced yet.
   *
   * @returns Array of pending key packages
   */
  async mlsGetPendingKeyPackages(): Promise<MlsKeyPackage[]> {
    const results =
      await OfflineProtocolNativeModule.mlsGetPendingKeyPackages();
    return results.map((r: any) => ({
      packageId: r.packageId,
      userId: r.userId,
      keyPackageData: r.keyPackageData,
      createdAt: r.createdAt,
      isSynced: r.isSynced,
    }));
  }

  /**
   * Marks a key package as synced.
   *
   * @param packageId - Key package ID to mark
   * @throws Error if operation fails
   */
  async mlsMarkKeyPackageSynced(packageId: string): Promise<void> {
    return await OfflineProtocolNativeModule.mlsMarkKeyPackageSynced(packageId);
  }

  /**
   * Imports another user's key package.
   * Required before you can send encrypted messages to them.
   *
   * @param userId - User ID that owns the key package
   * @param keyPackageData - Raw key package data
   * @throws Error if import fails
   */
  async mlsImportKeyPackage(
    userId: string,
    keyPackageData: number[]
  ): Promise<void> {
    return await OfflineProtocolNativeModule.mlsImportKeyPackage(
      userId,
      keyPackageData
    );
  }

  /**
   * Checks if an MLS session exists with another user.
   *
   * @param otherUserId - Other user's ID
   * @returns True if session exists
   */
  async mlsHasSession(otherUserId: string): Promise<boolean> {
    return await OfflineProtocolNativeModule.mlsHasSession(otherUserId);
  }

  /**
   * Checks if a pending key package is available for a peer.
   *
   * Key packages are received automatically when peers are discovered
   * (if auto_key_exchange is enabled). This method checks if we have
   * received the peer's key package and can establish a session.
   *
   * @param peerId - Peer's ID
   * @returns True if key package is available
   */
  async hasPendingKeyPackage(peerId: string): Promise<boolean> {
    return await OfflineProtocolNativeModule.hasPendingKeyPackage(peerId);
  }

  /**
   * Returns the current secure-session establishment state for a peer.
   *
   * Useful for retry/UI flows when operations fail with `SessionNotReady`.
   *
   * @param peerId - Peer's ID
   */
  async getEstablishmentState(peerId: string): Promise<EstablishmentState> {
    return await OfflineProtocolNativeModule.getEstablishmentState(peerId);
  }

  /**
   * Establishes a secure MLS session with a peer (high-level API).
   *
   * This method handles the complete session establishment flow:
   * - If session already exists, returns null
   * - If a pending key package is available, imports it, creates session, sends Welcome
   * - If no key package is available, throws an error
   *
   * This is the recommended method for establishing secure sessions as it
   * handles the key package exchange flow automatically.
   *
   * @param peerId - Peer's ID
   * @returns Welcome message if session was created, null if session already exists
   * @throws Error if no key package is available (peer hasn't completed key exchange)
   */
  async establishSecureSession(peerId: string): Promise<MlsWelcome | null> {
    const result = await OfflineProtocolNativeModule.establishSecureSession(
      peerId
    );
    if (!result) return null;
    return {
      groupId: result.groupId,
      welcomeData: result.welcomeData,
      inviterId: result.inviterId,
      timestampMs: result.timestampMs,
    };
  }

  /**
   * Rotate the 1:1 session with a peer, advancing post-compromise security.
   *
   * Post-compromise security arrives when a commit rotates a member's leaf in
   * the ratchet tree, and the SDK originates one on a re-key. Nothing drives a
   * re-key on its own except an epoch desync, so a pair that never forks never
   * rotates unless the application asks. That bites hardest against a leaf
   * node — a lock, a sensor — which never commits at all, so every rotation in
   * such a pair is this side's to originate.
   *
   * The cadence is yours on purpose: a rotation costs a teardown, a key-package
   * exchange and a re-establish, and what that is worth depends on the
   * deployment rather than on anything the wire says.
   *
   * The peer sees exactly what a desync-driven re-key sends. Queued messages
   * survive, because they are sealed at flush time against whatever session is
   * current then.
   *
   * A rotation that fails changes nothing. The reset is advertised before the
   * local session is torn down, so a rejection leaves the session intact,
   * still usable, and the rate-limit window unspent. Rotate while the peer is
   * reachable and treat a failure as "try again later" rather than as a
   * session to rebuild.
   *
   * @param peerId - Peer's ID
   * @returns true when the rotation was driven; false when the per-peer
   *   rate-limit window has not lapsed. False is not a failure — call again
   *   later.
   * @throws Error if encryption is not initialized, the peer is blocked, there
   *   is no session to rotate (establish one first), or no transport carried
   *   the reset.
   */
  async rekeySession(peerId: string): Promise<boolean> {
    return await OfflineProtocolNativeModule.rekeySession(peerId);
  }

  private normalizeMlsGroupInfo(raw: any): MlsGroupInfo {
    return {
      groupId: raw.groupId,
      groupName: raw.groupName ?? raw.name ?? '',
      memberIds: raw.memberIds ?? raw.members ?? [],
      epoch: raw.epoch,
      createdAt: raw.createdAt ?? raw.createdAtMs ?? 0,
    };
  }

  private toMlsSessionInfo(raw: any): MlsSessionInfo {
    const members: string[] = raw.memberIds ?? raw.members ?? [];
    // Without our own address there is no way to tell which half of the pair
    // is the peer, and guessing picks us half the time. `start()` caches the
    // address as soon as MLS is up, so a null here means there is no session
    // to describe yet.
    const localAddress = this.cachedLocalAddress;
    const otherUserId =
      localAddress === null
        ? ''
        : members.find(memberId => memberId !== localAddress) ?? '';
    return {
      otherUserId,
      groupId: raw.groupId,
      epoch: raw.epoch,
      createdAt: raw.createdAt ?? raw.createdAtMs ?? 0,
    };
  }

  /**
   * Creates an MLS session with another user.
   * Returns a Welcome message that must be sent to the other user.
   *
   * Note: Prefer using `establishSecureSession` which handles the key package
   * flow automatically. This lower-level method requires the peer's key package
   * to already be imported via `mlsImportKeyPackage`.
   *
   * @param otherUserId - Other user's ID
   * @returns Welcome message to send to the other user
   * @throws Error if session creation fails
   */
  async mlsCreateSession(otherUserId: string): Promise<MlsWelcome> {
    const result = await OfflineProtocolNativeModule.mlsCreateSession(
      otherUserId
    );
    return {
      groupId: result.groupId,
      welcomeData: result.welcomeData,
      inviterId: result.inviterId,
      timestampMs: result.timestampMs,
    };
  }

  /**
   * Joins an MLS session from a Welcome message.
   *
   * @param welcome - Welcome message received from session creator
   * @returns Session info
   * @throws Error if joining fails
   */
  async mlsJoinSession(welcome: MlsWelcome): Promise<MlsSessionInfo> {
    const welcomeJson = JSON.stringify({
      groupId: welcome.groupId,
      welcomeData: welcome.welcomeData,
      inviterId: welcome.inviterId,
      timestampMs: welcome.timestampMs,
    });
    const result = await OfflineProtocolNativeModule.mlsJoinSession(
      welcomeJson
    );
    return this.toMlsSessionInfo(result);
  }

  /**
   * Encrypts a message for another user.
   * Creates a session automatically if one doesn't exist.
   *
   * @param otherUserId - Recipient's user ID
   * @param plaintext - Message content as bytes
   * @returns Encrypted message
   * @throws Error if encryption fails
   */
  async mlsEncryptForUser(
    otherUserId: string,
    plaintext: number[]
  ): Promise<MlsEncryptedMessage> {
    const result = await OfflineProtocolNativeModule.mlsEncryptForUser(
      otherUserId,
      plaintext
    );
    return {
      groupId: result.groupId,
      messageType: result.messageType,
      epoch: result.epoch,
      ciphertext: result.ciphertext,
      senderId: result.senderId,
      timestampMs: result.timestampMs,
    };
  }

  /**
   * Decrypts a message from another user.
   *
   * @param encrypted - Encrypted message
   * @returns Decrypted plaintext as bytes, or null if decryption fails
   */
  async mlsDecryptFromUser(
    encrypted: MlsEncryptedMessage
  ): Promise<number[] | null> {
    const encryptedJson = JSON.stringify({
      groupId: encrypted.groupId,
      messageType: encrypted.messageType,
      epoch: encrypted.epoch,
      ciphertext: encrypted.ciphertext,
      senderId: encrypted.senderId,
      timestampMs: encrypted.timestampMs,
    });
    return await OfflineProtocolNativeModule.mlsDecryptFromUser(encryptedJson);
  }

  /**
   * Decrypts any MLS message (1:1 or group).
   *
   * @param encrypted - Encrypted message
   * @returns Decrypted plaintext as bytes, or null if decryption fails
   */
  async mlsDecrypt(encrypted: MlsEncryptedMessage): Promise<number[] | null> {
    const encryptedJson = JSON.stringify({
      groupId: encrypted.groupId,
      messageType: encrypted.messageType,
      epoch: encrypted.epoch,
      ciphertext: encrypted.ciphertext,
      senderId: encrypted.senderId,
      timestampMs: encrypted.timestampMs,
    });
    return await OfflineProtocolNativeModule.mlsDecrypt(encryptedJson);
  }

  /**
   * Lists all active MLS sessions.
   *
   * @returns Array of user IDs with active sessions
   */
  async mlsListSessions(): Promise<string[]> {
    return await OfflineProtocolNativeModule.mlsListSessions();
  }

  /**
   * Deletes an MLS session with another user.
   *
   * @param otherUserId - Other user's ID
   * @throws Error if deletion fails
   */
  async mlsDeleteSession(otherUserId: string): Promise<void> {
    return await OfflineProtocolNativeModule.mlsDeleteSession(otherUserId);
  }

  /**
   * Processes a Welcome message (auto-detects session vs group).
   *
   * @param welcome - Welcome message
   * @returns Session or group info
   * @throws Error if processing fails
   */
  async mlsProcessWelcome(
    welcome: MlsWelcome
  ): Promise<MlsSessionInfo | MlsGroupInfo> {
    const welcomeJson = JSON.stringify({
      groupId: welcome.groupId,
      welcomeData: welcome.welcomeData,
      inviterId: welcome.inviterId,
      timestampMs: welcome.timestampMs,
    });
    const result = await OfflineProtocolNativeModule.mlsProcessWelcome(welcomeJson);
    if (result?.isSession) {
      return this.toMlsSessionInfo(result);
    }
    return this.normalizeMlsGroupInfo(result);
  }

  // ============================================================================
  // MLS GROUP METHODS
  // ============================================================================

  // ============================================================================
  // HIGH-LEVEL GROUP METHODS (MLS-encrypted, mesh transport)
  // ============================================================================

  /**
   * Creates a new MLS group with full protocol integration.
   * Emits GroupCreated event. Use inviteToGroup() to add members.
   *
   * @param groupName - Human-readable group name
   * @returns Group info
   */
  async meshCreateGroup(groupName: string): Promise<MlsGroupInfo> {
    const result = await OfflineProtocolNativeModule.meshCreateGroup(groupName);
    return this.normalizeMlsGroupInfo(result);
  }

  /**
   * Invites a user to an MLS group.
   * Sends MLS Welcome to invitee and Commit to existing members.
   * Emits GroupMemberAdded event on all participants.
   *
   * @param groupId - Group ID
   * @param inviteeUserId - User ID to invite
   */
  async meshInviteToGroup(groupId: string, inviteeUserId: string): Promise<void> {
    await OfflineProtocolNativeModule.meshInviteToGroup(groupId, inviteeUserId);
  }

  /**
   * Sends an MLS-encrypted message to all group members via mesh transport.
   * Handles encryption and fan-out to each member automatically.
   *
   * @param groupId - Group ID
   * @param content - Message content
   * @param priority - Optional priority ("low", "medium", "high", "critical")
   * @param replyToMsg - Optional message ID to reply to
   * @returns Array of per-member message IDs
   */
  async meshSendGroupMessage(
    groupId: string,
    content: string,
    priority?: string | null,
    replyToMsg?: string | null
  ): Promise<string[]> {
    return await OfflineProtocolNativeModule.meshSendGroupMessage(
      groupId,
      content,
      priority || null,
      replyToMsg || null
    );
  }

  /**
   * Forwards a message to all members of a group with forwarding attribution.
   *
   * The message content is encrypted via MLS for the group and fan-out follows
   * the same path as regular group messages.
   *
   * @param params - Forward to group parameters
   * @returns Array of per-member message IDs
   */
  async meshForwardMessageToGroup(params: ForwardMessageToGroupParams): Promise<string[]> {
    let priorityStr: string | null = null;
    if (params.priority != null) {
      const map: Record<number, string> = {
        [MessagePriority.Low]: 'low',
        [MessagePriority.Medium]: 'medium',
        [MessagePriority.High]: 'high',
        [MessagePriority.Critical]: 'critical',
      };
      priorityStr = map[params.priority] ?? null;
    }
    return await OfflineProtocolNativeModule.meshForwardMessageToGroup(
      params.originalMessageJson,
      params.groupId,
      priorityStr
    );
  }

  /**
   * Removes a member from an MLS group.
   * Sends removal notification to all members.
   *
   * @param groupId - Group ID
   * @param memberId - Member to remove
   */
  async meshRemoveFromGroup(groupId: string, memberId: string): Promise<void> {
    await OfflineProtocolNativeModule.meshRemoveFromGroup(groupId, memberId);
  }

  /**
   * Leaves an MLS group with notification to other members.
   *
   * @param groupId - Group ID to leave
   */
  async meshLeaveGroup(groupId: string): Promise<void> {
    await OfflineProtocolNativeModule.meshLeaveGroup(groupId);
  }

  /**
   * Lists all MLS groups (excluding 1:1 sessions).
   *
   * @returns Array of group IDs
   */
  async meshListGroups(): Promise<string[]> {
    return await OfflineProtocolNativeModule.meshListGroups();
  }

  /**
   * Gets information about an MLS group.
   *
   * @param groupId - Group ID
   * @returns Group info or null if not found
   */
  async meshGetGroupInfo(groupId: string): Promise<MlsGroupInfo | null> {
    const result = await OfflineProtocolNativeModule.meshGetGroupInfo(groupId);
    if (!result) return null;
    return {
      groupId: result.groupId,
      groupName: result.groupName ?? result.name ?? '',
      memberIds: result.memberIds ?? result.members ?? [],
      epoch: result.epoch,
      createdAt: result.createdAt ?? result.createdAtMs ?? 0,
    };
  }

  /**
   * Whether a rich group send right now would seal its extras, and which
   * members hold the gate closed. Point-in-time and advisory: the send path
   * re-evaluates the gate itself — use this to warn before sending instead
   * of learning from a GroupRichExtrasDropped event after the drop.
   *
   * @param groupId - Group ID
   * @returns Readiness snapshot ({ ready, unknownMembers })
   */
  async meshGroupRichReadiness(groupId: string): Promise<GroupRichReadiness> {
    const result = await OfflineProtocolNativeModule.meshGroupRichReadiness(groupId);
    return {
      ready: !!result?.ready,
      unknownMembers: result?.unknownMembers ?? [],
    };
  }

  /**
   * The relay-side registration state of a group. Point-in-time:
   * transitions arrive as `group_relay_sync_changed` events.
   *
   * `'synced'` means the relay positively acknowledged the group's
   * registration on the current connection — relay-dependent server
   * commands for it (`CreateGroupInviteLink` & co. via
   * `sendRawServerCommand`) can be issued. `'pending'` means a
   * registration is in flight; `'unsynced'` means none is (Internet down,
   * relay grouping disabled, or a prior attempt errored / timed out).
   *
   * @param groupId - Group ID
   */
  async groupRelaySyncState(groupId: string): Promise<RelaySyncState> {
    return await OfflineProtocolNativeModule.meshGroupRelaySyncState(groupId);
  }

  /**
   * Registers (or re-registers) a group with the relay server on demand —
   * the supported path for making a mesh-created group known to the relay
   * before issuing relay-dependent server commands for it. Never raw-send
   * `CreateGroup`: it desyncs the SDK's registration tracking.
   *
   * Fire-and-event: the outcome arrives as `group_relay_sync_changed`
   * (`ensureGroupRegistered` wraps the wait). Resolves true when the
   * registration frame was queued (or the group is already synced), false
   * when relay grouping is disabled or the Internet transport is
   * unavailable; rejects when the group is unknown locally.
   *
   * @param groupId - Group ID
   */
  async requestGroupRelayRegistration(groupId: string): Promise<boolean> {
    return await OfflineProtocolNativeModule.meshRequestGroupRelayRegistration(
      groupId
    );
  }

  /**
   * Resolves once the relay holds a positively acknowledged registration
   * for the group — the gate to await before `CreateGroupInviteLink` and
   * other relay-dependent raw server commands for a mesh-created group.
   *
   * Resolves immediately when the group is already synced; otherwise kicks
   * a registration (when none is in flight) and waits for the
   * `group_relay_sync_changed` outcome. Rejects on a negative outcome
   * (`reason: 'error' | 'ack_timeout' | …`), when the registration cannot
   * be sent (Internet down / relay grouping disabled / unknown group), or
   * on timeout.
   *
   * The SDK re-sends an unanswered registration every 30s up to 3 attempts
   * before giving up with `ack_timeout` (~90s worst case) — the default
   * timeout covers that full cycle. A shorter timeout is fine for UI
   * purposes: the SDK keeps retrying in the background and a later
   * `group_relay_sync_changed` still fires on success.
   *
   * @param groupId - Group ID
   * @param options - `timeoutMs`: how long to wait (default 100000)
   */
  async ensureGroupRegistered(
    groupId: string,
    options?: { timeoutMs?: number }
  ): Promise<void> {
    const timeoutMs = options?.timeoutMs ?? 100_000;

    // Subscribe BEFORE the state check: an ack landing between check and
    // subscribe would otherwise be missed and hang this until timeout.
    let settle: (() => void) | undefined;
    const outcome = new Promise<void>((resolve, reject) => {
      const listener: EventListener<GroupRelaySyncChangedEvent> = (event) => {
        if (event.group_id !== groupId) return;
        if (event.synced) {
          resolve();
        } else {
          reject(
            new Error(
              `Relay registration for group ${groupId} failed: ${event.reason}`
            )
          );
        }
      };
      this.on('group_relay_sync_changed', listener);
      settle = () => this.off('group_relay_sync_changed', listener);
    });

    try {
      const state = await this.groupRelaySyncState(groupId);
      if (state === 'synced') return;
      if (state === 'unsynced') {
        // Kick a registration; a clean false means it cannot reach the
        // relay at all — fail fast rather than waiting out the timeout.
        const queued = await this.requestGroupRelayRegistration(groupId);
        if (!queued) {
          throw new Error(
            `Cannot register group ${groupId}: relay grouping disabled or Internet transport unavailable`
          );
        }
      }
      // state === 'pending' (or just kicked): await the relay's answer.
      let timer: ReturnType<typeof setTimeout> | undefined;
      try {
        await Promise.race([
          outcome,
          new Promise<never>((_, reject) => {
            timer = setTimeout(
              () =>
                reject(
                  new Error(
                    `Timed out after ${timeoutMs}ms waiting for relay registration of group ${groupId}`
                  )
                ),
              timeoutMs
            );
          }),
        ]);
      } finally {
        if (timer !== undefined) clearTimeout(timer);
      }
    } finally {
      settle?.();
      // The outcome promise may reject after we've already returned or
      // thrown (e.g. a later revocation caught by the still-registered
      // listener in the same tick); a detached rejection must not surface
      // as an unhandled rejection.
      outcome.catch(() => {});
    }
  }

  /**
   * Sets a member's role in an MLS group (admin only).
   * Broadcasts role change to all group members.
   *
   * @param groupId - Group ID
   * @param userId - Target member's user ID
   * @param role - New role ("admin" or "member")
   */
  async meshSetMemberRole(
    groupId: string,
    userId: string,
    role: string
  ): Promise<void> {
    await OfflineProtocolNativeModule.meshSetMemberRole(groupId, userId, role);
  }

  /**
   * Gets a member's role in an MLS group.
   *
   * @param groupId - Group ID
   * @param userId - Member's user ID
   * @returns Role string ("admin" or "member")
   */
  async meshGetMemberRole(
    groupId: string,
    userId: string
  ): Promise<string> {
    return await OfflineProtocolNativeModule.meshGetMemberRole(
      groupId,
      userId
    );
  }

  /**
   * Gets all member roles in an MLS group.
   *
   * @param groupId - Group ID
   * @returns Map of user_id -> role
   */
  async meshGetGroupRoles(
    groupId: string
  ): Promise<Record<string, string>> {
    return await OfflineProtocolNativeModule.meshGetGroupRoles(groupId);
  }

  /**
   * Renames an MLS group (admin only).
   * Broadcasts the rename to all group members.
   *
   * @param groupId - Group ID
   * @param newName - New group name
   * @throws Error if not admin or group not found
   */
  async meshRenameGroup(groupId: string, newName: string): Promise<void> {
    await OfflineProtocolNativeModule.meshRenameGroup(groupId, newName);
  }

  /**
   * Destroys the protocol instance and cleans up resources
   */
  async destroy(): Promise<void> {
    // Remove all event listeners
    this.removeAllListeners();
    this.telemetryListeners.clear();
    this.droppedEventTypesWarned.clear();

    // Remove native event subscription
    if (this.eventSubscription) {
      this.eventSubscription.remove();
      this.eventSubscription = null;
    }
    if (this.telemetrySubscription) {
      this.telemetrySubscription.remove();
      this.telemetrySubscription = null;
    }

    // Destroy native protocol instance
    if (this.isCreated) {
      await OfflineProtocolNativeModule.destroy();
      this.isCreated = false;
    }

    this.initialRuntimeConfigApplied = false;

    // The address belonged to the destroyed instance. An app may re-create
    // this object against a different profile, or wipe this profile's storage
    // and come back with a freshly minted identity — the documented cleanup
    // flow does exactly that — and a surviving cache would answer
    // `localAddress()` with a dead identity for the rest of the process, never
    // re-reading the native side because the cache hit short-circuits it.
    this.cachedLocalAddress = null;

    // The session these held one-shot events belong to is over, so there is
    // nobody left to redeliver them to — and an instance can be started again
    // (`start()` re-creates), where a survivor would be handed to the next
    // session's first `on(...)`, which the documented order puts *before*
    // `start()` and its sweep. That is the stale redelivery this mechanism
    // exists to prevent, arriving by the one route `start()` cannot see.
    //
    // The yield is what makes the clear reach it. `removeAllListeners` above
    // empties the map, so a replay scheduled by an `on(...)` in the same tick
    // as this call finds nothing listening and re-holds its event — that is
    // `emitEvent` refusing to lose what it could not deliver, correct in
    // general and unwanted only here. Microtasks run in order, so awaiting
    // one queued now resumes strictly after that replay and the clear sees
    // what it left behind. Nothing can be held past this point: the native
    // subscription is gone. Awaiting unconditionally rather than leaning on
    // the native `destroy()` above, which an uncreated instance skips.
    await Promise.resolve();
    this.pendingOneShotEvents.clear();
  }

  /**
   * Erases every byte of persisted SDK state for one account: the namespaced
   * secure store (MLS identity, sessions, peer trust records, the Nostr signing secret,
   * the protocol-state record key), the account's protocol-state directory
   * (outbox, pending queues, block list, media descriptors), and — when this
   * account owns it, or nobody does — the pre-namespace store an upgraded
   * install inherited from.
   *
   * Call this on logout and on username switch, **after** `destroy()`. The
   * protocol persists as it works, so wiping underneath a live instance races
   * those writes; the native side rejects the call if the account named here is
   * the one the current instance is running.
   *
   * Without it, an account's undelivered messages are restored and re-driven on
   * the next launch for the lifetime of the outbox, and on iOS — where the
   * Keychain outlives the app container — its identity and delivery state
   * survive an uninstall and are adopted again after a reinstall.
   *
   * The account is named explicitly because `destroy()` clears the config the
   * namespace would otherwise be derived from. Pass the same `appId` and
   * `profile` the protocol was created with; any other pair names a different
   * account and wipes nothing.
   *
   * Irreversible, and it rotates the account's MLS and Nostr identities: peers
   * holding a session with it will see a desync on next contact and re-establish
   * from a fresh key package. Safe to call twice — a failed wipe should simply
   * be retried.
   *
   * Applications that supply their own storage providers must erase their own
   * containers: this only knows about the built-in ones.
   *
   * @param appId - The `appId` the protocol was created with
   * @param profile - The `profile` the protocol was created with. The
   *   namespace hash is unchanged from before the rename, so passing a
   *   pre-migration `userId` here reaches that account's old container.
   */
  async wipePersistedState(appId: string, profile: string): Promise<void> {
    await OfflineProtocolNativeModule.wipePersistedState(appId, profile);
  }

  // ─── Presence, Typing, Read Receipts ────────────────────────

  /**
   * Sends a presence update to a peer.
   *
   * @param recipient - Recipient's user ID
   * @param status - Presence status ('online', 'away', or 'offline')
   * @returns Message ID
   */
  async sendPresenceUpdate(
    recipient: string,
    status: 'online' | 'away' | 'offline'
  ): Promise<string> {
    const statusCode = status === 'online' ? 0 : status === 'away' ? 1 : 2;
    return await OfflineProtocolNativeModule.sendPresenceUpdate(
      recipient,
      statusCode
    );
  }

  /**
   * Asks the internet relay for a peer's presence (one-shot CheckPresence).
   *
   * ## Contract
   *
   * - **Always fresh.** The SDK never throttles or dedupes manual checks:
   *   every accepted call sends a new `CheckPresence` frame to the relay,
   *   regardless of how recently the same peer was queried. (The automatic
   *   watch loop's tick/TTL policy does not apply here.)
   * - **Fire-and-event.** The answer arrives as a `presence_updated` event
   *   with `source: 'internet'` (including `last_seen_ms` when the relay
   *   knows it) rather than in the returned promise. Every relay answer
   *   re-emits the event **even when nothing changed** — safe to drive a
   *   chat-header refresh from. Subscribe before calling; events have no
   *   replay.
   * - **Exceptions.** The core suppresses presence for blocked peers and
   *   your own user id — for those, this resolves `true` (the query was
   *   sent) but no `presence_updated` follows. And `true` means the query
   *   reached the socket, not that an answer will arrive: a connection
   *   dropped before the relay replies loses the answer (call again).
   * - **Rate limiting is never bypassed**, force or not: the SDK's
   *   client-side limiter mirrors the relay's per-connection budget, and an
   *   over-budget frame would be dropped server-side *after* a locally
   *   "successful" write — strictly worse than deferring.
   *
   * `options.force` is for chat open/focus: exactly when the app wants a
   * fresh header, the socket is often still resuming from background. A
   * non-forced call fails fast (`false`) in that window; a forced call is
   * parked and retried until the transport is authenticated and the
   * limiter admits it (up to ~8s), only then resolving `false`. On a
   * stopped transport (no reconnect coming) even forced calls fail fast.
   * Forced checks stay one-shot — they never join the SDK's automatic
   * watch set.
   *
   * @param userId - Peer's user ID
   * @param options - `force`: park through the reconnect/rate-limit window
   *          instead of failing fast (default false)
   * @returns true once the socket accepted the query (write-confirmed on
   *          iOS, enqueue-confirmed on Android — the closest OkHttp offers);
   *          false otherwise — an empty `userId` (never sent), or not
   *          connected+authenticated / rate-limiter-deferred past the
   *          force deadline (non-forced: immediately; safe to retry)
   * @throws when the internet transport was never initialized (enable it via
   *         `transports.internet` before calling)
   */
  async checkInternetPresence(
    userId: string,
    options?: { force?: boolean }
  ): Promise<boolean> {
    return await OfflineProtocolNativeModule.checkInternetPresence(userId, {
      force: options?.force === true,
    });
  }

  /**
   * Sends a raw, caller-built relay command verbatim over the SDK's internet
   * socket — the generic server-command channel for relay features that are
   * app concerns rather than SDK APIs (the invite-link lifecycle:
   * `CreateGroupInviteLink`, `JoinGroupViaInvite`, `AckGroupInviteJoin`, …).
   *
   * Responses the SDK doesn't consume arrive as `internet_server_message`
   * events carrying the verbatim frame. `GroupInfo` and `UserGroups` are also
   * emitted on that channel in addition to their stable typed events, so
   * application-owned extension fields remain lossless. Correlate
   * request/response with your own `request_id` where the relay supports one.
   *
   * Gate calls on `isInternetReady()` / the `internet_status_changed` event
   * rather than probing with a command and retrying on false. For
   * group-scoped commands (`CreateGroupInviteLink`, …) additionally await
   * `ensureGroupRegistered(groupId)` first — the relay must know the group.
   *
   * Do not send frame types the SDK itself manages (`SendMessage`,
   * `CreateGroup` / member deltas, `LeaveGroup`, `CheckPresence`, …): the
   * SDK cannot correlate their answers with its own in-flight state, and a
   * raw `CreateGroup`/`LeaveGroup` desyncs the bridge's registration
   * tracking. Use the typed SDK APIs for those.
   *
   * @param json - A complete relay frame, e.g.
   *   `{"type":"CreateGroupInviteLink","group_id":"g1","expires_in_secs":604800,"request_id":"req_…"}`
   * @returns true once the socket accepted the command (write-confirmed on
   *          iOS, enqueue-confirmed on Android — the closest OkHttp offers);
   *          false when not connected+authenticated, the JSON is invalid, or
   *          the SDK's client-side rate limiter deferred it (the SDK's
   *          mirror sits slightly under the relay's 30-burst/10-per-second
   *          budget, at 28 burst / 9 per second — safe to retry after a
   *          short delay)
   * @throws when the internet transport was never initialized (enable it via
   *         `transports.internet` before calling)
   */
  async sendRawServerCommand(json: string): Promise<boolean> {
    return await OfflineProtocolNativeModule.internetSendRawCommand(json);
  }

  /**
   * Whether the SDK's internet socket is connected AND relay-authenticated
   * — the same gate `sendRawServerCommand` checks before writing. The
   * positive replacement for app-side `relayStatus === 'authenticated'`
   * tracking: gate raw sends on this (or on the `internet_status_changed`
   * event) instead of probing with a command and retrying on false.
   *
   * Point-in-time; transitions arrive as `internet_status_changed` events.
   * A ready socket can still defer an individual send (client-side rate
   * limiter) — `sendRawServerCommand` returning false while ready means
   * retry after a short delay.
   *
   * Cannot tell you *why* it is false. An ordinary disconnect (which
   * reconnects itself) and a relay displacement (which never will) both read
   * false here — use {@link isInternetSuperseded} to tell them apart.
   *
   * @returns true when connected and authenticated; false otherwise,
   *          including when the internet transport was never initialized
   *          (never throws)
   */
  async isInternetReady(): Promise<boolean> {
    return await OfflineProtocolNativeModule.internetIsReady();
  }

  /**
   * Whether the relay displaced this session — another device registered the
   * same identity and took over the relay slot — and the SDK latched the
   * internet transport stopped.
   *
   * This is the question {@link isInternetReady} structurally cannot answer.
   * A `false` from it means "not usable right now" and nothing more: an
   * ordinary disconnect reconnects itself within seconds, while a displaced
   * session **will never reconnect on its own**, because a blind reconnect
   * would just re-displace the other device in a loop. The two are
   * indistinguishable from readiness alone.
   *
   * True here means the only recovery is a deliberate re-enable:
   *
   * ```ts
   * if (await sdk.isInternetSuperseded()) {
   *   // Surface "connected elsewhere" and let the user decide.
   *   await sdk.enableTransport('internet', { serverAddress });  // clears the latch
   * }
   * ```
   *
   * Complements the `internet_session_superseded` event rather than replacing
   * it. The event tells an app that is listening at that instant; this answers
   * an app that asks — including one that subscribed later, or whose process
   * was killed and restarted, which no in-memory event hold survives. A
   * foreground reconcile against this is the most robust shape.
   *
   * @returns true while the session is superseded; false otherwise, including
   *          when the internet transport was never initialized (never throws)
   */
  async isInternetSuperseded(): Promise<boolean> {
    return await OfflineProtocolNativeModule.internetIsSuperseded();
  }

  /**
   * Forces an immediate teardown + reconnect + re-authenticate of the SDK's
   * internet socket, bypassing the exponential reconnect backoff.
   *
   * `isInternetReady()` is a point-in-time cached flag, not a liveness probe:
   * an OS suspend can kill the TCP connection before a clean WebSocket close,
   * so after a background→foreground transition the socket may be a zombie
   * (dead but still reported ready) or alive-but-deregistered by the relay.
   * Neither is detectable by a ping, and both are healed by the same action —
   * a full reconnect that re-runs the relay authenticate/register handshake.
   *
   * You normally do NOT need to call this on foreground: as of the automatic
   * foreground-heal, both native bridges (iOS `applicationWillEnterForeground`,
   * Android `onHostResume`) already force a reconnect themselves after a
   * background stay long enough to have killed the socket (~4s), gated on
   * monotonic background duration — and iOS additionally tears down a zombie on
   * the first stalled write via the write-stall watchdog. Calling this method
   * in addition, on every foreground, would double-reconnect and drop a
   * genuinely-healthy socket, forcing a wasted group re-registration round-trip.
   *
   * Keep it for the cases the automatic heal does not cover: a deliberate
   * user-initiated "reconnect now", or a stale socket you detect while already
   * foregrounded (e.g. a long idle period with no background transition). If you
   * do drive foreground recovery yourself, debounce and gate on background
   * duration rather than calling on every foreground.
   *
   * Recovery lands in ~1s rather than waiting ~20-30s for zombie ping detection.
   *
   * Emits a transient `internet_status_changed` down→up. No-op unless the
   * internet transport is running (respects the enable/disable lifecycle).
   *
   * @returns true once the request reached a live internet transport — this
   *          means "accepted", not "reconnected": it is also true when the
   *          transport is initialized but not currently running/starting, in
   *          which case the call is a deliberate no-op. false only when the
   *          internet transport was never initialized. Never throws.
   */
  async forceInternetReconnect(): Promise<boolean> {
    return await OfflineProtocolNativeModule.internetForceReconnect();
  }

  /**
   * Sends a typing indicator to a peer.
   *
   * @param recipient - Recipient's user ID
   * @param conversationId - Opaque conversation key. The SDK carries it to
   *   the peer and echoes it back on `typing_indicator_received`; it is never
   *   parsed or routed on. Conventionally the peer's `off1…` address for a
   *   DM and the group id for a group.
   *
   *   Whatever you choose, make sure it is stable: if your app keys stored
   *   conversations by this value, deriving it from a display name (or any
   *   other mutable label) means the key changes under you and the history
   *   stops being found.
   * @param isTyping - Whether the user is currently typing
   * @returns Message ID
   */
  async sendTypingIndicator(
    recipient: string,
    conversationId: string,
    isTyping: boolean
  ): Promise<string> {
    return await OfflineProtocolNativeModule.sendTypingIndicator(
      recipient,
      conversationId,
      isTyping
    );
  }

  /**
   * Sends a read receipt to a peer.
   *
   * @param recipient - Recipient's user ID
   * @param messageIds - Array of message IDs that were read
   * @returns Message ID
   */
  async sendReadReceipt(
    recipient: string,
    messageIds: string[]
  ): Promise<string> {
    return await OfflineProtocolNativeModule.sendReadReceipt(
      recipient,
      messageIds
    );
  }

  // ─── User Blocking ──────────────────────────────────────────

  /**
   * Blocks a user. Messages from blocked users are silently dropped at the protocol level.
   *
   * @param userId - User ID to block
   */
  async blockUser(userId: string): Promise<void> {
    await OfflineProtocolNativeModule.blockUser(userId);
  }

  /**
   * Unblocks a previously blocked user.
   *
   * @param userId - User ID to unblock
   */
  async unblockUser(userId: string): Promise<void> {
    await OfflineProtocolNativeModule.unblockUser(userId);
  }

  /**
   * Returns the list of blocked user IDs.
   *
   * @returns Array of blocked user IDs
   */
  async getBlockedUsers(): Promise<string[]> {
    return await OfflineProtocolNativeModule.getBlockedUsers();
  }

  /**
   * Checks if a specific user is blocked.
   *
   * @param userId - User ID to check
   * @returns true if the user is blocked
   */
  async isUserBlocked(userId: string): Promise<boolean> {
    return await OfflineProtocolNativeModule.isUserBlocked(userId);
  }
}

/**
 * A value stored in a document collection.
 *
 * Structured values go in as JSON strings and merge whole (last write wins
 * per key). That is the honest description of what v1 replicates: whole
 * collections within a space, with no nested addressing and no query
 * language.
 *
 * `attachment` is the one composite, and it is composite because it is a
 * reference rather than a value: the bytes it names never enter the
 * document. It is replaced, never edited, so two people attaching different
 * blobs to one key resolve like any other value and neither replica ends up
 * holding a hash from one beside a size from another.
 *
 * Every member of this union can come back from {@link DataStore.mapGet},
 * `attachment` included, so a switch over `kind` must handle it. A peer on a
 * build predating attachments reads such a value as absent instead.
 */
export type DataValue =
  | { kind: 'null' }
  | { kind: 'bool'; value: boolean }
  | { kind: 'int'; value: number }
  | { kind: 'float'; value: number }
  | { kind: 'text'; value: string }
  | { kind: 'bytes'; value: number[] }
  | {
      kind: 'attachment';
      /** Lowercase hex SHA-256 of the blob, exactly 64 characters. */
      hash: string;
      /** Length of the blob in bytes. Non-zero. */
      size: number;
      /** Display name, if the writer had one. Never treat it as a path. */
      name?: string;
      /** Media type, if the writer knew it. */
      mime?: string;
    };

/**
 * Replicated documents: offline-first state any member of a space can edit
 * while disconnected, merging deterministically when replicas meet again.
 *
 * Messaging is synced events; this is synced state.
 *
 * Requires `initializeMlsWithSecureStorage()` to have run — documents are
 * sealed at rest with the same per-install key as every other protocol
 * record, and that key is minted there — and `data.enabled` set in the
 * config. Every method answers `DataDisabled` until it is.
 *
 * Edits batch before they reach storage. Call {@link flush} when the app
 * must know a change is durable; the SDK also flushes on shutdown.
 *
 * @example
 * ```typescript
 * const store = new DataStore();
 * await store.mapSet('space-1', 'profile', 'fields', 'name', {
 *   kind: 'text',
 *   value: 'Ada',
 * });
 * await store.flush('space-1', 'profile');
 * const state = await store.docJson('space-1', 'profile');
 * ```
 */
export class DataStore {
  /** Creates a document, or does nothing if it already exists. */
  async createDoc(spaceId: string, docId: string): Promise<void> {
    await OfflineProtocolNativeModule.dataCreateDoc(spaceId, docId);
  }

  /** Deletes a document and every record belonging to it. */
  async deleteDoc(spaceId: string, docId: string): Promise<void> {
    await OfflineProtocolNativeModule.dataDeleteDoc(spaceId, docId);
  }

  /** The documents in a space. */
  async listDocs(spaceId: string): Promise<string[]> {
    return JSON.parse(await OfflineProtocolNativeModule.dataListDocs(spaceId));
  }

  /** Every space that holds at least one document. */
  async listSpaces(): Promise<string[]> {
    return JSON.parse(await OfflineProtocolNativeModule.dataListSpaces());
  }

  /** Sets a key in a map collection. */
  async mapSet(
    spaceId: string,
    docId: string,
    collection: string,
    key: string,
    value: DataValue
  ): Promise<void> {
    await OfflineProtocolNativeModule.dataMapSet(
      spaceId,
      docId,
      collection,
      key,
      JSON.stringify(value)
    );
  }

  /** Removes a key from a map collection. */
  async mapDelete(
    spaceId: string,
    docId: string,
    collection: string,
    key: string
  ): Promise<void> {
    await OfflineProtocolNativeModule.dataMapDelete(
      spaceId,
      docId,
      collection,
      key
    );
  }

  /** Reads a key from a map collection, or null when it is absent. */
  async mapGet(
    spaceId: string,
    docId: string,
    collection: string,
    key: string
  ): Promise<DataValue | null> {
    const json = await OfflineProtocolNativeModule.dataMapGetJson(
      spaceId,
      docId,
      collection,
      key
    );
    return json == null ? null : JSON.parse(json);
  }

  /** Appends to a list collection. */
  async listPush(
    spaceId: string,
    docId: string,
    collection: string,
    value: DataValue
  ): Promise<void> {
    await OfflineProtocolNativeModule.dataListPush(
      spaceId,
      docId,
      collection,
      JSON.stringify(value)
    );
  }

  /** Deletes entries from a list collection. */
  async listDelete(
    spaceId: string,
    docId: string,
    collection: string,
    index: number,
    count: number
  ): Promise<void> {
    await OfflineProtocolNativeModule.dataListDelete(
      spaceId,
      docId,
      collection,
      index,
      count
    );
  }

  /** The number of entries in a list collection. */
  async listLength(
    spaceId: string,
    docId: string,
    collection: string
  ): Promise<number> {
    return await OfflineProtocolNativeModule.dataListLen(
      spaceId,
      docId,
      collection
    );
  }

  /**
   * Inserts into a text collection.
   *
   * `position` is a character offset, not a byte offset.
   */
  async textInsert(
    spaceId: string,
    docId: string,
    collection: string,
    position: number,
    text: string
  ): Promise<void> {
    await OfflineProtocolNativeModule.dataTextInsert(
      spaceId,
      docId,
      collection,
      position,
      text
    );
  }

  /** Deletes characters from a text collection. */
  async textDelete(
    spaceId: string,
    docId: string,
    collection: string,
    position: number,
    count: number
  ): Promise<void> {
    await OfflineProtocolNativeModule.dataTextDelete(
      spaceId,
      docId,
      collection,
      position,
      count
    );
  }

  /** The contents of a text collection. */
  async textValue(
    spaceId: string,
    docId: string,
    collection: string
  ): Promise<string> {
    return await OfflineProtocolNativeModule.dataTextValue(
      spaceId,
      docId,
      collection
    );
  }

  /** Adds to a counter collection. Negative amounts subtract. */
  async counterIncrement(
    spaceId: string,
    docId: string,
    collection: string,
    amount: number
  ): Promise<void> {
    await OfflineProtocolNativeModule.dataCounterIncrement(
      spaceId,
      docId,
      collection,
      amount
    );
  }

  /** The value of a counter collection. */
  async counterValue(
    spaceId: string,
    docId: string,
    collection: string
  ): Promise<number> {
    return await OfflineProtocolNativeModule.dataCounterValue(
      spaceId,
      docId,
      collection
    );
  }

  /**
   * The document's current state as plain JSON.
   *
   * Half of the escape hatch: an app can always take its data and leave,
   * and this half needs no knowledge of the SDK to read.
   */
  async docJson(spaceId: string, docId: string): Promise<unknown> {
    return JSON.parse(
      await OfflineProtocolNativeModule.dataDocJson(spaceId, docId)
    );
  }

  /**
   * The document's full history, base64-encoded.
   *
   * The other half of the escape hatch. Prefer {@link docJson} unless the
   * history itself is what you need.
   */
  async exportRaw(spaceId: string, docId: string): Promise<string> {
    return await OfflineProtocolNativeModule.dataExportRaw(spaceId, docId);
  }

  /**
   * Persists pending edits to a document.
   *
   * Edits batch before they reach a record, so call this when the app must
   * know a change survives a crash. The SDK also flushes on shutdown.
   */
  async flush(spaceId: string, docId: string): Promise<void> {
    await OfflineProtocolNativeModule.dataFlush(spaceId, docId);
  }

  /** Persists pending edits to every open document. */
  async flushAll(): Promise<void> {
    await OfflineProtocolNativeModule.dataFlushAll();
  }

  /** The compacted size of a document, in bytes. */
  async docSize(spaceId: string, docId: string): Promise<number> {
    return await OfflineProtocolNativeModule.dataDocSize(spaceId, docId);
  }

  /**
   * Deletes every record the data layer owns.
   *
   * Only needed when documents were pointed at a storage backend the app
   * supplied: `wipePersistedState()` clears the default provider's account
   * directory, which a custom backend is not inside. Skipping it there
   * leaves documents behind after the account that made them is gone.
   *
   * Only durable once replication has stopped. There are no deletion
   * tombstones, so a peer cannot tell a wiped space from one this device has
   * never seen, and with the engine running and sessions live its next
   * version offer recreates and refills every document, with no error and no
   * event. Logout tears the engine down anyway; call `destroy()` first if you
   * are wiping for any other reason, and only for as long as it stays stopped:
   * the peer still holds the documents, so they return when replication
   * resumes. This clears the device, it does not delete content.
   */
  async wipeAll(): Promise<void> {
    await OfflineProtocolNativeModule.dataWipeAll();
  }

  // ---- attachments ------------------------------------------------------
  //
  // Blob bytes never enter a document and never enter protocol state: a
  // document holds a reference and the bytes ride the media path. The
  // consequence for your app is that YOU own the bytes. The SDK cannot
  // answer a peer's request on its own because it never kept a copy, so it
  // asks you, through the `data_attachment_requested` event.

  /**
   * The address of some bytes, in the spelling a reference uses.
   *
   * Write the result into a document with `mapSet`, which takes the value
   * as an object and encodes it for you:
   *
   * ```ts
   * const hash = await store.attachmentHash(bytesBase64);
   * await store.mapSet(space, 'notes', 'files', 'plan', {
   *   kind: 'attachment', hash, size: byteLength, name: 'plan.pdf',
   * });
   * ```
   *
   * Compute it here rather than anywhere else. Two spellings of one address
   * are two addresses: they fetch twice, store twice, and compare unequal
   * while naming identical bytes.
   *
   * @param bytesBase64 The blob, base64.
   */
  async attachmentHash(bytesBase64: string): Promise<string> {
    return await OfflineProtocolNativeModule.dataAttachmentHash(bytesBase64);
  }

  /**
   * Asks the peer a 1:1 space is named after for the bytes behind a
   * reference.
   *
   * Pull rather than push, because a space may reference more bytes than a
   * phone wants over Bluetooth: the decision to spend that is yours, per
   * blob, when somebody opens one. The answer arrives later as
   * `data_attachment_received` or `data_attachment_unavailable`, never from
   * this call.
   *
   * Group spaces are refused in this version: a blob rides a transfer to a
   * confirmed 1:1 session, and two group members need not have one.
   */
  async fetchAttachment(spaceId: string, hash: string): Promise<void> {
    await OfflineProtocolNativeModule.dataFetchAttachment(spaceId, hash);
  }

  /**
   * Answers a peer's `data_attachment_requested` with the bytes.
   *
   * Rejects bytes that do not hash to `hash`, so a mistake reaches you while
   * you still have the file in hand rather than travelling the whole media
   * path to be refused on the other side.
   *
   * @param bytesBase64 The blob, base64.
   */
  async provideAttachment(
    spaceId: string,
    peerId: string,
    hash: string,
    bytesBase64: string
  ): Promise<void> {
    await OfflineProtocolNativeModule.dataProvideAttachment(spaceId, peerId, hash, bytesBase64);
  }

  /**
   * Tells a peer their request will not be answered.
   *
   * Answer this way when you no longer hold the bytes. It is a real answer
   * rather than silence: a reference outlives the bytes it names, and without
   * this the asking side cannot tell a peer that lost the file from one that
   * is merely slow, so it shows somebody a spinner that never resolves.
   */
  async declineAttachment(spaceId: string, peerId: string, hash: string): Promise<void> {
    await OfflineProtocolNativeModule.dataDeclineAttachment(spaceId, peerId, hash);
  }
}

/**
 * Standalone mesh services interface.
 *
 * Provides a focused API for service registration, discovery, and
 * request/response on the mesh network.
 *
 * @example
 * ```typescript
 * const protocol = new OfflineProtocol(config);
 * await protocol.start();
 * const services = new MeshServices();
 * await services.registerService('weather.v1', '1.0', { format: 'json' });
 * const queryId = await services.discoverServices();
 * ```
 */
export class MeshServices {
  /**
   * Registers a local service that this node offers for discovery by other peers.
   *
   * @param serviceId - Unique service identifier (e.g., "weather.v1")
   * @param version - Service version string
   * @param capabilities - Key-value map of service capabilities
   */
  async registerService(
    serviceId: string,
    version: string,
    capabilities: Record<string, string> = {}
  ): Promise<void> {
    await OfflineProtocolNativeModule.registerService(
      serviceId,
      version,
      JSON.stringify(capabilities)
    );
  }

  /**
   * Unregisters a local service.
   *
   * @param serviceId - Service identifier to unregister
   * @returns true if the service was found and removed
   */
  async unregisterService(serviceId: string): Promise<boolean> {
    return await OfflineProtocolNativeModule.unregisterService(serviceId);
  }

  /**
   * Broadcasts a service discovery query to the mesh.
   * Responses arrive asynchronously as `service_discovered` events.
   *
   * @param serviceId - Optional service ID to filter by (null discovers all)
   * @returns Query ID for correlating responses
   */
  async discoverServices(serviceId?: string): Promise<string> {
    return await OfflineProtocolNativeModule.discoverServices(serviceId ?? null);
  }

  /**
   * Sends a service request to a specific provider peer.
   * The response arrives as a `service_response_received` event.
   *
   * @param provider - Peer ID of the service provider
   * @param serviceId - Service identifier
   * @param method - Method name or action to invoke
   * @param body - Request body (JSON string or arbitrary string)
   * @returns Request ID for correlating the response
   */
  async sendServiceRequest(
    provider: string,
    serviceId: string,
    method: string,
    body: string
  ): Promise<string> {
    return await OfflineProtocolNativeModule.sendServiceRequest(
      provider,
      serviceId,
      method,
      body
    );
  }

  /**
   * Responds to a service request from another peer.
   * Call this after receiving a `service_request_received` event.
   *
   * @param requestId - Request ID from the received event
   * @param requester - Peer ID of the requester
   * @param serviceId - Service identifier
   * @param status - Response status ("ok", "error", or custom)
   * @param body - Response body
   * @returns Message ID of the response
   */
  async respondToServiceRequest(
    requestId: string,
    requester: string,
    serviceId: string,
    status: string,
    body: string
  ): Promise<string> {
    return await OfflineProtocolNativeModule.respondToServiceRequest(
      requestId,
      requester,
      serviceId,
      status,
      body
    );
  }
}

/**
 * Registers the task that restores the mesh after Android killed your process.
 * No-op on iOS, which has no equivalent restart to hook.
 *
 * Android can kill the app process while mesh is running. The keep-alive
 * service is `START_STICKY`, so the system hands the service back — but the SDK
 * will not rebuild the protocol from there, because a protocol with no
 * JavaScript behind it *destroys* the messages it receives and tells their
 * senders they arrived (see §6.2 of the React Native integration guide). This
 * task is the sound alternative: JavaScript is started *first*, so a receiver
 * exists before a protocol does, and your app — not the SDK — decides whether
 * to bring the mesh back.
 *
 * Opting in takes both halves. Add the manifest flag to your
 * `<application>` block:
 *
 * ```xml
 * <meta-data android:name="com.offlineprotocol.MESH_WAKE_ENABLED"
 *            android:value="true" />
 * ```
 *
 * and register the task at **module scope** in `index.js`, next to
 * `AppRegistry.registerComponent` — not inside a component, which will not have
 * mounted:
 *
 * ```js
 * import { registerMeshWakeTask } from '@offline-protocol/mesh-sdk';
 *
 * registerMeshWakeTask(async () => {
 *   if (getLiveProtocol()) return;          // already running: nothing to do
 *   const config = await loadSavedConfig(); // your storage, your credentials
 *   if (!config) return;                    // logged out: stay down
 *   const protocol = new OfflineProtocol(config);
 *   protocol.on('message_received', persistMessage); // BEFORE start()
 *   await protocol.start();
 *   await protocol.enableTransport('internet', { serverAddress, authToken });
 * });
 * ```
 *
 * Four obligations, each of which the SDK cannot meet for you:
 *
 * 1. **Store what you receive, durably, before `start()`.** The core never
 *    persists inbound content, and the receive path ACKs a message before it
 *    emits it — so a handler registered late, or one that only updates React
 *    state, loses the message *and* has already told its sender otherwise. This
 *    is the single reason the wake is opt-in rather than a default.
 * 2. **Be idempotent and cheap when there is nothing to do.** The task is
 *    allowed to run while the app is in the foreground (the alternative is a
 *    process crash if the user opens the app mid-wake), so it can find a live
 *    protocol. Return early rather than building a second one.
 * 3. **Re-issue anything `start()` does not restore** — Wi-Fi Direct always, and
 *    the relay whenever its `serverAddress`/`authToken` reach the SDK through
 *    `enableTransport('internet', ...)`. Same list as a normal cold launch.
 * 4. **Resolve promptly.** The keep-alive holds the process; the task does not
 *    need to. It has a bounded budget (60s by default, override with the
 *    `com.offlineprotocol.MESH_WAKE_TIMEOUT_SECONDS` meta-data), after which
 *    React Native terminates it.
 *
 * If the task never registers, throws, or declines, the keep-alive stops itself
 * on a watchdog rather than leaving a "Mesh Active" notification over a mesh
 * that is not running — the same honest outcome as before you opted in.
 *
 * Requires React Native 0.76.5+ when the New Architecture is enabled (headless
 * tasks were broken under bridgeless before 0.76, and patchy until 0.76.5).
 *
 * @param task - Runs on wake. Receives {@link MeshWakeTaskData}.
 */
export function registerMeshWakeTask(
  task: (data: MeshWakeTaskData) => Promise<void>
): void {
  if (Platform.OS !== 'android') {
    return;
  }
  AppRegistry.registerHeadlessTask(MESH_WAKE_TASK_KEY, () => task);
}

/**
 * Default export
 */
export default OfflineProtocol;
