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
  /** Auth token for authentication (if not provided, deviceId will be used) */
  authToken?: string;
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
 * Reticulum mesh transport configuration.
 * Requires a running Reticulum daemon or RNode hardware.
 */
export interface ReticulumTransportConfig {
  /** Enable Reticulum transport (default: false) */
  enabled: boolean;
  /** TCP address of the Reticulum daemon in "host:port" format (default: "localhost:4242") */
  daemonAddress?: string;
  /** Whether to auto-reconnect on disconnect (default: true) */
  autoReconnect?: boolean;
  /** Maximum reconnect attempts, 0 = infinite (default: 0) */
  maxReconnectAttempts?: number;
}

/**
 * Nostr relay transport configuration.
 * Connects to Nostr relays via WebSocket for message routing using NIP-04 direct messages.
 */
export interface NostrTransportConfig {
  /** Enable Nostr transport (default: false) */
  enabled: boolean;
  /** List of Nostr relay WebSocket URLs (e.g., ["wss://relay.damus.io"]) */
  relayUrls?: string[];
  /** Connection timeout in seconds (default: 30) */
  connectionTimeout?: number;
  /** Whether to auto-reconnect on disconnect (default: true) */
  autoReconnect?: boolean;
  /** Reconnection delay in milliseconds (default: 1000) */
  reconnectDelay?: number;
  /** Maximum reconnect attempts per relay, 0 = infinite (default: 0) */
  maxReconnectAttempts?: number;
}

/**
 * Content type for messages
 */
export enum ContentType {
  Text = 'text',
  Image = 'image',
  Video = 'video',
  Audio = 'audio',
  VoiceNote = 'voice_note',
  VideoNote = 'video_note',
  File = 'file',
  FileChunk = 'file_chunk',
}

/**
 * Media metadata for attachments
 */
export interface MediaMetadata {
  /** MIME type (e.g. "image/jpeg", "video/mp4") */
  mimeType: string;
  /** Original file name */
  fileName: string;
  /** File size in bytes */
  fileSize: number;
  /** Duration in milliseconds (audio/video) */
  durationMs?: number;
  /** Width in pixels (images/video) */
  width?: number;
  /** Height in pixels (images/video) */
  height?: number;
  /** Small base64-encoded thumbnail for preview (< 2 KB) */
  thumbnailBase64?: string;
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
  /** Number of chunks acknowledged/processed */
  chunks_sent: number;
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
  /** Reticulum mesh transport configuration (requires external Reticulum daemon) */
  reticulum?: ReticulumTransportConfig;
  /** Nostr relay transport configuration */
  nostr?: NostrTransportConfig;
}

/**
 * Encryption configuration for automatic MLS handling.
 * When enabled, messages are automatically encrypted/decrypted using MLS.
 */
export interface EncryptionConfig {
  /**
   * Enable automatic encryption (default: true).
   * When enabled, messages are automatically encrypted/decrypted.
   */
  enabled?: boolean;
  /**
   * Auto-exchange key packages on peer discovery (default: true).
   * When enabled, key packages are automatically sent when neighbors are discovered.
   */
  autoKeyExchange?: boolean;
  /**
   * Store pending messages when no session exists (default: true).
   * Messages will be sent automatically after the session is established.
   */
  storePending?: boolean;
  /**
   * Require encrypted delivery (default: false).
   * When true, send fails if encryption cannot be applied.
   */
  requireEncryption?: boolean;
  /**
   * Bounds and policy for encrypted messages received before session readiness.
   */
  pendingQueue?: PendingQueueConfig;
}

/**
 * Overflow policy when pending queue reaches configured limits.
 */
export type OverflowPolicy = 'drop_oldest' | 'drop_newest';

/**
 * Bounded pending queue settings for pre-session encrypted messages.
 */
