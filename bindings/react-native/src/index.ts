/**
 * Offline Protocol SDK for React Native
 *
 * @packageDocumentation
 */

import { NativeModules, NativeEventEmitter, EmitterSubscription } from 'react-native';
import type {
  ProtocolConfig,
  SendMessageParams,
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
} from './types';
import { MessagePriority } from './types';
import { LINKING_ERROR } from './constants';

export * from './types';
export * from './constants';

const OfflineProtocolNativeModule = NativeModules.OfflineProtocolModule
  ? NativeModules.OfflineProtocolModule
  : new Proxy(
      {},
      {
        get() {
          throw new Error(LINKING_ERROR);
        },
      }
    );

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
  dors?: {
    preferOnline: boolean;
    switchHysteresis: number;
    switchCooldownSecs: number;
    bleToWifiRetryThreshold: number;
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
  private eventListeners: Map<EventType | 'all', Set<EventListener>> = new Map();
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

    this.initialRuntimeConfig =
      dorsConfig || relayConfig ? { dors: dorsConfig, relay: relayConfig } : null;
    this.initialRuntimeConfigApplied = false;

    const nativeConfig: NativeConfig = {
      appId: this.config.appId,
      userId: this.config.userId,
      bleEnabled: this.config.transports?.ble?.enabled ?? true,
      wifiDirectEnabled: this.config.transports?.wifiDirect?.enabled ?? false,
      internetEnabled: this.config.transports?.internet?.enabled ?? false,
      preferOnline: dorsSource?.preferOnline ?? false,
      initialTtl: this.config.network?.initialTtl ?? 8,
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

    if (this.config.reliability) {
      const reliabilityConfig = sanitize({
        ack: sanitize(this.config.reliability.ack ?? {}),
        retry: sanitize(this.config.reliability.retry ?? {}),
        dedup: sanitize(this.config.reliability.dedup ?? {}),
      });
      if (reliabilityConfig) {
        nativeConfig.reliability = reliabilityConfig;
      }
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

    console.log('[OfflineProtocol] Native config:', JSON.stringify(nativeConfig));
    return nativeConfig;
  }

  private normalizeRelayPriority(priority?: string | null): NativeRelayPriority | null {
    if (!priority) {
      return null;
    }
    const normalized = priority.toLowerCase();
    switch (normalized) {
      case 'low':
      case 'medium':
      case 'high':
        return normalized as NativeRelayPriority;
      case 'never':
        return 'low';
      case 'always':
        return 'high';
      case 'auto':
        return 'medium';
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

    const { dors, relay } = this.initialRuntimeConfig;

    if (dors) {
      try {
        await OfflineProtocolNativeModule.updateDorsConfig(JSON.stringify(dors));
      } catch (error) {
        console.warn('[OfflineProtocol] Failed to apply DORS configuration', error);
      }
    }

    if (relay?.relayPriority) {
      const normalizedPriority = this.normalizeRelayPriority(relay.relayPriority);
      if (normalizedPriority) {
        try {
          await OfflineProtocolNativeModule.setRelayPriority(normalizedPriority);
        } catch (error) {
          console.warn('[OfflineProtocol] Failed to apply relay priority', error);
        }
      }
    }

    this.initialRuntimeConfigApplied = true;
  }

  /**
   * Sets up the native event subscription
   */
  private setupEventSubscription(): void {
    this.eventSubscription = this.eventEmitter.addListener(
      'OfflineProtocol_Event',
      (data: { eventJson: string }) => {
        try {
          const event = JSON.parse(data.eventJson) as ProtocolEvent;
          this.emitEvent(event);
        } catch (error) {
          console.error('Failed to parse event JSON:', error);
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
    const allListeners = this.eventListeners.get('all');
    if (allListeners) {
      allListeners.forEach((listener) => {
        try {
          listener(event);
        } catch (error) {
          console.error('Error in event listener for all events:', error);
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
    eventType: EventType | 'all',
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
    eventType: EventType | 'all',
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
    eventType: EventType | 'all',
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
  removeAllListeners(eventType?: EventType | 'all'): this {
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

    await OfflineProtocolNativeModule.start();

    // Auto-enable internet transport if configured with a server address
    const internetConfig = this.config.transports?.internet;
    if (internetConfig?.enabled && internetConfig?.serverAddress) {
      try {
        await this.enableTransport('internet', {
          enabled: true,
          serverAddress: internetConfig.serverAddress,
          autoReconnect: internetConfig.autoReconnect ?? true,
        });
        console.log('[OfflineProtocol] Internet transport auto-enabled');
      } catch (error) {
        console.warn('[OfflineProtocol] Failed to auto-enable internet transport:', error);
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
      priority
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
    const fileName = params.fileName || params.filePath.split('/').pop() || 'file';
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
  async setRelayPriority(priority: 'low' | 'medium' | 'high'): Promise<void> {
    return await OfflineProtocolNativeModule.setRelayPriority(priority);
  }
  
  /**
   * Gets the current relay priority
   *
   * @returns Relay priority
   */
  async getRelayPriority(): Promise<'low' | 'medium' | 'high'> {
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
      payload.congestionDurationSecs = Math.max(0, payload.congestionDurationSecs);
    }
    if (payload.ttlEscalationHoldSecs !== undefined) {
      payload.ttlEscalationHoldSecs = Math.max(1, payload.ttlEscalationHoldSecs);
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
    return await OfflineProtocolNativeModule.updateDorsConfig(JSON.stringify(payload));
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
    return await OfflineProtocolNativeModule.updateAckConfig(JSON.stringify(config));
  }
  
  /**
   * Updates retry configuration at runtime
   *
   * @param config - Retry configuration
   * @throws Error if update fails
   */
  async updateRetryConfig(config: RetryConfig): Promise<void> {
    return await OfflineProtocolNativeModule.updateRetryConfig(JSON.stringify(config));
  }
  
  /**
   * Updates deduplication configuration at runtime
   *
   * @param config - Deduplication configuration
   * @throws Error if update fails
   */
  async updateDedupConfig(config: DedupConfig): Promise<void> {
    return await OfflineProtocolNativeModule.updateDedupConfig(JSON.stringify(config));
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
   */
  async learnRoute(
    destination: string,
    nextHop: string,
    hopCount: number,
    quality: number
  ): Promise<void> {
    return await OfflineProtocolNativeModule.learnRoute(
      destination,
      nextHop,
      hopCount,
      quality
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
   * @returns Message to send or null if queue is empty
   */
  async internetGetNextMessage(): Promise<{
    recipientId: string;
    data: number[];
  } | null> {
    return await OfflineProtocolNativeModule.internetGetNextMessage();
  }

  /**
   * Marks the last internet message as sent.
   */
  async internetReturnMessage(): Promise<void> {
    return await OfflineProtocolNativeModule.internetReturnMessage();
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
