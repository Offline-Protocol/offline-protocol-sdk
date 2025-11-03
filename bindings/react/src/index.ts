/**
 * Offline Protocol SDK for React (Web)
 * 
 * This package provides React bindings for the Offline Protocol SDK,
 * enabling offline-first messaging in web browsers using WebAssembly.
 * 
 * @example
 * ```typescript
 * import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react';
 * 
 * const protocol = new OfflineProtocol({
 *   appId: 'my-app',
 *   userId: 'user123',
 *   transport: {
 *     internetEnabled: true,  // Only Internet available in browsers
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

import initWasm, { OfflineProtocol as WasmProtocol, MessagePriority as WasmMessagePriority } from '@offlineprotocol/web';
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

// Track WASM initialization
let wasmInitialized = false;
let wasmInitPromise: Promise<void> | null = null;

/**
 * Initialize the WASM module (call once before using OfflineProtocol)
 */
async function initializeWasm(): Promise<void> {
  if (wasmInitialized) {
    return;
  }
  
  if (wasmInitPromise) {
    return wasmInitPromise;
  }
  
  wasmInitPromise = initWasm().then(() => {
    wasmInitialized = true;
  });
  
  return wasmInitPromise;
}

/**
 * Simple event emitter implementation
 */
class EventEmitter {
  private listeners: Map<string, Set<EventListener>> = new Map();

  on(event: string, listener: EventListener): void {
    const listeners = this.listeners.get(event) || new Set();
    listeners.add(listener);
    this.listeners.set(event, listeners);
  }

  off(event: string, listener: EventListener): void {
    const listeners = this.listeners.get(event);
    if (listeners) {
      listeners.delete(listener);
      if (listeners.size === 0) {
        this.listeners.delete(event);
      }
    }
  }

  emit(event: string, data: Event): void {
    const listeners = this.listeners.get(event);
    if (listeners) {
      listeners.forEach(listener => listener(data));
    }
  }

  removeAllListeners(event?: string): void {
    if (event) {
      this.listeners.delete(event);
    } else {
      this.listeners.clear();
    }
  }
}

/**
 * Convert our MessagePriority enum to WASM MessagePriority
 */
function toWasmPriority(priority: MessagePriority): WasmMessagePriority {
  switch (priority) {
    case MessagePriority.Low:
      return WasmMessagePriority.Low;
    case MessagePriority.Medium:
      return WasmMessagePriority.Medium;
    case MessagePriority.High:
      return WasmMessagePriority.High;
    case MessagePriority.Critical:
      return WasmMessagePriority.Critical;
    default:
      return WasmMessagePriority.Medium;
  }
}

/**
 * Main Offline Protocol class
 */
export class OfflineProtocol extends EventEmitter {
  private config: ProtocolConfig;
  private wasmProtocol: WasmProtocol | null = null;
  private isStarted: boolean = false;

  /**
   * Creates a new OfflineProtocol instance
   * 
   * @param config - Protocol configuration
   */
  constructor(config: ProtocolConfig) {
    super();
    this.config = {
      ...config,
      // Force Internet-only for web browsers
      transport: {
        ...config.transport,
        bleEnabled: false,
        wifiDirectEnabled: false,
        internetEnabled: config.transport?.internetEnabled ?? true,
      },
    };
  }

  /**
   * Initializes the WASM module and starts the protocol
   */
  async start(): Promise<void> {
    if (this.isStarted) {
      throw new Error('Protocol is already started');
    }

    // Initialize WASM if not already done
    await initializeWasm();

    // Create WASM protocol instance
    const configJson = JSON.stringify(this.config);
    this.wasmProtocol = new WasmProtocol(configJson);

    // Start the protocol
    this.wasmProtocol.start();
    this.isStarted = true;

    // Set up event polling (WASM doesn't have callbacks, so we poll)
    this.startEventPolling();
  }