export interface PendingQueueConfig {
  /** Per-peer pending message cap (default: 64) */
  maxPendingPerPeer?: number;
  /** Global pending message cap (default: 4096) */
  maxPendingGlobal?: number;
  /** Pending message TTL in milliseconds (default: 120000) */
  pendingTtlMs?: number;
  /** Overflow behavior when limits are hit (default: 'drop_oldest') */
  overflowPolicy?: OverflowPolicy;
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
  /**
   * Encryption configuration (optional).
   * Defaults to encryption enabled with auto key exchange.
   */
  encryption?: EncryptionConfig;
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
    /** Min BLE success rate (0–1) before escalation; below this triggers escalation (default: 0.3) */
    minSuccessRateBeforeEscalation?: number;
    /** Min BLE samples required before using success-rate escalation (default: 5) */
    minBleSamplesBeforeSuccessRateEscalation?: number;
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
export type TransportType = 'ble' | 'internet' | 'wifiDirect' | 'reticulum' | 'nostr';

/**
 * Forwarding attribution (present when a message was forwarded).
 *
 * **Trust model:** This is a display-level hint, not a cryptographic proof.
 * UI layers should not rely on it for access-control or security decisions.
 */
export interface ForwardInfo {
  /** The original sender's user ID. */
  original_sender: string;
  /** The original message ID. */
  original_message_id: string;
  /** The original timestamp (wall-clock ms). */
  original_timestamp: number;
  /** Number of times this message has been forwarded. */
  forward_count: number;
}

/**
 * Parameters for forwarding a message to a new recipient
 */
export interface ForwardMessageParams {
  /** The original message JSON (as received from the protocol) */
  originalMessageJson: string;
  /** New recipient's user ID */
  newRecipient: string;
  /** Message priority (optional, defaults to Medium) */
  priority?: MessagePriority;
}

/**
 * Parameters for forwarding a message to a group
 */
export interface ForwardMessageToGroupParams {
  /** The original message JSON (as received from the protocol) */
  originalMessageJson: string;
  /** Group ID to forward to */
  groupId: string;
  /** Message priority (optional, defaults to Medium) */
  priority?: MessagePriority;
}

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
  /** ID of the message this is replying to (optional) */
  replyToMsg?: string;
}

/**
 * Parameters for sending a connection request
 */
export interface SendConnectionRequestParams {
  /** Recipient's user ID */
  recipient: string;
  /** Display name of the sender */
  senderName: string;
  /** Optional MLS key package bytes */
  keyPackage?: number[];
}

/**
 * Parameters for accepting a connection request
 */
export interface AcceptConnectionRequestParams {
  /** Recipient's user ID */
  recipient: string;
  /** Display name of the accepting party */
  accepterName: string;
  /** Optional MLS key package bytes */
  keyPackage?: number[];
}

/**
 * Parameters for rejecting a connection request
 */
export interface RejectConnectionRequestParams {
  /** Recipient's user ID */
  recipient: string;
}

/**
 * Parameters for cancelling a connection request
 */
export interface CancelConnectionRequestParams {
  /** Recipient's user ID */
  recipient: string;
}

/**
 * Parameters for sending a media attachment
 */
export interface SendMediaParams {
  /** Recipient's user ID */
  recipient: string;
  /** Raw file data as a base64 string (platform reads the file) */
  fileData: string;
  /** File name */
  fileName: string;
  /** Content type of the media */
  contentType: ContentType;
  /** Optional media metadata (dimensions, duration, thumbnail, etc.) */
  mediaMetadata?: MediaMetadata;
}

/**
 * Parameters for sending a file (convenience wrapper around SendMediaParams)
 */
