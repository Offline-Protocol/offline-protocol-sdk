/**
 * TypeScript definitions for Offline Protocol SDK (React Web)
 */

/**
 * Message priority levels
 */
export enum MessagePriority {
  Low = 0,
  Medium = 1,
  High = 2,
  Critical = 3,
}

/**
 * Transport types
 */
export enum TransportType {
  Internet = 'internet',
  BLE = 'ble',
  WiFiDirect = 'wifidirect',
}

/**
 * Protocol configuration
 * 
 * Note: Web browsers only support Internet transport (BLE and Wi-Fi Direct are not available).
 */
export interface ProtocolConfig {
  /** Application identifier (required) */
  appId: string;
  
  /** User identifier (required) */
  userId: string;
  
  /** Transport configuration */
  transport?: {
    /** Enable BLE transport (not available in web browsers, ignored) */
    bleEnabled?: boolean;
    
    /** Enable Wi-Fi Direct transport (not available in web browsers, ignored) */
    wifiDirectEnabled?: boolean;
    
    /** Enable Internet transport (only available transport in web) */
    internetEnabled?: boolean;
  };
  
  /** DORS configuration */
  dors?: {
    /** Prefer online/Internet when available */
    preferOnline?: boolean;
    
    /** Hysteresis for transport switching */
    switchHysteresis?: number;
    
    /** Cooldown after switching (seconds) */
    switchCooldownSecs?: number;
  };
  
  /** Relay configuration */
  relay?: {
    /** Allow this device to act as relay */
    allowRelay?: boolean;
    
    /** Minimum battery to act as relay (percentage) */
    minBatteryForRelay?: number;
    
    /** Relay threshold (min connections) */
    relayThreshold?: number;
  };
  
  /** Network parameters */
  network?: {
    /** Initial TTL for messages */
    initialTtl?: number;
  };
}

/**
 * Message data
 */
export interface Message {
  messageId: string;
  sender: string;
  recipient: string;
  content: string;
  priority: MessagePriority;
  hopCount: number;
  timestamp: number;
}

/**
 * Event types
 */
export type Event =
  | MessageSentEvent
  | MessageReceivedEvent
  | MessageDeliveredEvent
  | MessageFailedEvent
  | TransportSwitchedEvent
  | RelayPromotedEvent
  | RelayDemotedEvent
  | NeighborDiscoveredEvent
  | NeighborLostEvent
  | NetworkMetricsEvent
  | FileProgressEvent
  | FileReceivedEvent;

export interface MessageSentEvent {
  type: 'message_sent';
  messageId: string;
  timestamp: number;
}

export interface MessageReceivedEvent {
  type: 'message_received';
  messageId: string;
  sender: string;
  recipient: string;
  content: string;
  hopCount: number;
  transport: string;
  timestamp: number;
}

export interface MessageDeliveredEvent {
  type: 'message_delivered';
  messageId: string;
  latencyMs: number;
  hopCount: number;
  transport: string;
}

export interface MessageFailedEvent {
  type: 'message_failed';
  messageId: string;
  reason: string;
  retryCount: number;
}

export interface TransportSwitchedEvent {
  type: 'transport_switched';
  from?: string;
  to: string;
  reason: string;
}

export interface RelayPromotedEvent {
  type: 'relay_promoted';
  connectionCount: number;
  batteryLevel: number;
}

export interface RelayDemotedEvent {
  type: 'relay_demoted';
  reason: string;
}

export interface NeighborDiscoveredEvent {
  type: 'neighbor_discovered';
  peerId: string;
  transport: string;
  rssi?: number;
}

export interface NeighborLostEvent {
  type: 'neighbor_lost';
  peerId: string;
}

export interface NetworkMetricsEvent {
  type: 'network_metrics';
  neighborCount: number;
  relayCount: number;
  deliveryRatio: number;
  avgLatencyMs: number;
}

export interface FileProgressEvent {
  type: 'file_progress';
  fileId: string;
  chunksSent: number;
  totalChunks: number;
  percentage: number;
}

export interface FileReceivedEvent {
  type: 'file_received';
  fileId: string;
  fileName: string;
  fileSize: number;
  sender: string;
}

/**
 * Event listener types
 */
export type EventListener<T extends Event = Event> = (event: T) => void;

