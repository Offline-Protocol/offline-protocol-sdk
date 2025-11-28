/**
 * TypeScript type definitions for Offline Protocol SDK
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
 * Protocol state
 */
export enum ProtocolState {
  Stopped = 0,
  Running = 1,
  Paused = 2,
}

export interface AckConfig {
  /** Default ACK timeout in milliseconds */
  defaultTimeoutMs?: number;
  /** Maximum number of pending ACKs */
  maxPendingAcks?: number;
}

export interface RetryConfig {
  /** Maximum number of retries per message */
  maxRetries?: number;
  /** Initial retry delay in milliseconds */
  initialDelayMs?: number;
  /** Maximum retry delay in milliseconds */
  maxDelayMs?: number;
  /** Exponential backoff multiplier */
  backoffMultiplier?: number;
  /** Maximum lifetime for outbox messages in milliseconds */
  outboxMaxLifetimeMs?: number;
}

export interface DedupConfig {
  /** Maximum number of message IDs to track */
  maxTrackedMessages?: number;
  /** Time to retain IDs in seconds */
  retentionTimeSecs?: number;
}

/**
 * Deduplicator statistics for monitoring
 */
export interface DedupStats {
  /** Total number of messages being tracked */
  totalTracked: number;
  /** Number of messages seen in the last minute */
  recentTracked: number;
  /** Percentage of capacity used (0-100) */
  capacityUsedPercent: number;
  /** Deduplication mode ("HashMap" or "BloomFilter") */
  mode: string;
}

export interface ReliabilityConfig {
  /** ACK handling configuration */
  ack?: AckConfig;
  /** Retry queue configuration */
  retry?: RetryConfig;
  /** Deduplication configuration */
  dedup?: DedupConfig;
}

export type RelayPriority = 'never' | 'auto' | 'always';

export interface RelayConfig {
  /** Allow device to act as relay */
  allowRelay?: boolean;
  /** Minimum battery level for relaying */
  minBatteryForRelay?: number;
  /** Connection threshold for relay promotion */
  relayThreshold?: number;
  /** Preferred relay behavior */
  relayPriority?: RelayPriority;
}

export interface NetworkConfig {
  /** Initial TTL (time-to-live) */
  initialTtl?: number;
}

export interface PathConfig {
  /** Number of neighbors to forward to for redundancy */
  forwardToTopK?: number;
  /** Maximum acceptable congestion level (0.0-1.0) */
  maxCongestionLevel?: number;
}

/**
 * BLE transport configuration
 */
export interface BleTransportConfig {
  /** Enable BLE transport */
  enabled: boolean;
}

/**
 * Internet transport configuration
 */
export interface InternetTransportConfig {
  /** Enable Internet transport */
  enabled: boolean;
  /** Server address (WebSocket URL) */
  serverAddress?: string;
  /** Enable automatic reconnection */
  autoReconnect?: boolean;
  /** Reconnection delay in milliseconds */
  reconnectDelay?: number;
}

/**
 * WiFi Direct transport configuration
 */
export interface WifiDirectTransportConfig {
  /** Enable WiFi Direct transport */
  enabled: boolean;
  /** Device name to advertise */
  deviceName?: string;
  /** Enable autonomous group owner negotiation */
  autoAccept?: boolean;
  /** Group owner intent (0-15, higher = more likely to be GO) */
  groupOwnerIntent?: number;
}

/**
 * File transfer configuration
 */
export interface FileTransferConfig {
  /** Size of each chunk in bytes (default: 32KB) */
  chunkSize?: number;
  /** Maximum file size allowed in bytes (default: 100MB) */
  maxFileSize?: number;
}

/**
 * File transfer progress information
 */
export interface FileProgress {
  /** File identifier */
  file_id: string;
  /** File name */
  file_name: string;
  /** Total file size in bytes */
  file_size: number;
  /** Number of chunks completed */
  chunks_completed: number;
  /** Total number of chunks */
  total_chunks: number;
  /** Progress percentage (0-100) */
  percentage: number;
}

/**
 * Transport configuration
 */
export interface TransportsConfig {
  /** BLE transport configuration */
  ble?: BleTransportConfig;
  /** Internet transport configuration */
  internet?: InternetTransportConfig;
  /** WiFi Direct transport configuration (Android only) */
  wifiDirect?: WifiDirectTransportConfig;
}

/**
 * Protocol configuration
 */