export interface SendFileParams {
  /** Recipient's user ID */
  recipient: string;
  /** Raw file data as a base64 string (platform reads the file) */
  fileData: string;
  /** File name */
  fileName: string;
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
  /** Lamport logical clock value for causal ordering (0 for legacy messages). */
  lamport_clock: number;
  /** Forwarding attribution (present when this is a forwarded message). */
  forward_info?: ForwardInfo;
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
  /** Lamport logical clock value for causal ordering (0 for legacy messages). */
  lamport_clock: number;
  /** Whether the message was encrypted (auto-decrypted) */
  encrypted?: boolean;
  /** The type of content (text, image, video, voice_note, etc.). */
  content_type?: string;
  /** Media metadata (present for non-text content). */
  media_metadata?: {
    mime_type?: string;
    file_name?: string;
    file_size?: number;
    duration_ms?: number;
    width?: number;
    height?: number;
    thumbnail_base64?: string;
  };
  /** Forwarding attribution (present when this is a forwarded message). */
  forward_info?: ForwardInfo;
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
  /** The content type of the media (image, video, file, etc.). */
  content_type: string;
  /** Media metadata from the sender. */
  media_metadata?: {
    mime_type?: string;
    file_name?: string;
    file_size?: number;
    duration_ms?: number;
    width?: number;
    height?: number;
    thumbnail_base64?: string;
  };
  /** Base64-encoded reassembled file data. */
  file_data: string;
}

/**
 * Media sent event - all chunks ACK-delivered
 */
