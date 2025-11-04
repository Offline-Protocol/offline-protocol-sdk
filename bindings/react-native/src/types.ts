/**
 * Type definitions for the Offline Protocol SDK React Native bindings.
 */

/**
 * Transport configuration options.
 */
export interface TransportConfig {
  /** Whether BLE transport is enabled. */
  bleEnabled: boolean;
  /** Whether Wi-Fi Direct transport is enabled (Android only). */
  wifiDirectEnabled: boolean;
  /** Whether Internet transport is enabled. */
  internetEnabled: boolean;
}

/**
 * Main protocol configuration.
 */
export interface ProtocolConfig {
  /** Application identifier (required). */
  appId: string;
  /** User identifier (required). */
  userId: string;
  /** Transport configuration. */
  transport?: TransportConfig;
  /** Initial TTL (Time-To-Live) for messages. Default: 8 */
  initialTtl?: number;
}

/**
 * Message priority levels.
 */
export enum MessagePriority {
  /** Low priority - can be delayed or dropped under congestion. */
  Low = 0,
  /** Medium priority - default for most messages. */
  Medium = 1,
  /** High priority - important messages that should be delivered quickly. */
  High = 2,
  /** Critical priority - emergency messages, highest delivery guarantee. */
  Critical = 3,
}

/**
 * Base event interface.
 */
export interface Event {
  /** Event type identifier. */
  type: string;
  /** Timestamp when the event occurred (milliseconds since epoch). */
  timestamp?: number;
}

/**
 * Message received event.
 */
export interface MessageReceivedEvent extends Event {
  type: 'message:received';
  /** ID of the received message. */
  messageId: string;
  /** Sender's user ID. */
  sender: string;
  /** Recipient's user ID. */
  recipient: string;
  /** Message content. */
  content: string;
  /** Number of hops the message traversed. */
  hopCount: number;
  /** Transport used for final delivery. */
  transport: string;
  /** When the message was received. */
  timestamp: number;
}

/**
 * Message delivered event (ACK received).
 */
export interface MessageDeliveredEvent extends Event {
  type: 'message:delivered';
  /** ID of the delivered message. */
  messageId: string;
  /** Latency in milliseconds. */
  latencyMs: number;
  /** Number of hops traversed. */
  hopCount: number;
  /** Transport used. */
  transport: string;
}

/**
 * Message failed event.
 */
export interface MessageFailedEvent extends Event {
  type: 'message:failed';
  /** ID of the failed message. */
  messageId: string;
  /** Reason for failure. */
  reason: string;
  /** Number of retries attempted. */
  retryCount: number;
}

/**
 * Transport switched event.
 */
export interface TransportSwitchedEvent extends Event {
  type: 'transport:switched';
  /** Previous transport (if any). */
  from?: string;
  /** New transport. */
  to: string;
  /** Reason for switch. */
  reason: string;
}

/**
 * Relay promoted event.
 */
export interface RelayPromotedEvent extends Event {
  type: 'relay:promoted';
  /** Number of connections when promoted. */
  connectionCount: number;
  /** Battery level when promoted (0-100). */
  batteryLevel: number;
}

/**
 * Relay demoted event.
 */
export interface RelayDemotedEvent extends Event {
  type: 'relay:demoted';
  /** Reason for demotion. */
  reason: string;
}

/**
 * File transfer progress event.
 */
export interface FileProgressEvent extends Event {
  type: 'file:progress';
  /** File identifier. */
  fileId: string;
  /** Number of chunks sent so far. */
  chunksSent: number;
  /** Total number of chunks. */
  totalChunks: number;
  /** Progress percentage (0-100). */
  percentage: number;
}

/**
 * File received event.
 */
export interface FileReceivedEvent extends Event {
  type: 'file:received';
  /** File identifier. */
  fileId: string;
  /** File name. */
  fileName: string;
  /** File size in bytes. */
  fileSize: number;
  /** Sender's user ID. */
  sender: string;
}

/**
 * Neighbor discovered event.
 */
export interface NeighborDiscoveredEvent extends Event {
  type: 'neighbor:discovered';
  /** Peer ID of the neighbor. */
  peerId: string;
  /** Transport used to discover. */
  transport: string;
  /** RSSI signal strength (if available). */
  rssi?: number;
}

/**
 * Neighbor lost event.
 */
export interface NeighborLostEvent extends Event {
  type: 'neighbor:lost';
  /** Peer ID of the lost neighbor. */
  peerId: string;
}

/**
 * Network metrics event.
 */
export interface NetworkMetricsEvent extends Event {
  type: 'network:metrics';
  /** Number of active neighbors. */
  neighborCount: number;
  /** Number of active relays. */
  relayCount: number;
  /** Message delivery ratio (0.0-1.0). */
  deliveryRatio: number;
  /** Average message latency in milliseconds. */
  avgLatencyMs: number;
}

/**
 * Union type of all possible events.
 */
export type ProtocolEvent =
  | MessageReceivedEvent
  | MessageDeliveredEvent
  | MessageFailedEvent
  | TransportSwitchedEvent
  | RelayPromotedEvent
  | RelayDemotedEvent
  | FileProgressEvent
  | FileReceivedEvent
  | NeighborDiscoveredEvent
  | NeighborLostEvent
  | NetworkMetricsEvent;

/**
 * Event listener function type.
 */
export type EventListener<T extends Event = ProtocolEvent> = (event: T) => void;
