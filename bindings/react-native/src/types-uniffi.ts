/**
 * TypeScript type definitions for UniFFI-based Offline Protocol SDK
 * These match the types defined in offline_protocol_simple.udl
 */

export enum MessagePriority {
  Low = 0,
  Medium = 1,
  High = 2,
  Critical = 3,
}

export interface ProtocolConfig {
  appId: string;
  userId: string;
  bleEnabled: boolean;
  wifiDirectEnabled: boolean;
  internetEnabled: boolean;
  preferOnline: boolean;
  initialTtl: number; // u8 in Rust
}

export interface OfflineProtocolModule {
  /**
   * Creates a new protocol instance with the given configuration
   */
  create(configJson: string): Promise<void>;
  
  /**
   * Starts the protocol
   */
  start(): Promise<void>;
  
  /**
   * Stops the protocol gracefully
   */
  stop(): Promise<void>;
  
  /**
   * Pauses the protocol (for background mode)
   */
  pause(): Promise<void>;
  
  /**
   * Resumes the protocol from pause
   */
  resume(): Promise<void>;
  
  /**
   * Sends a message
   * @returns The message ID
   */
  sendMessage(
    recipient: string,
    content: string,
    priority: MessagePriority
  ): Promise<string>;
  
  /**
   * Receives the next available message
   * @returns JSON string of the message, or null if no message available
   */
  receiveMessage(): Promise<string | null>;
  
  /**
   * Add event listener (required by React Native)
   */
  addListener(eventName: string): void;
  
  /**
   * Remove event listeners (required by React Native)
   */
  removeListeners(count: number): void;
}

/**
 * Protocol errors that can be thrown
 */
export class ProtocolError extends Error {
  constructor(
    message: string,
    public code: string,
    public nativeError?: Error
  ) {
    super(message);
    this.name = 'ProtocolError';
  }
}

/**
 * Error codes
 */
export enum ProtocolErrorCode {
  NOT_STARTED = 'NotStarted',
  ALREADY_STARTED = 'AlreadyStarted',
  INVALID_CONFIGURATION = 'InvalidConfiguration',
  SEND_FAILED = 'SendFailed',
  OTHER = 'Other',
}

/**
 * Helper to create properly formatted config JSON
 */
export function createConfigJson(config: ProtocolConfig): string {
  return JSON.stringify(config);
}

/**
 * Helper to parse message JSON
 */
export interface ReceivedMessage {
  id: string;
  sender: string;
  recipient: string;
  content: string;
  timestamp: number;
  hop_count: number;
  priority: MessagePriority;
}

export function parseMessage(messageJson: string): ReceivedMessage {
  return JSON.parse(messageJson);
}