export interface ProtocolConfig {
  /** Application identifier */
  appId: string;
  /** User identifier */
  userId: string;
  /** Transport configuration (optional) */
  transports?: TransportsConfig;
  /** File transfer configuration (optional) */
  fileTransfer?: FileTransferConfig;
  /** DORS configuration (optional) */
  dors?: {
    /** Prefer online mode (default: false) */
    preferOnline?: boolean;
    /** Minimum score improvement required to switch transports (default: 15.0) */
    switchHysteresis?: number;
    /** Cooldown period after switching in seconds (default: 20) */
    switchCooldownSecs?: number;
    /** Number of retry failures before escalating from BLE to Wi-Fi Direct (default: 2) */
    bleToWifiRetryThreshold?: number;
    /** RSSI threshold for switching from BLE to Wi-Fi Direct in dBm (default: -85) */
    rssiSwitchThreshold?: number;
    /** Queue depth threshold for detecting congestion (default: 50) */
    congestionQueueThreshold?: number;
    /** Duration for checking stability before switching in seconds (default: 8) */
    stabilityWindowSecs?: number;
    /** Duration that RSSI must remain below the threshold before escalating (default: 10) */
    poorSignalDurationSecs?: number;
    /** TTL threshold that signals impending exhaustion (default: 2) */
    ttlEscalationThreshold?: number;
    /** Duration congestion must persist before escalating (default: 10) */
    congestionDurationSecs?: number;
    /** Duration to keep TTL escalation signal active (default: 20) */
    ttlEscalationHoldSecs?: number;
    /** Number of history samples to retain for smoothing (default: 10) */
    historyWindowSize?: number;
    /** Queue depth ratio indicating recovery (default: 0.5) */
    queueRecoveryRatio?: number;
  };
  /** Relay configuration (optional) */
  relay?: RelayConfig;
  /** Network configuration (optional) */
  network?: NetworkConfig;
  /** Reliability configuration (optional) */
  reliability?: ReliabilityConfig;
  /** Path selection configuration (optional) */
  path?: PathConfig;
}

/**
 * Transport type names
 */
export type TransportType = 'ble' | 'internet' | 'wifiDirect';

/**
 * Parameters for sending a message
 */
export interface SendMessageParams {
  /** Recipient's user ID */
  recipient: string;
  /** Message content */
  content: string;
  /** Message priority (optional, defaults to Medium) */
  priority?: MessagePriority;
}

/**
 * Parameters for sending a file
 */
export interface SendFileParams {
  /** File path or URI */
  filePath: string;
  /** Recipient's user ID */
  recipient: string;
  /** Optional custom file name */
  fileName?: string;
}

/**
 * Base event interface
 */
interface BaseEvent {
  type: string;
  /**
   * Local timestamp (ms) when this event was observed by the JS bridge.
   * Populated on the client so analytics can reason about event ordering even
   * when native payloads omit timestamps.
   */
  seenAt?: number;
}

/**
 * Message sent event
 */
export interface MessageSentEvent extends BaseEvent {
  type: 'message_sent';
  message_id: string;
  sender: string;
  recipient: string;
  content: string;
  priority: 'low' | 'medium' | 'high' | 'critical';
  requires_ack: boolean;
  timestamp: number;
}

/**
 * Message received event
 */
export interface MessageReceivedEvent extends BaseEvent {
  type: 'message_received';
  message_id: string;
  sender: string;
  recipient: string;
  content: string;
  hop_count: number;
  transport: string;
  timestamp: number;
}

/**
 * Message delivered event
 */
export interface MessageDeliveredEvent extends BaseEvent {
  type: 'message_delivered';
  message_id: string;
  latency_ms: number;
  hop_count: number;
  transport: string;
}

/**
 * Message failed event
 */
export interface MessageFailedEvent extends BaseEvent {
  type: 'message_failed';
  message_id: string;
  reason: string;
  retry_count: number;
}

/**
 * Transport switched event
 */
export interface TransportSwitchedEvent extends BaseEvent {
  type: 'transport_switched';
  from: string | null;
  to: string;
  reason: string;
}

/**
 * Relay promoted event
 */
export interface RelayPromotedEvent extends BaseEvent {
  type: 'relay_promoted';
  connection_count: number;
  battery_level: number;
}

/**
 * Relay demoted event
 */
export interface RelayDemotedEvent extends BaseEvent {
  type: 'relay_demoted';
  reason: string;
}

/**
 * Neighbor discovered event
 */
export interface NeighborDiscoveredEvent extends BaseEvent {
  type: 'neighbor_discovered';
  peer_id: string;
  transport: string;
  rssi?: number;
}

/**
 * Neighbor lost event
 */
export interface NeighborLostEvent extends BaseEvent {
  type: 'neighbor_lost';
  peer_id: string;
}

/**
 * Network metrics event
 */
export interface NetworkMetricsEvent extends BaseEvent {
  type: 'network_metrics';
  neighbor_count: number;
  relay_count: number;
  delivery_ratio: number;
  avg_latency_ms: number;
}

/**
 * File progress event
 */
export interface FileProgressEvent extends BaseEvent {
  type: 'file_progress';
  file_id: string;
  chunks_sent: number;
  total_chunks: number;
  percentage: number;
}

/**
 * File received event
 */
