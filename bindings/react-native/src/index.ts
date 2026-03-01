/**
 * Offline Protocol SDK for React Native
 *
 * @packageDocumentation
 */

import { NativeModules, NativeEventEmitter, EmitterSubscription } from 'react-native';
import type {
  ProtocolConfig,
  SendMessageParams,
  SendConnectionRequestParams,
  AcceptConnectionRequestParams,
  RejectConnectionRequestParams,
  SendFileParams,
  ProtocolEvent,
  EventListener,
  EventType,
  NetworkTopology,
  MessageDeliveryStats,
  TransportType,
  InternetTransportConfig,
  WifiDirectTransportConfig,
  FileProgress,
  ProtocolState,
  MessageReceivedEvent,
  BleTransportConfig,
  AckConfig,
  RetryConfig,
  DedupConfig,
  DedupStats,
  MlsKeyPackage,
  MlsEncryptedMessage,
  MlsWelcome,
  MlsSessionInfo,
  MlsGroupInfo,
  EstablishmentState,
} from './types';
import { MessagePriority } from './types';
import { LINKING_ERROR } from './constants';

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

type NativeRelayPriority = 'low' | 'medium' | 'high';

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
    relayThreshold?: number;
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
  userId: string;
  bleEnabled: boolean;
  wifiDirectEnabled: boolean;
  internetEnabled: boolean;
  preferOnline: boolean;
  initialTtl: number;
  encryptionEnabled?: boolean;
  autoKeyExchange?: boolean;
  storePending?: boolean;
  requireEncryption?: boolean;
  maxPendingPerPeer?: number;
  maxPendingGlobal?: number;
  pendingTtlMs?: number;
  overflowPolicy?: 'drop_oldest' | 'drop_newest';
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
    relayThreshold?: number;
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
  path?: {
    forwardToTopK?: number;
    maxCongestionLevel?: number;
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
 * - WiFi Direct - Future support
 * - Internet - Future support
 *
 * @example
 * ```typescript
 * const protocol = new OfflineProtocol({
 *   appId: 'my-app',
 *   userId: 'user123',
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
 * // Send message (routing handled automatically by DORS)
 * const messageId = await protocol.sendMessage({
 *   recipient: 'user456',
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
  private eventListeners: Map<EventType | "all", Set<EventListener>> =
    new Map();
  private config: ProtocolConfig;
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
        })
      : undefined;

    const relayConfig = relaySource
      ? sanitize({
          allowRelay: relaySource.allowRelay,
          minBatteryForRelay: relaySource.minBatteryForRelay,
          relayThreshold: relaySource.relayThreshold,
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

    const nativeConfig: NativeConfig = {
      appId: this.config.appId,
      userId: this.config.userId,
      bleEnabled: this.config.transports?.ble?.enabled ?? true,
      wifiDirectEnabled: this.config.transports?.wifiDirect?.enabled ?? false,
      internetEnabled: this.config.transports?.internet?.enabled ?? false,
      preferOnline: dorsSource?.preferOnline ?? false,
      initialTtl: this.config.network?.initialTtl ?? 8,
      encryptionEnabled: this.config.encryption?.enabled ?? true,
      autoKeyExchange: this.config.encryption?.autoKeyExchange ?? true,
      storePending: this.config.encryption?.storePending ?? true,
      requireEncryption: this.config.encryption?.requireEncryption ?? false,
      maxPendingPerPeer: this.config.encryption?.pendingQueue?.maxPendingPerPeer ?? 64,
      maxPendingGlobal: this.config.encryption?.pendingQueue?.maxPendingGlobal ?? 4096,
      pendingTtlMs: this.config.encryption?.pendingQueue?.pendingTtlMs ?? 120000,
      overflowPolicy: this.config.encryption?.pendingQueue?.overflowPolicy ?? 'drop_oldest',
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

    if (reliabilityConfig) {
      nativeConfig.reliability = reliabilityConfig;
    }

    if (this.config.path) {
      const pathConfig = sanitize({
        forwardToTopK: this.config.path.forwardToTopK,
        maxCongestionLevel: this.config.path.maxCongestionLevel,
      });
      if (pathConfig) {
        nativeConfig.path = pathConfig;
      }
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
      case "low":
      case "medium":
      case "high":
        return normalized as NativeRelayPriority;
      case "never":
        return "low";
      case "always":
        return "high";
      case "auto":
        return "medium";
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

    if (relay?.relayPriority) {
      const normalizedPriority = this.normalizeRelayPriority(
        relay.relayPriority
      );
      if (normalizedPriority) {
        try {
          await OfflineProtocolNativeModule.setRelayPriority(
            normalizedPriority
          );
        } catch (error) {
          console.warn(
            "[OfflineProtocol] Failed to apply relay priority",
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
   * Emits an event to all registered listeners
   */
  private emitEvent(event: ProtocolEvent): void {
    // Call event-specific listeners
    const specificListeners = this.eventListeners.get(event.type);
    if (specificListeners) {
      specificListeners.forEach((listener) => {
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
        try {
          listener(event);
        } catch (error) {
          console.error("Error in event listener for all events:", error);
        }
      });
    }
  }

  /**
   * Registers an event listener
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
    return this;
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
      } catch (error) {
        console.warn(
          "[OfflineProtocol] MLS initialization failed — secure sessions and handshake will not work:",
          error
        );
      }
    }

    await OfflineProtocolNativeModule.start();

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
    const messageId = await OfflineProtocolNativeModule.sendMessage(
      params.recipient,
      params.content,
      priority,
      params.replyToMsg ?? null
    );
    return messageId;
  }

  /**
   * Sends a connection request
   *
   * @param params - Connection request parameters
   * @returns Message ID
   * @throws Error if request fails to send
   */
  async sendConnectionRequest(params: SendConnectionRequestParams): Promise<string> {
    const messageId = await OfflineProtocolNativeModule.sendConnectionRequest(
      params.recipient,
      params.senderName,
      params.keyPackage ?? null
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
   * @param type - Transport type to enable
   * @param config - Optional transport configuration
   * @throws Error if transport fails to enable
   */
  async enableTransport(
    type: TransportType,
    config?: InternetTransportConfig | WifiDirectTransportConfig
  ): Promise<void> {
    return await OfflineProtocolNativeModule.enableTransport(type, config);
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
   * Sends a file to a recipient
   *
   * @param params - File sending parameters
   * @returns File ID for tracking progress
   * @throws Error if file fails to send
   */
  async sendFile(params: SendFileParams): Promise<string> {
    const fileName =
      params.fileName || params.filePath.split("/").pop() || "file";
    const fileId = await OfflineProtocolNativeModule.sendFile(
      params.filePath,
      params.recipient,
      fileName
    );
    return fileId;
  }

  /**
   * Gets the progress of a file transfer
   *
   * @param fileId - File identifier
   * @returns File progress information, or null if not found
   * @throws Error if retrieval fails
   */
  async getFileProgress(fileId: string): Promise<FileProgress | null> {
    return await OfflineProtocolNativeModule.getFileProgress(fileId);
  }

  /**
   * Cancels an active file transfer
   *
   * @param fileId - File identifier
   * @returns True if cancelled, false if not found
   * @throws Error if cancellation fails
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
   * Sets the battery level for relay decisions
   *
   * @param level - Battery level (0-100)
   */
  async setBatteryLevel(level: number): Promise<void> {
    return await OfflineProtocolNativeModule.setBatteryLevel(level);
  }

  /**
   * Gets the current battery level
   *
   * @returns Battery level (0-100) or null if not set
   */
  async getBatteryLevel(): Promise<number | null> {
    return await OfflineProtocolNativeModule.getBatteryLevel();
  }

  /**
   * Sets the relay priority
   *
   * @param priority - Relay priority ('low', 'medium', or 'high')
   * @throws Error if setting fails
   */
  async setRelayPriority(priority: "low" | "medium" | "high"): Promise<void> {
    return await OfflineProtocolNativeModule.setRelayPriority(priority);
  }

  /**
   * Gets the current relay priority
   *
   * @returns Relay priority
   */
  async getRelayPriority(): Promise<"low" | "medium" | "high"> {
    return await OfflineProtocolNativeModule.getRelayPriority();
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
   * Gets detailed metrics for a specific transport
   *
   * @param transportType - Transport type
   * @returns Transport metrics or null if not available
   */
  async getTransportMetrics(transportType: TransportType): Promise<{
    packetsSent: number;
    packetsReceived: number;
    bytesSent: number;
    bytesReceived: number;
    errorRate: number;
    avgLatencyMs: number;
  } | null> {
    return await OfflineProtocolNativeModule.getTransportMetrics(transportType);
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
   * Updates DORS configuration at runtime
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
  // GRADIENT ROUTING
  // ============================================================================

  /**
   * Learns a route from an incoming message.
   * Call this when receiving a message from a neighbor to record that
   * the neighbor can reach the message's original sender.
   *
   * @param destination - Destination user ID
   * @param nextHop - Neighbor ID that delivered the message
   * @param hopCount - Number of hops to reach destination
   * @param quality - Route quality score (0.0 - 1.0)
   * @param sequenceNumber - DSDV-style sequence number; pass 0 when the message does not carry one
   */
  async learnRoute(
    destination: string,
    nextHop: string,
    hopCount: number,
    quality: number,
    sequenceNumber: number = 0
  ): Promise<void> {
    // Clamp to match Android (coerceAtLeast(0)); avoids negative wrapping to uint32 (e.g. -1 → 2^32-1).
    const clampedSequenceNumber = Math.max(0, sequenceNumber);
    return await OfflineProtocolNativeModule.learnRoute(
      destination,
      nextHop,
      hopCount,
      quality,
      clampedSequenceNumber
    );
  }

  /**
   * Gets the best (highest quality) route to a destination.
   *
   * @param destination - Destination user ID
   * @returns Best route entry or null if no route exists
   */
  async getBestRoute(destination: string): Promise<{
    nextHop: string;
    hopCount: number;
    quality: number;
    lastSeenMs: number;
  } | null> {
    return await OfflineProtocolNativeModule.getBestRoute(destination);
  }

  /**
   * Gets all valid (non-expired) routes to a destination.
   *
   * @param destination - Destination user ID
   * @returns Array of route entries
   */
  async getAllRoutes(destination: string): Promise<
    Array<{
      nextHop: string;
      hopCount: number;
      quality: number;
      lastSeenMs: number;
    }>
  > {
    return await OfflineProtocolNativeModule.getAllRoutes(destination);
  }

  /**
   * Checks if a route exists to the destination.
   *
   * @param destination - Destination user ID
   * @returns True if at least one route exists
   */
  async hasRoute(destination: string): Promise<boolean> {
    return await OfflineProtocolNativeModule.hasRoute(destination);
  }

  /**
   * Removes all routes through a neighbor.
   * Call this when a neighbor disconnects to clean up stale routes.
   *
   * @param neighborId - Neighbor ID to remove routes for
   */
  async removeNeighborRoutes(neighborId: string): Promise<void> {
    return await OfflineProtocolNativeModule.removeNeighborRoutes(neighborId);
  }

  /**
   * Cleans up expired routes.
   * Call this periodically (e.g., every 30 seconds) for maintenance.
   */
  async cleanupExpiredRoutes(): Promise<void> {
    return await OfflineProtocolNativeModule.cleanupExpiredRoutes();
  }

  /**
   * Gets routing table statistics for monitoring.
   *
   * @returns Routing statistics
   */
  async getRoutingStats(): Promise<{
    destinationCount: number;
    routeCount: number;
  }> {
    return await OfflineProtocolNativeModule.getRoutingStats();
  }

  /**
   * Updates the gradient routing configuration.
   *
   * @param config - Routing configuration
   */
  async updateRoutingConfig(config: {
    maxRoutesPerDestination?: number;
    routeTtlSecs?: number;
    maxRoutingTableSize?: number;
  }): Promise<void> {
    return await OfflineProtocolNativeModule.updateRoutingConfig(
      JSON.stringify(config)
    );
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
   * @param data - Chunk data as array of bytes
   */
  async processFileChunk(
    fileId: string,
    chunkIndex: number,
    data: number[]
  ): Promise<void> {
    return await OfflineProtocolNativeModule.processFileChunk(
      fileId,
      chunkIndex,
      data
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
   * @param senderId - Sender peer ID
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
   * @param peerId - Peer ID
   */
  async wifiDirectPeerConnected(peerId: string): Promise<void> {
    return await OfflineProtocolNativeModule.wifiDirectPeerConnected(peerId);
  }

  /**
   * Notifies the protocol that a WiFi Direct peer has disconnected.
   *
   * @param peerId - Peer ID
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
   * Derives a deterministic user ID from a public key.
   * Returns a base58-encoded string derived from SHA-256(publicKey)[0:20].
   * The same public key always produces the same user ID.
   *
   * This allows binding a user's identity to their cryptographic keys,
   * ensuring that user IDs cannot be forged.
   *
   * @param publicKey - The public key bytes (32 bytes for Ed25519)
   * @returns The derived user ID string
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
    const otherUserId =
      members.find(memberId => memberId !== this.config.userId) ?? '';
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

  /**
   * Creates a new MLS group.
   *
   * @param groupName - Human-readable group name
   * @returns Group info
   * @throws Error if creation fails
   */
  async mlsCreateGroup(groupName: string): Promise<MlsGroupInfo> {
    const result = await OfflineProtocolNativeModule.mlsCreateGroup(groupName, []);
    return {
      groupId: result.groupId,
      groupName: result.groupName ?? result.name ?? '',
      memberIds: result.memberIds ?? result.members ?? [],
      epoch: result.epoch,
      createdAt: result.createdAt ?? result.createdAtMs ?? 0,
    };
  }

  /**
   * Adds a member to an MLS group.
   *
   * @param groupId - Group ID
   * @param memberKeyPackage - New member's key package data
   * @returns Welcome message to send to the new member
   * @throws Error if addition fails
   */
  async mlsAddGroupMember(
    groupId: string,
    memberKeyPackage: number[]
  ): Promise<MlsWelcome> {
    const result = await OfflineProtocolNativeModule.mlsAddGroupMember(
      groupId,
      memberKeyPackage
    );
    return {
      groupId: result.groupId,
      welcomeData: result.welcomeData,
      inviterId: result.inviterId,
      timestampMs: result.timestampMs,
    };
  }

  /**
   * Removes a member from an MLS group.
   *
   * @param groupId - Group ID
   * @param memberId - Member to remove
   * @throws Error if removal fails
   */
  async mlsRemoveGroupMember(groupId: string, memberId: string): Promise<void> {
    await OfflineProtocolNativeModule.mlsRemoveGroupMember(groupId, memberId);
  }

  /**
   * Leaves an MLS group.
   *
   * @param groupId - Group ID to leave
   * @throws Error if leaving fails
   */
  async mlsLeaveGroup(groupId: string): Promise<void> {
    return await OfflineProtocolNativeModule.mlsLeaveGroup(groupId);
  }

  /**
   * Encrypts a message for an MLS group.
   *
   * @param groupId - Group ID
   * @param plaintext - Message content as bytes
   * @returns Encrypted message
   * @throws Error if encryption fails
   */
  async mlsEncryptForGroup(
    groupId: string,
    plaintext: number[]
  ): Promise<MlsEncryptedMessage> {
    const result = await OfflineProtocolNativeModule.mlsEncryptForGroup(
      groupId,
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
   * Decrypts a message from an MLS group.
   *
   * @param encrypted - Encrypted message
   * @returns Decrypted plaintext as bytes, or null if decryption fails
   */
  async mlsDecryptFromGroup(
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
    return await OfflineProtocolNativeModule.mlsDecryptFromGroup(encryptedJson);
  }

  /**
   * Joins an MLS group from a Welcome message.
   *
   * @param welcome - Welcome message from group admin
   * @returns Group info
   * @throws Error if joining fails
   */
  async mlsJoinGroup(welcome: MlsWelcome): Promise<MlsGroupInfo> {
    const welcomeJson = JSON.stringify({
      groupId: welcome.groupId,
      welcomeData: welcome.welcomeData,
      inviterId: welcome.inviterId,
      timestampMs: welcome.timestampMs,
    });
    const result = await OfflineProtocolNativeModule.mlsJoinGroup(welcomeJson);
    return {
      groupId: result.groupId,
      groupName: result.groupName,
      memberIds: result.memberIds,
      epoch: result.epoch,
      createdAt: result.createdAt,
    };
  }

  /**
   * Lists all MLS groups.
   *
   * @returns Array of group IDs
   */
  async mlsListGroups(): Promise<string[]> {
    return await OfflineProtocolNativeModule.mlsListGroups();
  }

  /**
   * Gets information about an MLS group.
   *
   * @param groupId - Group ID
   * @returns Group info or null if not found
   */
  async mlsGetGroupInfo(groupId: string): Promise<MlsGroupInfo | null> {
    const result = await OfflineProtocolNativeModule.mlsGetGroupInfo(groupId);
    if (!result) return null;
    return {
      groupId: result.groupId,
      groupName: result.groupName,
      memberIds: result.memberIds,
      epoch: result.epoch,
      createdAt: result.createdAt,
    };
  }

  /**
   * ========================================================================
   * GROUP MANAGEMENT (RELAY SERVER API)
   * ========================================================================
   */

  /**
   * Create a new group. Creator becomes admin.
   * Returns JSON string to send via WebSocket relay.
   */
  async groupCreate(name: string): Promise<string> {
    return await OfflineProtocolNativeModule.groupCreate(name);
  }

  /**
   * Send encrypted message to a group. Content must be pre-encrypted by client.
   * Returns JSON string to send via WebSocket relay.
   */
  async groupSendMessage(
    groupId: string,
    content: string,
    replyToMsg?: string | null
  ): Promise<string> {
    return await OfflineProtocolNativeModule.groupSendMessage(
      groupId,
      content,
      replyToMsg || null
    );
  }

  /**
   * Add member to group. Admin only.
   * Returns JSON string to send via WebSocket relay.
   */
  async groupAddMember(groupId: string, username: string): Promise<string> {
    return await OfflineProtocolNativeModule.groupAddMember(groupId, username);
  }

  /**
   * Remove member from group. Admin only, or user can remove themselves.
   * Returns JSON string to send via WebSocket relay.
   */
  async groupRemoveMember(groupId: string, username: string): Promise<string> {
    return await OfflineProtocolNativeModule.groupRemoveMember(
      groupId,
      username
    );
  }

  /**
   * Set member as admin. Admin only.
   * Returns JSON string to send via WebSocket relay.
   */
  async groupSetAdmin(groupId: string, username: string): Promise<string> {
    return await OfflineProtocolNativeModule.groupSetAdmin(groupId, username);
  }

  /**
   * Remove admin role from member. Admin only.
   * Returns JSON string to send via WebSocket relay.
   */
  async groupRemoveAdmin(groupId: string, username: string): Promise<string> {
    return await OfflineProtocolNativeModule.groupRemoveAdmin(
      groupId,
      username
    );
  }

  /**
   * Leave a group.
   * Returns JSON string to send via WebSocket relay.
   */
  async groupLeave(groupId: string): Promise<string> {
    return await OfflineProtocolNativeModule.groupLeave(groupId);
  }

  /**
   * Delete a group. Admin only.
   * Returns JSON string to send via WebSocket relay.
   */
  async groupDelete(groupId: string): Promise<string> {
    return await OfflineProtocolNativeModule.groupDelete(groupId);
  }

  /**
   * Get group information.
   * Returns JSON string to send via WebSocket relay.
   */
  async groupGetInfo(groupId: string): Promise<string> {
    return await OfflineProtocolNativeModule.groupGetInfo(groupId);
  }

  /**
   * Get all groups the user is a member of.
   * Returns JSON string to send via WebSocket relay.
   */
  async groupGetUserGroups(): Promise<string> {
    return await OfflineProtocolNativeModule.groupGetUserGroups();
  }

  /**
   * Destroys the protocol instance and cleans up resources
   */
  async destroy(): Promise<void> {
    // Remove all event listeners
    this.removeAllListeners();

    // Remove native event subscription
    if (this.eventSubscription) {
      this.eventSubscription.remove();
      this.eventSubscription = null;
    }

    // Destroy native protocol instance
    if (this.isCreated) {
      await OfflineProtocolNativeModule.destroy();
      this.isCreated = false;
    }

    this.initialRuntimeConfigApplied = false;
  }
}

/**
 * Default export
 */
export default OfflineProtocol;