export interface MediaSentEvent extends BaseEvent {
  type: 'media_sent';
  file_id: string;
  content_type: string;
  recipient: string;
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
 * Secure session established event
 * Emitted when an MLS session is successfully established with a peer
 */
export interface SecureSessionEstablishedEvent extends BaseEvent {
  type: 'secure_session_established';
  /** Peer ID of the other party */
  peer_id: string;
  /** MLS group ID for the session */
  group_id: string;
  /** Whether this is a 1:1 session (true) or a multi-party group (false) */
  is_session: boolean;
  /** Whether the local device initiated the session (sent the Welcome) */
  initiated_by_local: boolean;
}

/**
 * Secure session failed event
 * Emitted when an MLS session fails to be established
 */
export interface SecureSessionFailedEvent extends BaseEvent {
  type: 'secure_session_failed';
  /** Peer ID of the other party */
  peer_id: string;
  /** Reason for the failure */
  reason: string;
}

/**
 * Machine-readable reason codes for welcome delivery failures.
 */
export type WelcomeReasonCode =
  | 'TRANSPORT_UNAVAILABLE'
  | 'PEER_DISCONNECTED'
  | 'TIMEOUT'
  | 'INTERNAL_ERROR'
  | 'RETRY_EXHAUSTED';

/**
 * Welcome send attempted event
 */
export interface WelcomeSendAttemptedEvent extends BaseEvent {
  type: 'welcome_send_attempted';
  peer_id: string;
  message_id: string;
  group_id: string;
  attempt: number;
}

/**
 * Welcome send succeeded event
 */
export interface WelcomeSendSucceededEvent extends BaseEvent {
  type: 'welcome_send_succeeded';
  peer_id: string;
  message_id: string;
  group_id: string;
  attempt: number;
}

/**
 * Welcome send failed event
 */
export interface WelcomeSendFailedEvent extends BaseEvent {
  type: 'welcome_send_failed';
  peer_id: string;
  message_id: string;
  group_id: string;
  attempt: number;
  reason_code: WelcomeReasonCode;
  transport_error?: string;
  retryable: boolean;
  next_retry_at?: number;
}

/**
 * Welcome send expired event
 */
export interface WelcomeSendExpiredEvent extends BaseEvent {
  type: 'welcome_send_expired';
  peer_id: string;
  message_id: string;
  attempt: number;
  reason_code: WelcomeReasonCode;
}

/**
 * Connection request received event
 */
export interface ConnectionRequestReceivedEvent extends BaseEvent {
  type: 'connection_request_received';
  sender: string;
  sender_name: string;
  timestamp: number;
  key_package?: number[];
}

/**
 * Connection accepted event
 */
export interface ConnectionAcceptedEvent extends BaseEvent {
  type: 'connection_accepted';
  accepted_by: string;
  accepted_by_name: string;
  timestamp: number;
  key_package?: number[];
}

/**
 * Connection rejected event
 */
export interface ConnectionRejectedEvent extends BaseEvent {
  type: 'connection_rejected';
  rejected_by: string;
}

/**
 * Connection request cancelled event
 */
export interface ConnectionRequestCancelledEvent extends BaseEvent {
  type: 'connection_request_cancelled';
  cancelled_by: string;
}

/**
 * Group created event (from relay)
 */
export interface GroupCreatedEvent extends BaseEvent {
  type: 'group_created';
  group_id: string;
  name: string;
}

/**
 * Group message received event (from relay)
 */
export interface GroupMessageReceivedEvent extends BaseEvent {
  type: 'group_message_received';
  group_id: string;
  sender: string;
  content: string;
  timestamp: string;
  message_id: string;
  reply_to_msg?: string;
  /** Forwarding attribution (present when this is a forwarded message). */
  forward_info?: ForwardInfo;
}

/**
 * Group member added event (from relay)
 */
export interface GroupMemberAddedEvent extends BaseEvent {
  type: 'group_member_added';
  group_id: string;
  user_id: string;
  added_by: string;
  group_name?: string;
}

/**
 * Group member removed event (from relay)
 */
export interface GroupMemberRemovedEvent extends BaseEvent {
  type: 'group_member_removed';
  group_id: string;
  user_id: string;
  removed_by: string;
}

/**
 * Group info member (in GroupInfoEvent)
 */
export interface GroupInfoMemberEvent {
  user_id: string;
  role: string;
  joined_at: string;
}

/**
 * Group info event (from relay)
 */
export interface GroupInfoEvent extends BaseEvent {
  type: 'group_info';
  group_id: string;
  name: string;
  created_by: string;
  created_at: string;
  members: GroupInfoMemberEvent[];
}

/**
 * User group summary (in UserGroupsEvent)
 */
export interface UserGroupSummaryEvent {
  group_id: string;
  name: string;
  created_at: string;
}

/**
 * User groups event (from relay)
 */
export interface UserGroupsEvent extends BaseEvent {
  type: 'user_groups';
  groups: UserGroupSummaryEvent[];
}

/**
 * Group error event (from relay)
 */
export interface GroupErrorEvent extends BaseEvent {
  type: 'group_error';
  reason: string;
}

/**
 * Group message sent event — a group message was sent to all members via mesh
 * (MLS-encrypted fan-out).
 */
export interface GroupMessageSentEvent extends BaseEvent {
  type: 'group_message_sent';
  group_id: string;
  message_ids: string[];
  member_count: number;
}

/**
 * Group message partial failure — some members could not be reached.
 */
export interface GroupMessagePartialFailureEvent extends BaseEvent {
  type: 'group_message_partial_failure';
  group_id: string;
  failed_members: string[];
  succeeded_members: string[];
}

/**
 * Epoch fork detected — concurrent MLS commits caused members to diverge
 * onto different branches. The deterministic leader will attempt automatic
 * resolution.
 */
export interface GroupEpochForkDetectedEvent extends BaseEvent {
  type: 'group_epoch_fork_detected';
  group_id: string;
  local_epoch?: number;
}

/**
 * Epoch fork resolved — the leader re-established a canonical epoch.
 * Members in `failed_members` could not be reached with the resolution
 * commit and may need re-inviting.
 */
export interface GroupEpochForkResolvedEvent extends BaseEvent {
  type: 'group_epoch_fork_resolved';
  group_id: string;
  resolved_epoch: number;
  failed_members: string[];
}

/**
 * Group role changed — a member's role was changed (e.g. admin ↔ member).
 */
export interface GroupRoleChangedEvent extends BaseEvent {
  type: 'group_role_changed';
  group_id: string;
  user_id: string;
  new_role: string;
  changed_by: string;
}

// ============================================================================
// SERVICE DISCOVERY & REQUEST/RESPONSE EVENTS
// ============================================================================

/**
 * A service was discovered on the mesh in response to a discovery query.
 */
export interface ServiceDiscoveredEvent extends BaseEvent {
  type: 'service_discovered';
  query_id: string;
  service_id: string;
  version: string;
  provider_peer_id: string;
  capabilities: Record<string, string>;
  hop_count: number;
}

/**
 * A service request was received from another peer.
 */
export interface ServiceRequestReceivedEvent extends BaseEvent {
  type: 'service_request_received';
  request_id: string;
  service_id: string;
  method: string;
  body: string;
  sender: string;
}

/**
 * A response to a service request was received.
 */
export interface ServiceResponseReceivedEvent extends BaseEvent {
  type: 'service_response_received';
  request_id: string;
  service_id: string;
  status: string;
  body: string;
  provider_peer_id: string;
}

// ============================================================================
// PRESENCE, TYPING, READ RECEIPTS EVENTS
// ============================================================================

/**
 * Presence status values
 */
export type PresenceStatus = 'online' | 'away' | 'offline';

/**
 * Presence updated event — a peer sent a presence update
 */
export interface PresenceUpdatedEvent extends BaseEvent {
  type: 'presence_updated';
  peer_id: string;
  status: PresenceStatus;
  timestamp: number;
}

/**
 * Typing indicator received event
 */
export interface TypingIndicatorReceivedEvent extends BaseEvent {
  type: 'typing_indicator_received';
  sender: string;
  conversation_id: string;
  is_typing: boolean;
  timestamp: number;
}

/**
 * Read receipt received event
 */
export interface ReadReceiptReceivedEvent extends BaseEvent {
  type: 'read_receipt_received';
  sender: string;
  message_ids: string[];
  timestamp: number;
}

// ============================================================================
// MESSAGE RELAY & DEFERRAL EVENTS
// ============================================================================

/**
 * Message relayed event — this node forwarded a message for another peer
 */
export interface MessageRelayedEvent extends BaseEvent {
  type: 'message_relayed';
  message_id: string;
  sender: string;
  recipient: string;
  hop_count: number;
  remaining_ttl: number;
}

/**
 * Message deferred event — a message was queued because no transport was available
 */
export interface MessageDeferredEvent extends BaseEvent {
  type: 'message_deferred';
  message_id: string;
  reason: string;
  retry_count: number;
  next_retry_at?: number;
}

// ============================================================================
// DORS OBSERVABILITY EVENTS (from SDK DORS decision / escalation)
// ============================================================================

/** Reason code for DORS transport selection/switch. */
export type DorsReasonCode =
  | 'INITIAL_SELECTION'
  | 'PRIMARY_SELECTED'
  | 'PRIMARY_SUCCESS'
  | 'FALLBACK_SUCCESS'
  | 'ESCALATION_APPLIED'
  | 'CURRENT_UNAVAILABLE';

/** Phase of DORS escalation: TRIGGERED = recommendation, APPLIED = fallback succeeded. */
export type DorsEscalationPhase = 'TRIGGERED' | 'APPLIED';

/** Reason code for DORS BLE→Wi‑Fi escalation. */
export type DorsEscalationReasonCode =
  | 'FALLBACK_SUCCESS'
  | 'RETRY_THRESHOLD'
  | 'POOR_SIGNAL'
  | 'CONGESTION'
  | 'LOW_TTL'
  | 'LOW_SUCCESS_RATE';

/**
 * DORS score updated event (per-transport scores).
 */
export interface DorsScoreUpdatedEvent extends BaseEvent {
  type: 'dors_score_updated';
  scores: Array<[string, number]>;
}

/**
 * DORS transport selected event (current choice and reason).
 */
export interface DorsTransportSelectedEvent extends BaseEvent {
  type: 'dors_transport_selected';
  from: string | null;
  transport: string;
  reason_code: DorsReasonCode;
  score?: number;
}

/**
 * DORS transport switched event (transition with reason).
 */
export interface DorsTransportSwitchedEvent extends BaseEvent {
  type: 'dors_transport_switched';
  from: string | null;
  to: string;
  reason_code: DorsReasonCode;
  reason_detail?: string;
}

/**
 * DORS escalation triggered (BLE→Wi‑Fi recommendation or applied).
 */
export interface DorsEscalationTriggeredEvent extends BaseEvent {
  type: 'dors_escalation_triggered';
  phase: DorsEscalationPhase;
  from: string;
  to: string;
  reason_code: DorsEscalationReasonCode;
  reason_detail?: string;
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
  | MediaSentEvent
  | DiagnosticEvent
  | SecureSessionEstablishedEvent
  | SecureSessionFailedEvent
  | WelcomeSendAttemptedEvent
  | WelcomeSendSucceededEvent
  | WelcomeSendFailedEvent
  | WelcomeSendExpiredEvent
  | ConnectionRequestReceivedEvent
  | ConnectionAcceptedEvent
  | ConnectionRejectedEvent
  | ConnectionRequestCancelledEvent
  | GroupCreatedEvent
  | GroupMessageReceivedEvent
  | GroupMemberAddedEvent
  | GroupMemberRemovedEvent
  | GroupInfoEvent
  | UserGroupsEvent
  | GroupErrorEvent
  | GroupMessageSentEvent
  | GroupMessagePartialFailureEvent
  | GroupEpochForkDetectedEvent
  | GroupEpochForkResolvedEvent
  | GroupRoleChangedEvent
  | DorsScoreUpdatedEvent
  | DorsTransportSelectedEvent
  | DorsTransportSwitchedEvent
  | DorsEscalationTriggeredEvent
  | ServiceDiscoveredEvent
  | ServiceRequestReceivedEvent
  | ServiceResponseReceivedEvent
  | PresenceUpdatedEvent
  | TypingIndicatorReceivedEvent
  | ReadReceiptReceivedEvent
  | MessageRelayedEvent
  | MessageDeferredEvent;

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
 * Internet transport outgoing message.
 *
 * After sending the data over the wire, report the outcome by calling
 * `internetConfirmSent(messageId)` or `internetSendFailed(messageId)`.
 */
export interface InternetMessage {
  /** Unique message identifier — pass to `internetConfirmSent` / `internetSendFailed` */
  messageId: string;
  /** Recipient ID */
  recipientId: string;
  /** Message data as array of bytes */
  data: number[];
}

// ============================================================================
// MLS (MESSAGE LAYER SECURITY) TYPES
// ============================================================================

/**
 * Per-peer secure-session establishment state.
 */
export type EstablishmentState =
  | 'NoKeyPackage'
  | 'HaveKeyPackage'
  | 'SessionPending'
  | 'SessionConfirmed';

/**
 * MLS key package for session establishment.
 * Key packages are used to establish encrypted sessions with peers.
 */
export interface MlsKeyPackage {
  /** Unique identifier for this key package */
  packageId: string;
  /** User ID that owns this key package */
  userId: string;
  /** Raw key package data (bytes) */
  keyPackageData: number[];
  /** Timestamp when this package was created */
  createdAt: number;
  /** Whether this package has been synced to a server */
  isSynced: boolean;
}

/**
 * MLS encrypted message structure.
 * Contains the ciphertext and metadata needed for decryption.
 */
export interface MlsEncryptedMessage {
  /** Group ID (for 1:1 this is "session:userId") */
  groupId: string;
  /** MLS message type (e.g., "Application", "Proposal", "Commit") */
  messageType: string;
  /** MLS epoch number */
  epoch: number;
  /** Encrypted ciphertext (bytes) */
  ciphertext: number[];
  /** Sender's user ID */
  senderId: string;
  /** Timestamp in milliseconds */
  timestampMs: number;
}

/**
 * MLS welcome message for joining a session or group.
 * Sent to new members to allow them to join an encrypted session.
 */
export interface MlsWelcome {
  /** Group ID being joined */
  groupId: string;
  /** Raw welcome message data (bytes) */
  welcomeData: number[];
  /** User ID of the inviter */
  inviterId: string;
  /** Timestamp in milliseconds */
  timestampMs: number;
}

/**
 * MLS session information for 1:1 encrypted conversations.
 */
export interface MlsSessionInfo {
  /** Other user's ID in this session */
  otherUserId: string;
  /** Group ID for this session */
  groupId: string;
  /** Current epoch number */
  epoch: number;
  /** Timestamp when session was created */
  createdAt: number;
}

/**
 * MLS group information for group encrypted conversations.
 */
export interface MlsGroupInfo {
  /** Unique group identifier */
  groupId: string;
  /** Human-readable group name */
  groupName: string;
  /** List of member user IDs */
  memberIds: string[];
  /** Current epoch number */
  epoch: number;
  /** Timestamp when group was created */
  createdAt: number;
}

/**
 * MLS commit message for group state updates.
 * Used when members are added or removed from a group.
 */
export interface MlsCommit {
  /** Group ID being updated */
  groupId: string;
  /** Raw commit data (bytes) */
  commitData: number[];
  /** New epoch after commit */
  newEpoch: number;
}

// ============================================================================
// TELEMETRY (unified observer surface — mirrors the UniFFI TelemetrySink)
// ============================================================================

/** MLS lifecycle verbosity tier for TelemetryConfig. */
export type MlsVerbosity = 'off' | 'lifecycle' | 'diagnostic';

/** Underlying connection status of a single transport. */
export type TransportStatus =
  | 'available'
  | 'unavailable'
  | 'connecting'
  | 'disconnected'
  | 'error';

/** Local relay role reported by DeviceCapabilitySnapshot. */
export type RelayRole = 'regular' | 'relay';

/**
 * Which kind of routing decision a RoutingDecision record describes.
 * `'unknown'` signals new-core / old-FFI skew — consumers should surface it as
 * "unrecognised" rather than folding it into an existing phase.
 */
export type RoutingPhase =
  | 'scoreUpdated'
  | 'selected'
  | 'switched'
  | 'escalated'
  | 'unknown';

/**
 * Flat reason space for routing decisions. `'unknown'` carries the same
 * new-core / old-FFI skew semantics as `RoutingPhase`'s `'unknown'`.
 */
export type RoutingReasonCode =
  | 'initialSelection'
  | 'primarySelected'
  | 'primarySuccess'
  | 'fallbackSuccess'
  | 'escalationApplied'
  | 'currentUnavailable'
  | 'retryThreshold'
  | 'poorSignal'
  | 'congestion'
  | 'lowTtl'
  | 'lowSuccessRate'
  | 'unknown';

/**
 * Runtime configuration for the telemetry subsystem. All fields optional.
 * Defaults (applied on the Rust side when a field is omitted):
 *   scrubIds          = true
 *   mlsVerbosity      = 'lifecycle'
 *   metricsCadenceMs  = 5000
 *   routingDiagnostic = false
 *   enablePollQueue   = true
 *
 * Note: omitting `metricsCadenceMs` (or passing `undefined`) yields the
 * default cadence. There is currently no way to disable periodic emission
 * via this config — the single optional field cannot distinguish "use
 * default" from "disable" across the FFI. Track as a follow-up if disable
 * support is needed.
 *
 * `enablePollQueue` controls whether the Rust adapter builds the
 * pull-channel JSON envelope on every emit. Leave it at the default
 * (`true` / omitted) if you use `pollTelemetry()`; pass `false` for a
 * push-only integration to skip the per-emit `serde_json` cost on the
 * routing hot path. With `false`, `pollTelemetry()` returns `null` for
 * records emitted under that config (previously enqueued records remain
 * readable until drained).
 *
 * `mlsSamplingBypass` (default false) opts a telemetry-grade sink out of the
 * fixed-window rate limiter on high-volume MLS lifecycle events
 * (`mls.decryption_failed`, `mls.session_missing`) so aggregate counts are not
 * clipped to the per-window ceiling. Only enable it for sinks that apply their
 * own backpressure (e.g. enqueue-and-drain on a background task).
 */
export interface TelemetryConfig {
  scrubIds?: boolean;
  mlsVerbosity?: MlsVerbosity;
  metricsCadenceMs?: number;
  routingDiagnostic?: boolean;
  enablePollQueue?: boolean;
  mlsSamplingBypass?: boolean;
}

/**
 * Per-transport metrics — same shape flows through getTransportMetrics (pull)
 * and MetricsFrame.transports (push). The six legacy counters are always
 * present; the remaining fields populate whenever a transport reports them.
 */
export interface TransportMetrics {
  packetsSent: number;
  packetsReceived: number;
  bytesSent: number;
  bytesReceived: number;
  errorRate: number;
  avgLatencyMs: number;
  rssi?: number;
  bandwidthBps?: number;
  congestion?: number;
  queueDepth?: number;
  batteryLevel?: number;
  isCharging?: boolean;
  relayConnectionCount?: number;
  isActiveRelay?: boolean;
  deliveryRatio?: number;
  dropRate?: number;
  averageHopCount?: number;
  energyCost?: number;
}

/** Per-transport entry inside a MetricsFrame. */
export interface TransportMetricsEntry {
  transport: TransportType;
  metrics: TransportMetrics;
}

/** Retry-queue statistics frame entry. */
export interface RetryQueueStatsFrame {
  totalCount: number;
  readyCount: number;
  criticalPriorityCount: number;
  highPriorityCount: number;
  mediumPriorityCount: number;
  lowPriorityCount: number;
}

/** Deduplicator statistics frame entry. */
export interface DeduplicatorStatsFrame {
  totalTracked: number;
  recentTracked: number;
  capacityUsedPercent: number;
  falsePositiveRate?: number;
  mode: string;
}

/**
 * Periodic snapshot of protocol-wide counters and per-transport metrics.
 *
 * Note on precision: the counter fields here (`ackPending`, `neighborCount`,
 * every `RetryQueueStatsFrame.*Count`, `DeduplicatorStatsFrame.totalTracked`
 * / `.recentTracked`, and the `TransportMetrics.{packetsSent,
 * packetsReceived, bytesSent, bytesReceived, bandwidthBps}` fields) are
 * `u64` on the Rust side but bridge as JS `number` (f64, 53-bit mantissa).
 * Values above 2^53 silently lose precision. Realistic mobile deployments
 * do not hit this, but long-running relays that tail byte counters for
 * months should treat any single value above ~9 PB as approximate.
 */
export interface MetricsFrame {
  timestampMs: number;
  transports: TransportMetricsEntry[];
  retryQueue: RetryQueueStatsFrame;
  dedup: DeduplicatorStatsFrame;
  ackPending: number;
  neighborCount: number;
  isLocalRelay: boolean;
  currentTransport?: TransportType;
}

/** A single TransportStatus transition observed by the protocol engine. */
export interface TransportStateTelemetryEvent {
  timestampMs: number;
  transport: TransportType;
  previous: TransportStatus;
  current: TransportStatus;
}

/** Per-transport score breakdown carried by RoutingDecision (diagnostic tier). */
export interface RoutingScoreEntry {
  transport: TransportType;
  signal: number;
  proximity: number;
  bandwidth: number;
  congestion: number;
  energy: number;
  reliability: number;
  load: number;
  total: number;
}

/** A structured routing decision (superset of legacy Event::Dors* events). */
export interface RoutingDecision {
  timestampMs: number;
  phase: RoutingPhase;
  from?: TransportType;
  to?: TransportType;
  winningScore?: number;
  reasonCode?: RoutingReasonCode;
  scores: RoutingScoreEntry[];
}

/** Snapshot of local device capability at the moment of emission. */
export interface DeviceCapabilitySnapshot {
  timestampMs: number;
  batteryLevel?: number;
  isCharging: boolean;
  relayRole: RelayRole;
  /** Bitmask: 0b001 battery, 0b010 charging, 0b100 relay-role. */
  changedFields: number;
}

/**
 * Discriminated union of every telemetry record the SDK emits. New variants
 * land on `{ category: 'extension' }` at old client builds — regenerate
 * bindings to pick up typed handling.
 */
export type TelemetryRecord =
  | { category: 'protocol'; eventJson: string }
  | { category: 'mls'; eventJson: string }
  | { category: 'metricsFrame'; frame: MetricsFrame }
  | { category: 'transportState'; event: TransportStateTelemetryEvent }
  | { category: 'routingDecision'; decision: RoutingDecision }
  | { category: 'deviceCapability'; snapshot: DeviceCapabilitySnapshot }
  | { category: 'extension'; name: string; payloadJson: string };

/** Listener type for onTelemetry. */
export type TelemetryListener = (record: TelemetryRecord) => void;