export interface FileReceivedEvent extends BaseEvent {
  type: 'file_received';
  file_id: string;
  file_name: string;
  file_size: number;
  sender: string;
}

/**
 * Diagnostic event emitted by native modules for debugging purposes
 */
export interface DiagnosticEvent extends BaseEvent {
  type: 'diagnostic';
  level: 'info' | 'warning' | 'error';
  message: string;
  context?: Record<string, unknown>;
}

/**
 * Union type of all events
 */
export type ProtocolEvent =
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
  | FileReceivedEvent
  | DiagnosticEvent;

/**
 * Event listener type
 */
export type EventListener<T extends ProtocolEvent = ProtocolEvent> = (event: T) => void;

/**
 * Event type names
 */
export type EventType = ProtocolEvent['type'];

/**
 * Node role in the network
 */
export enum NodeRole {
  Normal = 'normal',
  Relay = 'relay',
}

/**
 * Network node information
 */
export interface NetworkNode {
  /** Node user ID */
  user_id: string;
  /** Node role (Normal or Relay) */
  role: NodeRole;
  /** Connection count */
  connection_count: number;
  /** Battery level (0-100), if known */
  battery_level?: number;
  /** Last seen timestamp */
  last_seen: number;
  /** Transport types available */
  transports: TransportType[];
}

/**
 * Link between two nodes
 */
export interface NetworkLink {
  /** Source node */
  from: string;
  /** Destination node */
  to: string;
  /** Link quality (0.0 - 1.0) */
  quality: number;
  /** Transport type used for this link */
  transport: TransportType;
  /** RSSI (signal strength) if available */
  rssi?: number;
}

/**
 * Network-wide statistics
 */
export interface NetworkStats {
  /** Total nodes in network */
  total_nodes: number;
  /** Total relay nodes */
  relay_nodes: number;
  /** Total active connections */
  total_connections: number;
  /** Average link quality */
  avg_link_quality: number;
  /** Network diameter (max hops between any two nodes) */
  network_diameter?: number;
}

/**
 * Complete network topology snapshot
 */
export interface NetworkTopology {
  /** Timestamp of this snapshot */
  timestamp: number;
  /** Local device user ID */
  local_user_id: string;
  /** All nodes in the network */
  nodes: NetworkNode[];
  /** All links between nodes */
  links: NetworkLink[];
  /** Network-wide statistics */
  stats: NetworkStats;
}

/**
 * Message delivery statistics
 */
export interface MessageDeliveryStats {
  /** Message ID */
  message_id: string;
  /** Sender */
  sender: string;
  /** Recipient */
  recipient: string;
  /** Timestamp sent */
  sent_at: number;
  /** Timestamp delivered (if delivered) */
  delivered_at?: number;
  /** Number of hops */
  hop_count: number;
  /** Transport used for final delivery */
  transport?: TransportType;
  /** Retry count */
  retry_count: number;
  /** Delivery latency in milliseconds (if delivered) */
  latency_ms?: number;
}

// ============================================================================
// GRADIENT ROUTING TYPES
// ============================================================================

/**
 * A route entry representing a path to a destination through a neighbor
 */
export interface RouteEntry {
  /** Next hop neighbor ID */
  nextHop: string;
  /** Number of hops to destination */
  hopCount: number;
  /** Route quality score (0.0 - 1.0) */
  quality: number;
  /** Timestamp when route was last seen (ms since epoch) */
  lastSeenMs: number;
}

/**
 * Routing table statistics
 */
export interface RoutingStats {
  /** Number of unique destinations in routing table */
  destinationCount: number;
  /** Total number of routes across all destinations */
  routeCount: number;
}

/**
 * Gradient routing configuration
 */
export interface GradientRoutingConfig {
  /** Maximum routes to keep per destination */
  maxRoutesPerDestination?: number;
  /** Route TTL in seconds before expiration */
  routeTtlSecs?: number;
  /** Maximum total routing table size */
  maxRoutingTableSize?: number;
}

// ============================================================================
// FILE TRANSFER TYPES
// ============================================================================

/**
 * Parameters for processing a file chunk
 */
export interface ProcessFileChunkParams {
  /** File identifier */
  fileId: string;
  /** Zero-based chunk index */
  chunkIndex: number;
  /** Chunk data as array of bytes */
  data: number[];
}

// ============================================================================
// WIFI DIRECT TYPES
// ============================================================================

/**
 * WiFi Direct outgoing message
 */
export interface WifiDirectMessage {
  /** Recipient peer ID */
  recipientId: string;
  /** Message data as array of bytes */
  data: number[];
}

// ============================================================================
// INTERNET TRANSPORT TYPES
// ============================================================================

/**
 * Internet transport outgoing message
 */
export interface InternetMessage {
  /** Recipient ID */
  recipientId: string;
  /** Message data as array of bytes */
  data: number[];
}
