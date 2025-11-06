/**
 * Offline Protocol SDK for React Native
 *
 * @packageDocumentation
 */

import { NativeModules, NativeEventEmitter, Platform, EmitterSubscription } from 'react-native';
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
} from './types';
import { MessagePriority } from './types';

export * from './types';

const LINKING_ERROR =
  `The package '@offlineprotocol/react-native' doesn't seem to be linked. Make sure: \n\n` +
  Platform.select({ ios: "- You have run 'pod install'\n", default: '' }) +
  '- You rebuilt the app after installing the package\n' +
  '- You are not using Expo Go\n';

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

/**
 * Main Offline Protocol class
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
 * // Start protocol
 * await protocol.start();
 *
 * // Send message
 * const messageId = await protocol.sendMessage({
 *   recipient: 'user456',
 *   content: 'Hello!',
 *   priority: MessagePriority.High,
 * });
 *
 * // Stop protocol
 * await protocol.stop();
 * ```
 */
export class OfflineProtocol {
  private eventEmitter: NativeEventEmitter;
  private eventSubscription: EmitterSubscription | null = null;
  private eventListeners: Map<EventType | 'all', Set<EventListener>> = new Map();
  private config: ProtocolConfig;
  private isCreated: boolean = false;

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
   * @throws Error if protocol is already started or fails to start
   */
  async start(): Promise<void> {
    // Create protocol instance if not already created
    if (!this.isCreated) {
      await OfflineProtocolNativeModule.create(JSON.stringify(this.config));
      this.isCreated = true;
    }

    await OfflineProtocolNativeModule.start();
  }

  /**
   * Stops the protocol
   *
   * @throws Error if protocol is not started or fails to stop
   */
  async stop(): Promise<void> {
    await OfflineProtocolNativeModule.stop();
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
  async receiveMessage(): Promise<any | null> {
    return await OfflineProtocolNativeModule.receiveMessage();
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
  }
}

/**
 * Default export
 */
export default OfflineProtocol;
