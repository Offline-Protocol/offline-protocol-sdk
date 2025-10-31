/**
 * Offline Protocol SDK for React Native
 * 
 * This package provides React Native bindings for the Offline Protocol SDK,
 * enabling offline-first messaging with automatic transport switching between
 * Internet, BLE Mesh, and Wi-Fi Direct.
 * 
 * @example
 * ```typescript
 * import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react-native';
 * 
 * const protocol = new OfflineProtocol({
 *   appId: 'my-app',
 *   userId: 'user123',
 *   transport: {
 *     bleEnabled: true,
 *     wifiDirectEnabled: true,
 *     internetEnabled: true,
 *   }
 * });
 * 
 * // Start the protocol
 * await protocol.start();
 * 
 * // Send a message
 * const messageId = await protocol.sendMessage({
 *   recipient: 'user456',
 *   content: 'Hello!',
 *   priority: MessagePriority.High,
 * });
 * 
 * // Listen for events
 * protocol.on('message:received', (event) => {
 *   console.log('Received message:', event.content);
 * });
 * ```
 */

import { NativeModules, NativeEventEmitter, Platform } from 'react-native';
import type {
  ProtocolConfig,
  MessagePriority,
  Event,
  EventListener,
  MessageReceivedEvent,
  MessageDeliveredEvent,
  MessageFailedEvent,
  TransportSwitchedEvent,
  RelayPromotedEvent,
  RelayDemotedEvent,
  FileProgressEvent,
  FileReceivedEvent,
} from './types';

const LINKING_ERROR =
  `The package '@offlineprotocol/react-native' doesn't seem to be linked. Make sure: \n\n` +
  Platform.select({ ios: "- You have run 'pod install'\n", default: '' }) +
  '- You rebuilt the app after installing the package\n' +
  '- You are not using Expo Go\n';

const OfflineProtocolNative = NativeModules.OfflineProtocol
  ? NativeModules.OfflineProtocol
  : new Proxy(
      {},
      {
        get() {
          throw new Error(LINKING_ERROR);
        },
      }
    );

const eventEmitter = new NativeEventEmitter(OfflineProtocolNative);

/**
 * Main Offline Protocol class
 */
export class OfflineProtocol {
  private config: ProtocolConfig;
  private eventListeners: Map<string, Set<EventListener>> = new Map();
  private nativeSubscriptions: any[] = [];

  /**
   * Creates a new OfflineProtocol instance
   * 
   * @param config - Protocol configuration
   */
  constructor(config: ProtocolConfig) {
    this.config = config;
    this.setupNativeEventListeners();
  }

  /**
   * Starts the protocol
   */
  async start(): Promise<void> {
    const configJson = JSON.stringify(this.config);
    await OfflineProtocolNative.start(configJson);
  }

  /**
   * Stops the protocol
   */
  async stop(): Promise<void> {
    await OfflineProtocolNative.stop();
    this.cleanup();
  }

  /**
   * Pauses the protocol (for background mode)
   */
  async pause(): Promise<void> {
    await OfflineProtocolNative.pause();
  }

  /**
   * Resumes the protocol from pause
   */
  async resume(): Promise<void> {
    await OfflineProtocolNative.resume();
  }

  /**
   * Sends a message
   * 
   * @param params - Message parameters
   * @returns Promise resolving to the message ID
   */
  async sendMessage(params: {
    recipient: string;
    content: string;
    priority?: MessagePriority;
  }): Promise<string> {
    const { recipient, content, priority = MessagePriority.Medium } = params;
    return await OfflineProtocolNative.sendMessage(recipient, content, priority);
  }

  /**
   * Sends a file
   * 
   * @param params - File parameters
   * @returns Promise resolving to the file ID
   */
  async sendFile(params: {
    recipient: string;
    filePath: string;
    priority?: MessagePriority;
  }): Promise<string> {
    const { recipient, filePath, priority = MessagePriority.Medium } = params;
    return await OfflineProtocolNative.sendFile(recipient, filePath, priority);
  }

  /**
   * Registers an event listener
   * 
   * @param event - Event name
   * @param listener - Event handler function
   */
  on(event: 'message:received', listener: EventListener<MessageReceivedEvent>): void;
  on(event: 'message:delivered', listener: EventListener<MessageDeliveredEvent>): void;
  on(event: 'message:failed', listener: EventListener<MessageFailedEvent>): void;
  on(event: 'transport:switched', listener: EventListener<TransportSwitchedEvent>): void;
  on(event: 'relay:promoted', listener: EventListener<RelayPromotedEvent>): void;
  on(event: 'relay:demoted', listener: EventListener<RelayDemotedEvent>): void;
  on(event: 'file:progress', listener: EventListener<FileProgressEvent>): void;
  on(event: 'file:received', listener: EventListener<FileReceivedEvent>): void;
  on(event: string, listener: EventListener): void {
    const listeners = this.eventListeners.get(event) || new Set();
    listeners.add(listener);
    this.eventListeners.set(event, listeners);
  }

  /**
   * Removes an event listener
   * 
   * @param event - Event name
   * @param listener - Event handler to remove
   */
  off(event: string, listener: EventListener): void {
    const listeners = this.eventListeners.get(event);
    if (listeners) {
      listeners.delete(listener);
      if (listeners.size === 0) {
        this.eventListeners.delete(event);
      }
    }
  }

  /**
   * Sets up native event listeners
   */
  private setupNativeEventListeners(): void {
    // Listen for all events from native side
    const subscription = eventEmitter.addListener('OfflineProtocolEvent', (event: Event) => {
      this.handleNativeEvent(event);
    });
    
    this.nativeSubscriptions.push(subscription);
  }

  /**
   * Handles events from the native side
   */
  private handleNativeEvent(event: Event): void {
    // Map event types to listener names
    const eventTypeMap: Record<string, string> = {
      'message_sent': 'message:sent',
      'message_received': 'message:received',
      'message_delivered': 'message:delivered',
      'message_failed': 'message:failed',
      'transport_switched': 'transport:switched',
      'relay_promoted': 'relay:promoted',
      'relay_demoted': 'relay:demoted',
      'neighbor_discovered': 'neighbor:discovered',
      'neighbor_lost': 'neighbor:lost',
      'network_metrics': 'network:metrics',
      'file_progress': 'file:progress',
      'file_received': 'file:received',
    };

    const listenerName = eventTypeMap[event.type];
    if (listenerName) {
      const listeners = this.eventListeners.get(listenerName);
      if (listeners) {
        listeners.forEach(listener => listener(event));
      }
    }
  }

  /**
   * Cleans up resources
   */
  private cleanup(): void {
    this.nativeSubscriptions.forEach(sub => sub.remove());
    this.nativeSubscriptions = [];
    this.eventListeners.clear();
  }
}

// Re-export types
export * from './types';