  /**
   * Stops the protocol
   */
  async stop(): Promise<void> {
    if (!this.isStarted || !this.wasmProtocol) {
      return;
    }

    this.isStarted = false;
    this.stopEventPolling();
    
    try {
      this.wasmProtocol.stop();
    } catch (error) {
      console.error('Error stopping protocol:', error);
    }
    
    this.wasmProtocol = null;
    this.removeAllListeners();
  }

  /**
   * Pauses the protocol (reduces activity but keeps connection alive)
   * Note: In web browsers, this is a no-op since we only have Internet transport
   */
  async pause(): Promise<void> {
    // Web browsers only have Internet transport, so pausing doesn't make sense
    // This is kept for API compatibility with React Native binding
    console.warn('pause() is not applicable for web browsers (Internet-only transport)');
  }

  /**
   * Resumes the protocol from pause
   * Note: In web browsers, this is a no-op since we only have Internet transport
   */
  async resume(): Promise<void> {
    // Web browsers only have Internet transport, so resuming doesn't make sense
    // This is kept for API compatibility with React Native binding
    console.warn('resume() is not applicable for web browsers (Internet-only transport)');
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
    if (!this.isStarted || !this.wasmProtocol) {
      throw new Error('Protocol is not started. Call start() first.');
    }

    const { recipient, content, priority = MessagePriority.Medium } = params;
    const wasmPriority = toWasmPriority(priority);
    
    try {
      const messageId = this.wasmProtocol.sendMessage(recipient, content, wasmPriority);
      
      // Emit message sent event
      this.emit('message:sent', {
        type: 'message_sent',
        messageId,
        timestamp: Date.now(),
      });

      return messageId;
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      
      // Emit message failed event
      this.emit('message:failed', {
        type: 'message_failed',
        messageId: '',
        reason: errorMessage,
        retryCount: 0,
      });
      
      throw error;
    }
  }

  /**
   * Gets the current protocol state
   */
  getState(): string {
    if (!this.wasmProtocol) {
      return 'uninitialized';
    }
    return this.wasmProtocol.getState();
  }

  /**
   * Check if the protocol is started
   */
  get started(): boolean {
    return this.isStarted;
  }

  /**
   * Start polling for events from WASM
   * Note: Since WASM doesn't have native callbacks, we simulate events
   * In a real implementation, you would set up WASM callbacks or polling
   */
  private eventPollingInterval: number | null = null;

  private startEventPolling(): void {
    // In a real implementation, you would:
    // 1. Set up WASM callbacks using wasm-bindgen's Closure API
    // 2. Or poll the protocol state and emit events based on changes
    // For now, this is a placeholder that could be extended
    
    // Example: Poll every second (adjust as needed)
    // this.eventPollingInterval = window.setInterval(() => {
    //   if (this.wasmProtocol) {
    //     // Check for new messages, state changes, etc.
    //     // Emit events based on protocol state
    //   }
    // }, 1000);
  }

  private stopEventPolling(): void {
    if (this.eventPollingInterval !== null) {
      clearInterval(this.eventPollingInterval);
      this.eventPollingInterval = null;
    }
  }

  // Override EventEmitter methods with proper typing
  on(event: 'message:received', listener: EventListener<MessageReceivedEvent>): void;
  on(event: 'message:delivered', listener: EventListener<MessageDeliveredEvent>): void;
  on(event: 'message:failed', listener: EventListener<MessageFailedEvent>): void;
  on(event: 'transport:switched', listener: EventListener<TransportSwitchedEvent>): void;
  on(event: 'relay:promoted', listener: EventListener<RelayPromotedEvent>): void;
  on(event: 'relay:demoted', listener: EventListener<RelayDemotedEvent>): void;
  on(event: 'file:progress', listener: EventListener<FileProgressEvent>): void;
  on(event: 'file:received', listener: EventListener<FileReceivedEvent>): void;
  on(event: string, listener: EventListener): void {
    super.on(event, listener);
  }
}

// Re-export types
export * from './types';

// Re-export hooks
export * from './hooks';

