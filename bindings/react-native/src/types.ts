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
 * Protocol state, as returned by `getState()`.
 *
 * String-valued because that is what crosses the bridge: both native modules
 * resolve the name ("Stopped" / "Running" / "Paused") and `getState()` passes
 * it through unmapped. The members were numeric through v0.19.0, which made
 * every comparison wrong in one direction or the other — `state === ProtocolState.Running`
 * compared "Running" to `1` and was never true, while `state === 'Running'`
 * worked at runtime but failed to typecheck. Nothing can have depended on the
 * numbers, since no code path ever produced them.
 */
export enum ProtocolState {
  Stopped = "Stopped",
  Running = "Running",
  Paused = "Paused",
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
  /**
   * Maximum lifetime for outbox messages in milliseconds (default
   * 604800000 = 7 days, matching the app-layer presence-flush window).
   * Applied end-to-end from `ProtocolConfig.reliability.retry`: the
   * JS bridge forwards it verbatim on init (applyInitialRuntimeConfig →
   * native updateRetryConfig → Rust update_retry_config), where it bounds
   * store-and-forward outbox entries and, after a restart, prunes both
   * restored outbox entries and persisted media transfer descriptors
   * (see MediaResendRequiredEvent). Expiry is terminal and emits
   * `message_failed` with reason `"Outbox lifetime exceeded"` (capacity
   * eviction likewise emits `"Outbox capacity exceeded"`). The restore
   * refresh is bounded: an entry older than 4× this lifetime in total is
   * dropped terminally instead of re-granted a window.
   */
  outboxMaxLifetimeMs?: number;
  /**
   * Maximum time a message may wait for MLS session establishment before a
   * terminal `message_failed` event is emitted (default 7 days).
   */
  pendingMessageMaxLifetimeMs?: number;
}

export interface DedupConfig {
  /** Maximum number of message IDs to track */
  maxTrackedMessages?: number;
  /** Time to retain IDs in seconds */
  retentionTimeSecs?: number;
}

/**
 * Mesh forwarding tunables, as passed in at construction.
 *
 * Every field is optional: an omitted one keeps the Rust core's default. That
 * is the point of the shape — a value written here would be a second copy of a
 * default that can drift from the core's, and this section has no runtime
 * update path to correct it. Durations are milliseconds.
 */
export interface MeshRelayConfig {
  /** Hop budget a forwarded frame is clamped to (default: 8) */
  maxTtl?: number;
  /** Hop budget applied once the neighborhood is dense (default: 5) */
  denseMaxTtl?: number;
  /** Neighbor count at which the dense budget applies (default: 6) */
  denseDegree?: number;
  /**
   * Neighbors a frame is forwarded to (default: 3). Must be at least 1 —
   * zero is refused at construction, because it is not a cheaper forward but
   * a silent drop. Use `relay.allowRelay` to stop carrying traffic.
   */
  fanout?: number;
  /** Shortest pre-transmit delay in ms (default: 20) */
  jitterMinMs?: number;
  /** Longest pre-transmit delay at low density in ms (default: 200) */
  jitterMaxMs?: number;
  /** Sustained forwarding rate, frames per second (default: 10) */
  ratePerSec?: number;
  /** Burst allowance above the sustained rate (default: 30) */
  burst?: number;
  /** Sustained per-neighbor acceptance rate (default: 5) */
  peerRatePerSec?: number;
  /** Per-neighbor burst allowance (default: 15) */
  peerBurst?: number;
  /** Maximum forwards awaiting transmission (default: 256) */
  queueCapacity?: number;
  /**
   * Smallest share of the full forwarding effort capability bias scales a
   * device down to (default: 0.25). Must be in (0, 1]; `1.0` disables bias so
   * every device forwards as eagerly as every other, whatever its battery.
   */
  biasMinScale?: number;
  /**
   * Longest extra pre-transmit delay bias adds to a weaker device, in ms
   * (default: 400). This plus `jitterMaxMs` must stay under the 5s overdue
   * cut-off, past which a forward is abandoned rather than merely late.
   */
  biasMaxHandicapMs?: number;
  /** How long a stretch of forwarding activity is measured over, in ms (default: 60000) */
  activityWindowMs?: number;
  /** Frames carried in one window at or above which this device reads as an active relay (default: 3) */
  activityMinForwards?: number;
  /** Consecutive quiet windows before an active relay reads as inactive (default: 2) */
  activityIdleWindows?: number;
}

/**
 * The mesh forwarding tunables actually in force.
 *
 * The read side of {@link MeshRelayConfig}, with every field populated. It is
 * required rather than optional on purpose: an optional answer would make each
 * caller invent its own fallback, and those fallbacks are duplicated defaults
 * that drift from the core.
 */
export interface MeshRelayTunables {
  maxTtl: number;
  denseMaxTtl: number;
  denseDegree: number;
  fanout: number;
  jitterMinMs: number;
  jitterMaxMs: number;
  ratePerSec: number;
  burst: number;
  peerRatePerSec: number;
  peerBurst: number;
  queueCapacity: number;
  biasMinScale: number;
  biasMaxHandicapMs: number;
  activityWindowMs: number;
  activityMinForwards: number;
  activityIdleWindows: number;
}

/**
 * What this device has been carrying for other people.
 *
 * Counters are cumulative for the lifetime of the protocol instance and never
 * reset, so a rate is a difference between two reads. The one exception is
 * `awaitingTransmission`, which is a gauge: it is the queue depth right now
 * and goes down as well as up. See `getMeshRelayStats()`.
 */
export interface MeshRelayStats {
  /**
   * Messages moved on someone else's behalf, counted once each. This is the
   * contribution figure to show a user.
   */
  forwarded: number;
  /**
   * Times a frame was put on a link, counting each link separately and
   * including this device handing over its own messages. This is what the
   * per-second budget bounds, so it is the one to compare against the ceiling.
   */
  transmissions: number;
  /** Forwards admitted to the queue */
  queued: number;
  /** Forwards queued and not yet transmitted */
  awaitingTransmission: number;
  /** Copies of a frame already being handled, dropped on arrival */
  duplicatesSuppressed: number;
  /**
   * Pending forwards stood down because a neighbor was heard carrying the same
   * frame first. This is the mesh working as intended, not a loss.
   */
  coveredByANeighbor: number;
  /** Frames refused because one neighbor was sending more than its share */
  peerRateLimited: number;
  /**
   * Frames refused because the pending queue was full, on arrival or on the
   * way back to it. Unlike `rateDeferred` this is a real loss: the frame
   * reached nobody, and only a copy behind it or the sender's own retry will
   * carry it now. Rising means forwards arrive faster than this device can
   * transmit them.
   */
  refusedQueueFull: number;
  /** Forwards delayed by the per-second ceiling. Delayed, not dropped. */
  rateDeferred: number;
  /**
   * Queued forwards given up on after waiting too long past their due time.
   * The other end of `rateDeferred`: deferral is free to look healthy while
   * this climbs, so it is the pair that says whether back-pressure is costing
   * anything.
   */
  abandonedOverdue: number;
  /** Frames arriving with no hop budget left */
  hopLimitReached: number;
  /** Frames whose TTL was clamped down to the mesh's own hop budget */
  reachClamped: number;
  /**
   * Suppression-cache entries evicted for capacity. Expected to stay at zero;
   * anything else means this device is seeing more traffic than it can
   * remember having handled.
   */
  droppedForCapacity: number;
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

/**
 * Relay priority.
 *
 * The legacy `'low' | 'medium' | 'high'` spelling of the same three values is
 * still accepted on input and maps to `never` / `auto` / `always`.
 */
export type RelayPriority = 'never' | 'auto' | 'always';

/**
 * Whether this device carries other people's traffic, and under what
 * conditions it takes the relay role.
 *
 * The battery-dependent parts (`minBatteryForRelay`, and the charging
 * exemption) need a battery feed to do anything — call `setBatteryState` on
 * start and on each platform battery notification, or the device runs in the
 * unknown-level branch and stays willing to relay at any charge.
 */
export interface RelayConfig {
  /** Allow device to act as relay (default: true) */
  allowRelay?: boolean;
  /** Minimum battery level for relaying; charging devices are exempt (default: 30) */
  minBatteryForRelay?: number;
  /** Preferred relay behavior (default: 'auto') */
  relayPriority?: RelayPriority;
}

export interface NetworkConfig {
  /** Initial TTL (time-to-live) */
  initialTtl?: number;
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
  /**
   * Seal outgoing Nostr frames into NIP-59 gift wraps (default: true).
   *
   * With sealing on, a relay sees only an opaque routing tag, an ephemeral
   * per-event key, and ciphertext. With it off, the transport publishes the
   * legacy kind-4 event whose content is the entire protocol envelope in
   * cleartext — both usernames, the app id, the metadata map, the content type
   * and a millisecond timestamp, readable by every relay, permanently.
   *
   * Send-side only: inbound gift wraps are always unsealed and inbound legacy
   * frames always parsed, so nothing a peer sends you becomes unreadable
   * whatever this is set to.
   *
   * It does affect outbound reachability against builds that predate sealing.
   * Those subscribe to kind 4 only, so they are never handed a kind-1059 event
   * and cannot unseal one — leaving this on makes such a peer unreachable over
   * Nostr (visibly: no ACK, so the send fails through the retry ladder). Turn
   * it off to reach one, or for a relay that rejects kind 1059.
   */
  sealingEnabled?: boolean;
  /**
   * Publish MLS key packages to the relays, and resolve peers' published
   * packages (default: true).
   *
   * Buys **cold first contact**: a peer known only by username becomes
   * reachable over Nostr with no prior key-package exchange over some other
   * transport, which is otherwise impossible.
   *
   * The cost is that this is the first thing the transport emits unprompted.
   * A small set of addressable records sits at this install's routing tag and
   * is refreshed as key packages are consumed, whether or not you ever send a
   * message. Their *content* is sealed — necessary, because an MLS key package
   * carries its owner's username in the leaf credential and cannot be stripped
   * of it — so a relay scraping by event kind reads nothing. But the existence
   * of a record at a given tag, and the timing of its refreshes, are visible to
   * every relay you publish to.
   *
   * Turn it off to keep the transport silent until it has traffic.
   */
  coldContactEnabled?: boolean;

  /**
   * Publish a username discovery record for this install's profile, and allow
   * `resolveUsername()` to look names up.
   *
   * **Off by default**, unlike `coldContactEnabled`, and it additionally
   * requires cold contact to be on: a claim points at an address whose key
   * packages a resolver fetches next, so without them the name resolves and
   * then dead-ends one hop later.
   *
   * Buys back reach-by-username: a stranger who knows only a name can find the
   * addresses claiming it. The name published is the app's configured
   * `profile`, normalized to NFC and lowercase.
   *
   * **Default-off is deliberate, and the reason is disclosure.** Publishing
   * binds a human-readable name to an address in a public place — here the
   * mapping *is* the payload, which is materially more than the key-package
   * record's "an install with this tag exists". The record is sealed, so a
   * relay scraping by kind reads nothing, but anyone who guesses the name can
   * compute the tag and read the claim.
   *
   * **The directory is not authoritative.** Anyone may claim any name, so a
   * resolution returns the whole set of claimants and a human must confirm out
   * of band. See {@link UsernameResolvedEvent}.
   */
  usernameDiscoveryEnabled?: boolean;
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
  Poll = 'poll',
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
  /**
   * Stable media identifier assigned by the application.
   *
   * Note: this and the fields below are surfaced on received messages;
   * the end-to-end-sealed rich send surface (v0.16.0) is what populates
   * them on the sending side.
   */
  mediaId?: string;
  /** URL to fetch the full media from (cloud-stored media). */
  downloadUrl?: string;
  /** URL to fetch a thumbnail from (cloud-stored media). */
  thumbnailUrl?: string;
  /**
   * Content-encryption key for cloud-stored media (base64). Secret
   * material: only ever carried inside end-to-end-encrypted payloads —
   * the SDK's wire chokepoint strips it from any cleartext frame.
   */
  encryptionKey?: string;
  /**
   * Initialization vector for the cloud-media content encryption (base64).
   * Secret material — same handling as `encryptionKey`.
   */
  iv?: string;
  /** Integrity hash of the encrypted cloud-media blob (base64). */
  ciphertextHash?: string;
  /** Sticker pack provider (sticker messages). */
  stickerProvider?: string;
  /** Provider-scoped sticker identifier (sticker messages). */
  stickerRemoteId?: string;
  /** Sticker rendering kind (e.g. "static", "animated", "lottie"). */
  stickerKind?: string;
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
   * Require encrypted delivery (default: true).
   * When true, sends fail closed if encryption cannot be applied — a node
   * that never initialized MLS errors instead of silently sending plaintext,
   * and inbound legacy plaintext media is rejected. Set to false only to
   * deliberately operate in plaintext; each plaintext send then emits a
   * `PLAINTEXT_SEND` security warning (once per peer).
   */
  requireEncryption?: boolean;
  /**
   * Kill switch for the compact MLS envelope on encrypted messages
   * (default: true). Negotiated per recipient via the key package; parsing
   * of inbound compact envelopes is always on. Disabling stops advertising
   * and emitting, so both directions fall back to the legacy JSON envelope
   * without an SDK release if a field interop issue ever surfaces. The
   * end-to-end sibling of `binaryWireEnabled` on `ProtocolConfig`.
   */
  compactEnvelopeEnabled?: boolean;
  /**
   * Kill switch for the sealed rich payload on encrypted messages
   * (default: true): quoted-reply context, rich media metadata, and forward
   * attribution sealed inside the MLS ciphertext. Negotiated per recipient
   * via the key package; parsing of inbound sealed bodies is always on.
   * Disabling stops advertising and sealing, so rich extras degrade to
   * being dropped — never sent cleartext. Independent of
   * `compactEnvelopeEnabled`.
   */
  richPayloadEnabled?: boolean;
  /**
   * Kill switch for 1:1 MLS crypto-failure recovery (default: true). An
   * undecryptable DM or media chunk is not delivery-ACKed, so the sender keeps
   * retrying — and each DM resend is re-sealed against the peer's current
   * session — instead of being told "delivered" for a message that was
   * dropped. Only an epoch mismatch (the two sides disagreeing on the MLS
   * epoch, e.g. after a fork) additionally triggers a rate-limited session
   * re-key to heal the channel; failures that are *not* an epoch mismatch
   * (AEAD/authentication failures, discarded ratchet generations, malformed
   * frames) withhold the ACK but never re-key. Disabling reverts to the legacy
   * drop-and-ACK behavior.
   *
   * Plan for one consequence: `messageDecryptionFailed` is **advisory**, not
   * terminal, and fires once per failed *attempt* rather than once per message.
   * Read it as "this attempt did not decrypt"; `messageFailed` (or
   * `fileReceiveFailed` for media) remains the terminal signal.
   *
   * The re-key trigger is unauthenticated by construction: an MLS epoch is
   * checked during framing validation, before the sender is verified, so any
   * party able to inject a frame can drive a re-key — no key material or
   * captured ciphertext needed. It is safe because it is bounded, not because
   * it is trusted: the re-key is confined to that peer's own session slot,
   * limited to one per peer per 30s, discards no queued message, and raises a
   * `SESSION_REKEY_TRIGGERED` security warning — so a sustained rate (injection
   * rather than a real fork) is visible to your app.
   */
  cryptoRecoveryEnabled?: boolean;
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
  /** Pending message TTL in milliseconds (default: 1800000 — 30 min) */
  pendingTtlMs?: number;
  /** Overflow behavior when limits are hit (default: 'drop_oldest') */
  overflowPolicy?: OverflowPolicy;
}

/**
 * Protocol configuration
 */
/**
 * Replicated-document data layer configuration.
 *
 * Absent leaves every value at its core default — the defaults live in one
 * place, in Rust, and this bridge does not restate them.
 */
export interface DataConfig {
  /**
   * Whether the data layer accepts work (default: false).
   *
   * Off until the replication half ships: a capability advertised with no
   * sync behind it invites peers to expect a sync that never comes.
   */
  enabled?: boolean;
}

export interface ProtocolConfig {
  /** Application identifier */
  appId: string;
  /**
   * Local profile selector: which stored identity this instance runs as.
   *
   * Never leaves the device and is not this device's identity. It picks a
   * storage namespace — one per `(appId, profile)` — so an app hosting several
   * accounts gives each its own value, and an app hosting one can pass a
   * constant such as `'default'`.
   *
   * The wire identity is the self-certifying address derived from the identity
   * key in that namespace: read it with `localAddress()` or from the
   * `identity_ready` event. An app cannot choose it.
   */
  profile: string;
  /** Transport configuration (optional) */
  transports?: TransportsConfig;
  /**
   * Kill switch for the compact binary wire codec on mesh hops
   * (default: true). Negotiated per peer via the key package; decoding of
   * inbound binary frames is always on. Disabling stops advertising and
   * emitting, so both directions fall back to JSON framing without an SDK
   * release if a field interop issue ever surfaces. The hop-local sibling
   * of `encryption.compactEnvelopeEnabled`.
   */
  binaryWireEnabled?: boolean;
  /** File transfer configuration (optional) */
  fileTransfer?: FileTransferConfig;
  /**
   * Encryption configuration (optional).
   * Defaults to encryption enabled with auto key exchange.
   */
  encryption?: EncryptionConfig;
  /** Group messaging configuration (optional). */
  group?: {
    /** Maximum members allowed in a single group (default: 256). */
    maxGroupMembers?: number;
    /**
     * Whether groups register with the relay server (default: true).
     * Registration is what invite links resolve against — leave it on
     * unless the app never uses relay group features.
     */
    relayEnabled?: boolean;
    /**
     * Whether a relay-synced group may send one O(1) relay broadcast
     * instead of per-member fan-out (default: true). The flag alone never
     * selects the broadcast: the path additionally requires the connected
     * relay to advertise the `group_delivery_v3` capability, whose settled
     * per-recipient delivery report is what gives the broadcast a delivery
     * contract (members the relay did not reach are re-sent per-member
     * automatically — see the `group_message_delivery_report` event). Set
     * false to force per-member fan-out even against a capable relay.
     */
    relayBroadcastEnabled?: boolean;
    /**
     * Whether an incoming MLS membership commit is REFUSED when the local
     * admin overlay does not authorize its committer (default: false).
     *
     * Leaving this off does not mean unauthorized changes go unnoticed —
     * they are applied and reported via
     * `group_unauthorized_membership_change`, and the roster events carry
     * `authorized: false`. This flag only decides whether the commit is
     * *also* refused.
     *
     * Turning it on is a decision about partition risk, not a hardening
     * tweak. Refusing a commit means declining the MLS merge, so this
     * device's epoch stays behind every member that accepted it and MLS
     * cannot heal that — the app has to re-invite. The check fails open on
     * absent knowledge (no roles stored yet) but cannot detect *divergent*
     * admin views, so enable it only for a closed deployment that controls
     * role distribution, and never on part of a fleet.
     */
    enforceAdminCommits?: boolean;
  };
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
    /**
     * Battery level at or below which DORS treats the device as low on power,
     * penalising high-power transports and favouring low-power ones
     * (default: 20). Requires a battery feed — see `setBatteryState`.
     */
    lowBatteryThreshold?: number;
    /**
     * Battery level below which DORS stops treating BLE as a viable relay
     * carrier and escalates; charging devices are exempt (default: 30).
     */
    relayMinBatteryLevel?: number;
    /** Connection count DORS scores relay capacity against (default: 4) */
    relayOptimalConnectionCount?: number;
  };
  /**
   * Mesh forwarding tunables (optional).
   *
   * The shape of carrying other people's traffic, once `relay.relayPriority`
   * and the battery floors have already decided that this device does. Whether
   * to forward at all is `relay.allowRelay`'s job, not this section's.
   *
   * Every field is optional and an omitted one keeps the SDK default — the
   * defaults live in the Rust core and nowhere else, so no value here is ever
   * a restatement that can drift. Applied at construction only; there is no
   * runtime update for this section. Read what is actually in force with
   * `getMeshRelayTunables()`.
   */
  meshRelay?: MeshRelayConfig;
  /** Relay configuration (optional) */
  relay?: RelayConfig;
  /** Network configuration (optional) */
  network?: NetworkConfig;
  /** Reliability configuration (optional) */
  reliability?: ReliabilityConfig;
  /**
   * Replicated-document data layer (optional).
   *
   * Every field is optional and an omitted one keeps the SDK default — the
   * defaults live in the Rust core and nowhere else.
   */
  data?: DataConfig;
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
 * Quoted-reply context for rendering a reply preview without a local copy
 * of the original message.
 *
 * **Trust model:** This is a display-level hint copied by the sending
 * client, not a cryptographic proof. UI layers should not rely on it for
 * access-control or security decisions.
 */
export interface ReplyContext {
  /** Sender of the message being replied to. */
  sender: string;
  /** Text (or excerpt) of the message being replied to. */
  text: string;
  /** Timestamp of the quoted message (wall-clock ms). */
  timestamp?: number;
  /** Short human-readable label for quoted media (e.g. a file name). */
  reply_media_label?: string;
  /** Content type of the quoted message (e.g. "image"). */
  reply_content_type?: string;
}

/**
 * Media metadata as carried on received events (snake_case, mirroring the
 * event JSON — distinct from the camelCase `MediaMetadata` used as native
 * module input).
 *
 * `encryption_key`/`iv` are secret material: the SDK strips them from every
 * cleartext wire frame and redacts them from telemetry, so they only ever
 * arrive here via the end-to-end-sealed media envelope.
 */
export interface MediaMetadataEvent {
  mime_type?: string;
  file_name?: string;
  file_size?: number;
  duration_ms?: number;
  width?: number;
  height?: number;
  thumbnail_base64?: string;
  media_id?: string;
  download_url?: string;
  thumbnail_url?: string;
  encryption_key?: string;
  iv?: string;
  ciphertext_hash?: string;
  sticker_provider?: string;
  sticker_remote_id?: string;
  sticker_kind?: string;
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
  /**
   * Content type hint stamped on the outer message (optional, defaults to
   * text). A coarse rendering hint — the content itself stays MLS-sealed.
   * Must not be `file_chunk` (an internal transport content type); the SDK
   * rejects it as InvalidArgument.
   */
  contentType?: ContentType;
  /**
   * Quoted-reply context (optional). Only ever delivered inside the
   * MLS-sealed rich payload, and only to recipients whose SDK advertised
   * support; toward anyone else it is silently dropped — never sent
   * cleartext.
   */
  replyContext?: ReplyContext;
  /**
   * Rich media metadata (optional) — cloud attachments, stickers, including
   * any `encryptionKey`/`iv` secrets. Sealed-only, like `replyContext`.
   */
  mediaMetadata?: MediaMetadata;
  /** Forward attribution (optional). Sealed-only, like `replyContext`. */
  forwardInfo?: ForwardInfo;
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
  /**
   * Optional first message shown with the request. Delivered in the
   * recipient's connection_request_received event; like the sender name it
   * travels in plaintext (connection requests precede the MLS session).
   */
  initialMessage?: string;
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
  /**
   * Caption text. Travels sealed inside the chunk-0 MLS ciphertext toward
   * recipients that advertised rich payload support and is silently dropped
   * otherwise — never sent cleartext.
   */
  caption?: string;
  /** ID of the message this media replies to. Sealed-only, like caption. */
  replyToMsg?: string;
  /** Quoted-reply context. Sealed-only, like caption. */
  replyContext?: ReplyContext;
  /** Forward attribution. Sealed-only, like caption. */
  forwardInfo?: ForwardInfo;
  /**
   * Caller-supplied file id for the transfer (minted when absent). Must not
   * collide with an active outbound transfer; max 4096 bytes.
   */
  fileId?: string;
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
  /**
   * `true` when the content arrived MLS-encrypted and was auto-decrypted;
   * `false` for plaintext accepted under the `requireEncryption: false`
   * opt-out. Always present on the wire since the inbound plaintext gate
   * landed; optional here for compatibility with older SDK cores.
   */
  encrypted?: boolean;
  /** ID of the message this is replying to (optional). */
  reply_to_msg?: string;
  /** Quoted-reply context (present when this message quotes another). */
  reply_context?: ReplyContext;
  /** The type of content (text, image, video, voice_note, etc.). */
  content_type?: string;
  /** Media metadata (present for non-text content). */
  media_metadata?: MediaMetadataEvent;
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
 * Machine-readable decryption failure codes. `PENDING_QUEUE_DROPPED` means the
 * message was dropped from the pending-decryption queue (overflow or TTL
 * expiry) before the sender's session became ready; it was ACKed on receipt,
 * so the sender will not retransmit it.
 */
export type DecryptionFailureCode =
  | 'INVALID_PAYLOAD'
  | 'NOT_INITIALIZED'
  | 'INVALID_CIPHERTEXT'
  | 'IDENTITY_MISMATCH'
  | 'CRYPTO_FAILURE'
  | 'PENDING_QUEUE_DROPPED'
  | 'UNKNOWN';

/**
 * Failed to decrypt an inbound encrypted message.
 */
export interface MessageDecryptionFailedEvent extends BaseEvent {
  type: 'message_decryption_failed';
  message_id: string;
  sender: string;
  code: DecryptionFailureCode;
  reason: string;
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
/**
 * This device started acting as a relay — carrying traffic for other devices
 * that cannot reach each other directly.
 *
 * Reports observed behaviour, not capability: it fires because the device has
 * been forwarding, so it needs no battery feed and `battery_level` is null on
 * a device that has never reported one.
 */
export interface RelayPromotedEvent extends BaseEvent {
  type: 'relay_promoted';
  connection_count: number;
  battery_level: number | null;
}

/**
 * Relay demoted event
 */
export interface RelayDemotedEvent extends BaseEvent {
  type: 'relay_demoted';
  reason: string;
}

/**
 * Relay role was demoted due to battery constraints.
 */
export interface RelayDemotedBatteryEvent extends BaseEvent {
  type: 'relay_demoted_battery';
  battery_level: number;
  min_required: number;
}

/**
 * Neighbor discovered event
 */
export interface NeighborDiscoveredEvent extends BaseEvent {
  type: 'neighbor_discovered';
  /**
   * The peer's canonical address (`off1…`) — the value that peer derived
   * from its own identity key, which it reports as its `localAddress()`.
   * This is the one identifier the SDK uses on every surface: use it
   * directly as `recipient` in `sendMessage` and `sendConnectionRequest`,
   * regardless of which transport discovered the peer.
   */
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
 * This device's own address is known.
 *
 * Fires once per successful startup, before any message can be sent. The
 * address is derived from the identity key held in this profile's storage —
 * the app does not choose it. It is this device's `sender` on every outbound
 * frame and the string peers must use as `recipient` to reach it, and it is
 * stable across restarts of the same `profile`.
 *
 * Also readable at any time after startup via `localAddress()`.
 */
export interface IdentityReadyEvent extends BaseEvent {
  type: 'identity_ready';
  /** This device's self-certifying address (`off1…`). */
  address: string;
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
  media_metadata?: MediaMetadataEvent;
  /** Base64-encoded reassembled file data. */
  file_data: string;
  /**
   * When the sender queued the transfer (chunk-0 message timestamp,
   * wall-clock ms) — for display ordering alongside
   * `MessageReceivedEvent.timestamp`.
   */
  timestamp?: number;
  /** Caption text from the sealed chunk-0 rich extras. */
  caption?: string;
  /** ID of the message this media replies to (sealed chunk-0 extras). */
  reply_to_msg?: string;
  /** Quoted-reply context (sealed chunk-0 extras). */
  reply_context?: ReplyContext;
  /** Forwarding attribution (sealed chunk-0 extras). */
  forward_info?: ForwardInfo;
}

/**
 * An inbound file transfer was dropped before completion — the receiver hit
 * a resource limit (too many concurrent transfers, per-sender quota, or the
 * buffered-bytes budget), the reassembled file failed its integrity checks,
 * or the transfer went stale (no chunks within the stale timeout). Terminal
 * and fired at most once per transfer: the failed transfer's remaining
 * in-flight chunks are dropped silently. No `file_received` will follow for
 * this `file_id`; the sender must re-send the file (under a fresh `file_id`)
 * to retry.
 */
export interface FileReceiveFailedEvent extends BaseEvent {
  type: 'file_receive_failed';
  file_id: string;
  file_name: string;
  sender: string;
  /**
   * Machine-readable reason: 'too_many_transfers' | 'sender_quota_exceeded'
   * | 'buffer_budget_exhausted' | 'integrity_check_failed' | 'stale_timeout'
   */
  reason: string;
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
 * An outbound media transfer was aborted before all chunks were delivered
 * (chunk encryption failed, or a chunk failed terminally). No `media_sent`
 * will follow for this `file_id`; retry with a new `sendMedia` call.
 */
export interface MediaSendFailedEvent extends BaseEvent {
  type: 'media_send_failed';
  file_id: string;
  recipient: string;
  reason: string;
}

/**
 * A pending ACK was evicted due to capacity constraints.
 */
export interface AckEvictedEvent extends BaseEvent {
  type: 'ack_evicted';
  message_id: string;
  priority: string;
  reason: string;
}

/**
 * A fragment assembly was evicted to make room for new fragments.
 */
export interface FragmentAssemblyEvictedEvent extends BaseEvent {
  type: 'fragment_assembly_evicted';
  message_id: string;
  /** Completion percentage (0-100) when evicted. */
  completion_percent: number;
  reason: string;
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
 * Convergence diagnostic event (1:1 MLS convergence instrumentation).
 * Pure receiver-side breadcrumb for the Welcome receive / adopt / confirm
 * path — carries no protocol effect. Surfaces stages the receiver otherwise
 * emits nothing for (welcome reassembled, branch taken, decrypt landed).
 */
export interface ConvergenceDiagEvent extends BaseEvent {
  type: 'convergence_diag';
  /** Fixed stage label, e.g. 'welcome_received' | 'welcome_branch' | 'decrypt_success' */
  stage: string;
  /** Peer ID this breadcrumb concerns */
  peer_id: string;
  /** Free-form key=value context */
  detail: string;
}

/**
 * Machine-readable reason codes for welcome delivery failures.
 */
export type WelcomeReasonCode =
  | 'TRANSPORT_UNAVAILABLE'
  | 'PEER_UNREACHABLE'
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
 * Welcome send failed event.
 *
 * Note: `welcome_send_succeeded → welcome_send_failed` is a LEGAL sequence
 * for the same welcome over the internet transport. The bridge confirms on
 * socket-write success, but the relay stores nothing for offline
 * recipients — its later `DeliveryError` corrects the earlier success
 * (reason_code `PEER_UNREACHABLE`, `retryable: true`). Recovery is
 * automatic: the SDK keeps an escalating reachability probe running on
 * every carrier (a `PEER_UNREACHABLE` failure therefore always carries
 * `next_retry_at`), with presence-driven re-send as the faster edge when
 * the platform polls presence. This event may repeat once per probe round
 * while the peer stays offline; treat it as state, not as a terminal
 * verdict.
 */
export interface WelcomeSendFailedEvent extends BaseEvent {
  type: 'welcome_send_failed';
  peer_id: string;
  message_id: string;
  group_id: string;
  attempt: number;
  reason_code: WelcomeReasonCode;
  /**
   * Stable classification of the transport failure, when one was recorded —
   * never the rendered error. Both feeders named an identifier (the relay's
   * `DeliveryError` prose, and a local transport error that interpolates the
   * peer), while `peer_id` and `group_id` beside it are hashed.
   */
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
  /** First message sent along with the request, if any. */
  initial_message?: string;
}

/**
 * An outbound connection request could not be delivered: the transport
 * reported the recipient unreachable (e.g. the relay's DeliveryError for an
 * offline peer), or the request exhausted its retry budget (a generic
 * `message_failed` also fires for the same id in that case). Correlate via
 * the message id returned by sendConnectionRequest. This is a status
 * signal, not proof of permanent failure — the original request may still
 * be delivered by the retry machinery if the peer comes back online, so
 * treat a user-initiated resend as potentially duplicating the original on
 * the recipient's side.
 */
export interface ConnectionRequestUndeliverableEvent extends BaseEvent {
  type: 'connection_request_undeliverable';
  recipient: string;
  message_id: string;
  /**
   * Failure reason, drawn from a fixed local vocabulary:
   * `recipient_unreachable` (transport-level offline signal),
   * `max_retries_exceeded` (retry budget exhausted),
   * `outbox_lifetime_exceeded` (aged out of the store-and-forward outbox),
   * or `outbox_capacity_exceeded` (evicted when the outbox hit capacity).
   * The relay's wording is no longer appended to the first of these.
   */
  reason: string;
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
  /**
   * Media metadata restored from the sealed body of a rich group message
   * (cloud attachments — including the `encryption_key`/`iv` needed to
   * decrypt cloud media, which only ever travel MLS-sealed). Absent on
   * plain group messages.
   */
  media_metadata?: MediaMetadataEvent;
  /** Content-type rendering hint from the sealed body (text, image, ...). */
  content_type?: string;
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
  /**
   * Whether `added_by` was authorized to make this change.
   *
   * `false` means the change **did** happen — MLS accepted the commit and
   * the roster really has changed — but the committer was not a known
   * admin. See `GroupUnauthorizedMembershipChangeEvent`.
   *
   * **Absent means "not evaluated"**, not "authorized": your own join from
   * a Welcome (there is no prior group state to judge the inviter against),
   * relay reconciliation frames (no authenticated committer to judge), and
   * events from an older core all omit the field. Only a present value is
   * a positive statement either way.
   *
   * Judged against this device's local replica of role state, which
   * replicates best-effort and can lag — a legitimate change may be flagged
   * `false`, and different members can disagree. Do not act on it
   * automatically.
   */
  authorized?: boolean;
}

/**
 * Group member removed event (from relay)
 */
export interface GroupMemberRemovedEvent extends BaseEvent {
  type: 'group_member_removed';
  group_id: string;
  user_id: string;
  removed_by: string;
  /**
   * See `GroupMemberAddedEvent.authorized` (absent = not evaluated). On the
   * relay reconciliation path the judgment applies to the frame's
   * authenticated wire sender, which is not necessarily the `removed_by`
   * named here.
   */
  authorized?: boolean;
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
 * Stable SDK projection of a GroupInfo relay frame.
 *
 * The same frame is also emitted as `internet_server_message`; use that raw
 * event for application-owned fields such as descriptions, avatars, pending
 * join requests, or future server extensions. Do not apply standard group
 * state from both events, and do not rely on their arrival order.
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
 * Stable SDK projection of a UserGroups relay frame.
 *
 * The same frame is also emitted as `internet_server_message`; use that raw
 * event for profile, membership, or future application-owned fields. Do not
 * apply standard group state from both events, and do not rely on their
 * arrival order.
 */
export interface UserGroupsEvent extends BaseEvent {
  type: 'user_groups';
  groups: UserGroupSummaryEvent[];
}

/**
 * Group error event (from relay).
 *
 * `reason` is a **fixed code minted locally**, not the relay's wording:
 * `not_found` (the relay has no such group — invite links and relay fan-out
 * for it are dead, not merely refused), `sync_denied` (the relay refused to
 * register or sync the group for this caller), or `error` (any other
 * refusal). Do not parse it as prose or show it to users as-is.
 *
 * The relay's text is deliberately not carried here: `__GROUP_ERROR__` is
 * accepted unsigned on the relay ingest path, so its wording is chosen by
 * whoever put the frame on that socket. Apps that genuinely need the exact
 * wording — invite-link flows correlating by `request_id`, for instance —
 * should read the raw `GroupError` frame, which both bridges still dual-emit
 * on the server-message channel.
 *
 * `group_id` is present when the relay scoped the error to a group.
 */
export interface GroupErrorEvent extends BaseEvent {
  type: 'group_error';
  reason: 'not_found' | 'sync_denied' | 'error';
  group_id?: string;
}

/**
 * The relay-side registration state of a group changed.
 *
 * `synced: true` fires only on the relay's positive registration
 * acknowledgment (including the idempotent re-sync ack after a membership
 * change) — the signal that relay-dependent server commands for the group
 * (invite links via `sendRawServerCommand`, server-side fan-out) can be
 * issued. Await it via `ensureGroupRegistered`. `synced: false` fires when
 * that trust is revoked; `reason` says why: `registered` (relay ack),
 * `error` (group-scoped relay error — details arrive separately as
 * `group_error` / the raw `GroupError` frame), `removed` (we were removed
 * from the group), `left` (local leave), `internet_dropped` (transport
 * lost; re-registration re-arms on reconnect), `ack_timeout` (the relay
 * never answered — likely a relay without group support). Not emitted for
 * groups the relay was never asked about.
 */
export interface GroupRelaySyncChangedEvent extends BaseEvent {
  type: 'group_relay_sync_changed';
  group_id: string;
  synced: boolean;
  reason: string;
}

/**
 * A decoded and verified invite.
 *
 * Every field has passed verification: the address is the one its public key
 * derives to, and when `signed` is true the petname is bound to that key by
 * the key's owner.
 */
export interface InviteInfo {
  /** The address this invite reaches, canonical `off1…`. */
  address: string;
  /** The Ed25519 identity key the address derives from. */
  public_key: number[];
  /**
   * Suggested display name, if the invite carried one.
   *
   * Suggested, never authoritative: a petname is a *locally assigned* name and
   * an app is right to let the user edit it before saving.
   */
  petname: string | null;
  /**
   * Whether a valid signature accompanied the invite.
   *
   * `false` does **not** mean untrustworthy — an unsigned invite is the
   * ordinary shape for a QR shown phone to phone, where the physical channel
   * is the authentication. It means only that the petname is unbound to the
   * key, so a forwarded copy could carry a different name.
   */
  signed: boolean;
}

/**
 * One device's verified claim to a username.
 *
 * Every field here has already passed verification: the address derives from
 * the public key, the record's signature verifies under that key, and the
 * record was published under the Nostr key it names. What that proves is
 * narrow: **this key asserts this name**. It does not prove the name belongs
 * to the claimant, because nothing can.
 */
export interface UsernameClaim {
  /**
   * The claimed address, canonical `off1…`.
   *
   * This is the value to keep. An app that stores the *name* has stored
   * something anyone can re-claim tomorrow; the address is self-certifying.
   */
  address: string;
  /** The Ed25519 identity key the address derives from, base64. */
  public_key: string;
  /**
   * When the claimant signed the record, in milliseconds since the epoch.
   *
   * **Advisory.** A record is not a liveness signal. An old claim from a peer
   * who has been offline for a month is still valid, and filtering on age
   * would make them unreachable by name while their key packages sit live on
   * a relay. Sort by it if it helps a user choose; do not filter on it.
   */
  issued_at_ms: number;
}

/**
 * A username resolution finished, carrying **every** claim found.
 *
 * ## The set is the whole point
 *
 * Anyone may publish any username claim, so a name resolves to a set of
 * assertions, never to an answer. Even a single-device user is a set of one.
 * This event fires exactly once per resolution and carries the complete set
 * precisely so an app cannot accidentally treat the first arrival as the
 * winner: there is no per-claim event to race and no ordering to mistake for
 * a ranking.
 *
 * **Let the user choose.** Silently picking one claim converts a
 * non-authoritative directory into an authoritative-looking one, which is
 * worse than not shipping the feature: the user believes they are talking to
 * the name, and the protocol only ever promised them a key. Present the
 * claims, have a human confirm out of band (a QR code, a shared secret, a
 * voice call), and store the address rather than the name.
 *
 * An empty `claims` list is an ordinary outcome, not an error.
 */
export interface UsernameResolvedEvent extends BaseEvent {
  type: 'username_resolved';
  /** The normalized username that was resolved. */
  username: string;
  /** Every claim that verified, in no meaningful order. */
  claims: UsernameClaim[];
  /**
   * How many records were seen but rejected.
   *
   * Non-zero is normal, not an error: the tag is public, anyone may publish
   * to it, and junk arrives. Surfaced so "not found, having seen nothing" can
   * be told apart from "not found, everything was junk".
   */
  rejected: number;
  /**
   * How many *verified* claims were dropped at the accumulator's ceiling.
   *
   * The opposite statement to `rejected`: these records passed every check and
   * are missing anyway, because one name attracted more claimants than a
   * resolution will hold. Non-zero means `claims` is a sample, so an absence
   * from it proves nothing — and it is the only signal that a name is being
   * squatted at volume, which would otherwise render as a clean set.
   */
  truncated: number;
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
 * The relay's settled per-recipient delivery report for a group message
 * sent via relay broadcast, after the SDK acted on it.
 *
 * Fires once per broadcast whose report arrived, seconds after
 * `group_message_sent` (the relay reports only when its whole fan-out has
 * settled — correlate by `message_id`, never by order). Members in
 * `delivered` took the message over a live relay socket; members in
 * `pushed` took a device push carrying the ciphertext. Every other MLS
 * roster member has already been re-sent a per-member copy through the
 * ordinary delivery ladder (`missed_reissued`) — no app action is
 * required; this is delivery observability, not a failure signal. Not
 * emitted when the report itself is lost: the SDK then re-broadcasts
 * (bounded) and finally downgrades the message to per-member fan-out.
 */
export interface GroupMessageDeliveryReportEvent extends BaseEvent {
  type: 'group_message_delivery_report';
  group_id: string;
  /** The logical group message id, as returned by the send. */
  message_id: string;
  /** Members whose relay socket write was confirmed (relay-side write-ack). */
  delivered: string[];
  /** Members offline at the relay who took a device push. */
  pushed: string[];
  /** Members re-sent a per-member copy because the relay reached them neither way. */
  missed_reissued: string[];
}

/**
 * Rich media metadata was dropped from an outbound group message because
 * the group is not fully rich-capable (not every member advertised sealed
 * rich payload support, or the local kill switch disabled it). The text
 * was still sent; members receive it without the media attachment. Use
 * this to warn the sender that an attachment did not go through.
 */
export interface GroupRichExtrasDroppedEvent extends BaseEvent {
  type: 'group_rich_extras_dropped';
  group_id: string;
  /**
   * Members not known to parse the sealed rich payload — the ones holding
   * the seal gate closed (unknown and known-non-support are
   * indistinguishable). Empty when the drop was caused by the local
   * `richPayloadEnabled` kill switch instead. The SDK probes these members
   * for their capability automatically, so a later retry may stop dropping.
   */
  unknown_members: string[];
}

/**
 * An MLS membership change was made that its committer was not authorized
 * to make. Read `enforced` first — it decides what this event means.
 *
 * With `group.enforceAdminCommits` off (the default), the change **has been
 * applied** and `enforced` is `false`. MLS authenticated the committer as a
 * group member and accepted the commit. The SDK's admin model is an
 * application-layer overlay on MLS (which has no admin concept), enforced
 * when *sending*; refusing the change on receipt would mean refusing the
 * MLS merge, permanently forking this member away from everyone who
 * accepted it. So the change stands and is reported here instead.
 *
 * With that flag on, the commit was instead **refused before merging** and
 * `enforced` is `true` — nothing changed locally, and this device is now an
 * epoch behind the group. See the field doc below.
 *
 * This signal can false-positive: "unauthorized" is judged against this
 * device's local replica of role state, which replicates best-effort and
 * can lag — a legitimate change may be reported, and different members can
 * disagree about the same commit.
 *
 * Treat this as a moderation signal for a *human* admin: an admin can undo
 * it with `meshRemoveFromGroup` / `meshInviteToGroup`. Never reverse a
 * change automatically off a single member's event — corroborate first. A
 * member added this way can read all subsequent group traffic until removed.
 *
 * Reports are rate-limited per (group, committer, enforced): a repeat of the
 * same outcome within a short window is not re-emitted, but every affected
 * `group_member_added` / `group_member_removed` still carries
 * `authorized: false`.
 *
 * Known limitation: the member removed by an unauthorized Remove does not
 * receive this event — only the remaining members report the removal.
 */
export interface GroupUnauthorizedMembershipChangeEvent extends BaseEvent {
  type: 'group_unauthorized_membership_change';
  group_id: string;
  /** The MLS-authenticated committer that made the change. */
  committer: string;
  /** Members the commit added, sorted. Empty for a pure removal. */
  added: string[];
  /** Members the commit removed, sorted. Empty for a pure addition. */
  removed: string[];
  /**
   * Why the change was judged unauthorized: `'sender_not_admin'` or
   * `'affected_member_mismatch'`. Treat as opaque — values may be added.
   */
  reason: string;
  /**
   * Whether the commit was *refused* rather than applied.
   *
   * `false` (the default configuration) means the membership change
   * happened: `added` / `removed` describe real roster changes an admin can
   * undo, and the matching roster events were emitted alongside this one.
   *
   * `true` means `group.enforceAdminCommits` was on and the commit was
   * rejected before merging: no roster event accompanies this one, nothing
   * changed locally, and `added` / `removed` describe what the commit
   * *would* have done. Treat it as a partition alarm, not just a moderation
   * signal — this device declined an epoch every accepting member advanced
   * to, so it can no longer decrypt that group's traffic and has to be
   * re-invited. Re-inviting arrives as a Welcome, which is not policy-gated,
   * so it readmits this device into the group *including* whatever the
   * refused commit did; resolve that change too rather than treating the
   * re-invite as the whole remedy.
   */
  enforced: boolean;
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

/**
 * Group renamed — emitted when a rename is performed or received via
 * `meshRenameGroup`. Renames observed only as relay-native `GroupRenamed`
 * frames surface through `internet_server_message` instead.
 */
export interface GroupRenamedEvent extends BaseEvent {
  type: 'group_renamed';
  group_id: string;
  new_name: string;
  /** Previous group name, if known. */
  old_name: string | null;
  renamed_by: string;
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
 * Which channel produced a `presence_updated` event.
 *
 * - `internet`: relay-observed presence — the relay's authoritative answer
 *   to `CheckPresence` (`PresenceStatus` / `PresenceStatusWithLastSeen`),
 *   or relay-derived reachability (a `DeliveryError` naming the recipient
 *   unreachable also reports `offline` here, so a failed send can produce
 *   an `internet`-sourced offline event without any explicit query).
 * - `peer`: a peer-sent `__PRESENCE__` self-report. Transport-agnostic —
 *   it may arrive over BLE, WiFi Direct, or even relay-forwarded frames,
 *   hence "peer", not "mesh".
 *
 * Apps rendering relay-style presence UI (a direct-chat header's
 * "Online" / "Last seen …") should filter on `internet` so a nearby
 * peer's self-report can't flip a header defined as relay-observed.
 */
export type PresenceSource = 'internet' | 'peer';

/**
 * Presence updated event — one unified stream for both sources
 * (discriminated by `source`): a peer-sent `__PRESENCE__` update, or the
 * internet relay's presence answer (driven by the SDK's automatic watch
 * loop or an explicit `checkInternetPresence`).
 *
 * Emission is 1:1 with the underlying signal — the SDK never dedupes
 * unchanged statuses, so every relay answer re-emits this event even when
 * nothing changed.
 */
export interface PresenceUpdatedEvent extends BaseEvent {
  type: 'presence_updated';
  peer_id: string;
  status: PresenceStatus;
  timestamp: number;
  /**
   * When the peer was last seen (Unix ms), if the source knows it —
   * relay-sourced presence only; absent for peer-sent updates. May also be
   * absent on relay answers when the relay itself doesn't know (e.g. the
   * peer hasn't connected since the last relay restart).
   */
  last_seen_ms?: number;
  /** Which channel produced this update. */
  source: PresenceSource;
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
  /**
   * Recipient's user ID. Carried as a field so the telemetry scrubber hashes
   * it; it previously reached apps only inside the rendered `reason`.
   */
  recipient: string;
  /**
   * Stable classification of the deferral, drawn from a fixed local
   * vocabulary (`peer_not_reachable`, `transport_not_connected`,
   * `transport_send_failed`, ...). Not a rendered error — the underlying
   * message, which names the peer, stays in the device log.
   */
  reason: string;
  retry_count: number;
  next_retry_at?: number;
}

/**
 * Message retrying event — a retry was scheduled after a failed attempt
 * (transport send error or ACK timeout). Non-terminal: MessageDelivered or
 * MessageFailed still settles the message.
 */
export interface MessageRetryingEvent extends BaseEvent {
  type: 'message_retrying';
  message_id: string;
  recipient: string;
  retry_count: number;
  /** Absolute time the retry is scheduled for (Unix timestamp ms). */
  next_retry_at: number;
}

/**
 * Message undeliverable event — the transport reported the recipient
 * unreachable for an in-flight message (e.g. the internet relay's
 * DeliveryError). Non-terminal: the message stays in the outbox. A plain DM
 * is parked — its ACK retry budget stops burning while the peer is provably
 * offline — and is re-driven on reachability edges (transport reconnect,
 * peer discovery, presence-online; the SDK adds parked recipients to the
 * presence watchlist). It settles only via MessageDelivered or, at
 * outbox-lifetime expiry, MessageFailed. Media chunks are not parked and
 * keep the normal retry machinery. Fires repeatedly for the same message_id
 * while the recipient remains offline: a parked DM keeps an escalating
 * reachability probe on every carrier (15s doubling to a 600s cap, the
 * ladder shared per recipient), and each probe that reaches the relay while
 * the peer is still offline earns a fresh verdict and re-emits this event.
 * Treat it as a repeatable status signal, never as a terminal one. Note the
 * probes refresh the outbox entry's last-send timestamp, so a parked DM's
 * terminal MessageFailed arrives at the absolute outbox cap (4x the
 * configured lifetime, ~28 days on defaults) rather than the sliding ~7-day
 * window — plan pending-message UI accordingly.
 */
export interface MessageUndeliverableEvent extends BaseEvent {
  type: 'message_undeliverable';
  message_id: string;
  recipient: string;
  /**
   * Stable classification of the failure — `recipient_unreachable` on the
   * relay-verdict path this event is normally reached by. The relay's own
   * wording is no longer appended: it is remote-chosen text, and `recipient`
   * beside it is hashed for telemetry. On the internet path the bridges still
   * deliver that wording raw on the `diagnostic` channel.
   */
  reason: string;
  /** Owning media transfer when the message is a media chunk. */
  file_id?: string;
}

/**
 * Media resend required event — an outbound media transfer was in flight
 * when the previous process died. The SDK persists only the transfer
 * descriptor (never chunk bytes), so the app must re-supply the file bytes
 * via sendMedia with this file_id; they are checksum-validated against the
 * original transfer.
 */
export interface MediaResendRequiredEvent extends BaseEvent {
  type: 'media_resend_required';
  file_id: string;
  recipient: string;
  file_name: string;
  file_size: number;
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
 * Machine-readable classification for a {@link SecurityWarningEvent}. Branch on
 * this instead of parsing the human-readable `reason` string, which is for
 * logs/UI and may change between versions.
 */
export type SecurityWarningCode =
  | 'SENDER_ADDRESS_MISMATCH'
  | 'TRANSPORT_IDENTITY_MISMATCH'
  | 'CONTROL_SIGNATURE_INVALID'
  | 'UNSIGNED_CONTROL_REJECTED'
  | 'MEDIA_SENDER_GROUP_MISMATCH'
  | 'PLAINTEXT_SEND'
  | 'PLAINTEXT_RECEIVE_REJECTED'
  | 'SESSION_SENDER_GROUP_MISMATCH'
  | 'SESSION_REKEY_TRIGGERED'
  | 'NOSTR_KEY_PACKAGE_SLOT_EXHAUSTED'
  | 'PUSH_KEY_PACKAGE_POOL_EXHAUSTED'
  | 'RELAY_ADDRESS_BINDING_MISMATCH'
  | 'RELAY_ADDRESS_DECLARATION_REFUSED'
  | 'GROUP_LEAF_IDENTITY_UNPROVEN'
  | 'STALE_CONTROL_FRAME';

/**
 * A security-relevant anomaly was detected for a peer.
 *
 * `SENDER_ADDRESS_MISMATCH` has no benign reading: an address is the hash of
 * its owner's identity key, so a frame signed by anyone else re-derives to a
 * different address. A peer that reinstalls does not re-key an address — it
 * gets a *new* address, and reaches you as a new contact. Treat this as an
 * impersonation attempt, not as a peer to re-trust.
 *
 * `UNSIGNED_CONTROL_REJECTED` fires when a control frame arrives without a
 * signature. This is unconditional and not configurable: every peer running
 * this protocol signs its control traffic.
 *
 * `SESSION_REKEY_TRIGGERED` is rate-based rather than per-event: a genuine
 * epoch fork heals this way occasionally, but the frame that triggers it is
 * not authenticated (see `schedule_session_rekey`), so a sustained rate for
 * one peer indicates injected frames. Delivery is delayed, never lost.
 *
 * `PUSH_KEY_PACKAGE_POOL_EXHAUSTED` is also rate-based: each peer normally
 * gets its own single-use MLS init key, and this reports that the pool of
 * unconsumed packages hit its ceiling so one is being shared. Nothing fails —
 * it clears as packages are consumed or expire — but a sustained rate means
 * the device is accumulating advertisements to peers that never establish a
 * session.
 *
 * `RELAY_ADDRESS_BINDING_MISMATCH` has no benign reading either: the relay
 * acknowledged binding an address that is not this device's, which only a
 * broken or hostile relay can produce. `peer_id` is the foreign address it
 * echoed. The connection is not torn down — a relay that controls the socket
 * already controls what a local refusal would protect.
 *
 * `RELAY_ADDRESS_DECLARATION_REFUSED` is operational, not adversarial: the
 * relay declined the declaration, so this connection stays attributed by
 * account name. Existing conversations keep working; what breaks is
 * establishing *new* encrypted sessions over the relay, since the key-package
 * and welcome frames are identity-checked. `peer_id` is this device's own id
 * and `reason` is the relay's text, verbatim and opaque.
 *
 * `GROUP_LEAF_IDENTITY_UNPROVEN` has no benign reading, and unlike the others
 * it accuses someone the user is already in a room with. Every member of a
 * group carries an identity key whose hash *is* their address, so an honest
 * member always re-derives their own name; a member that does not was built
 * around someone else's. Emitted from four sites. Three refuse a claim as it
 * arrives — a declined group invite, a refused membership change, and a
 * declined 1:1 session Welcome — and in each the frame is dropped, so nothing
 * is delivered.
 *
 * The session case can fire while an *existing* session with that peer stays
 * live — the refusal is non-destructive — so a `secure_session_failed`
 * alongside it means "this handshake attempt failed", not "the session ended".
 *
 * The fourth site needs a different response: a roster read finding such a leaf
 * **already seated in local group state**. No wire gate can have admitted it,
 * so it was written to this device's secure store directly, or joined by a
 * build predating those gates. It is kept out of every roster and cannot speak,
 * but it holds live group secrets and reads everything sent to the group — so
 * the remedy is to leave and re-create the group, not to remove a member.
 *
 * `peer_id` is who the finding concerns, not always who to blame. On the three
 * refusal sites it is the peer that delivered the forgery — the inviter, or the
 * sender of the membership change — and it is proved: those frames are
 * signature-gated against the sender's own address. On the fourth there is no
 * delivering peer and it is this device's own id. `reason` is diagnostic text,
 * must not be parsed, and carries no identifier: the impersonated address stays
 * in the logs of the device that refused it.
 *
 * `STALE_CONTROL_FRAME` refuses a frame for what its signature says about
 * *when*: stamped too far in the past, too far ahead of this device's clock, or
 * carrying the older signing payload from a peer that has proved it can produce
 * the newer one. **Check this device's clock first.** The signed timestamp is
 * compared against it, so a device whose own clock is wrong refuses honest
 * peers in bulk, and the event stream then looks like an attack rather than a
 * local fault. Many different `peer_id`s in a short window is a clock; one
 * `peer_id` while others are fine is that peer, either replaying captured
 * frames or running a broken build. Nothing is torn down — the frame is
 * dropped, unacknowledged, and a peer whose frame was genuinely just slow
 * re-sends a fresh one.
 */
export interface SecurityWarningEvent extends BaseEvent {
  type: 'security_warning';
  peer_id: string;
  reason_code: SecurityWarningCode;
  reason: string;
}

/**
 * A user was blocked. Emitted for local UI notification only.
 */
export interface UserBlockedEvent extends BaseEvent {
  type: 'user_blocked';
  user_id: string;
}

/**
 * A user was unblocked. Emitted for local UI notification only.
 */
export interface UserUnblockedEvent extends BaseEvent {
  type: 'user_unblocked';
  user_id: string;
}

/**
 * A replicated document changed and the change reached storage.
 *
 * Emitted after the delta is durable, never before, so a UI that re-renders
 * on this event is rendering state that survives a crash.
 */
export interface DataChangedEvent extends BaseEvent {
  type: 'data_changed';
  space_id: string;
  doc_id: string;
  /** Size of the change that was persisted. */
  delta_bytes: number;
}

/**
 * A replicated document is approaching the per-document size cap.
 *
 * Fires while there is still room to act (archive, split, prune). Without
 * it the cap would be a cliff the app meets for the first time as a failed
 * write.
 */
export interface DataDocSizeWarningEvent extends BaseEvent {
  type: 'data_doc_size_warning';
  space_id: string;
  doc_id: string;
  /** Current compacted size of the document. */
  compacted_bytes: number;
  /** The cap it is measured against. */
  cap_bytes: number;
}

/**
 * A peer asked for the bytes behind an attachment reference.
 *
 * The SDK never holds blob bytes, so it cannot answer this on its own:
 * answer with `DataStore.provideAttachment`, or say you cannot with
 * `DataStore.declineAttachment`. An app that handles neither leaves the
 * asking peer with a reference it can see and never open.
 */
export interface DataAttachmentRequestedEvent extends BaseEvent {
  type: 'data_attachment_requested';
  space_id: string;
  peer_id: string;
  /** Lowercase hex SHA-256 of the blob they want. */
  hash: string;
}

/**
 * Attachment bytes arrived and matched the hash that asked for them.
 *
 * The bytes are handed over rather than stored: blobs never enter protocol
 * state, so where the file lives is the app's decision and its storage.
 */
export interface DataAttachmentReceivedEvent extends BaseEvent {
  type: 'data_attachment_received';
  space_id: string;
  peer_id: string;
  /** Lowercase hex SHA-256, verified against the bytes. */
  hash: string;
  /** The blob, base64. */
  data: string;
}

/**
 * A fetch ended without bytes: the peer no longer holds the blob, or the
 * transfer failed. Both are ordinary — an attachment reference outlives the
 * bytes it names, because the reference replicates and the bytes do not.
 */
export interface DataAttachmentUnavailableEvent extends BaseEvent {
  type: 'data_attachment_unavailable';
  space_id: string;
  peer_id: string;
  hash: string;
  /**
   * `declined` (the peer no longer holds it), `timeout`, `evicted`
   * (displaced by newer fetches), `hash_mismatch`, `peer_gone` (the peer
   * was blocked, forgotten, or came back without the replication
   * capability), or a transfer failure reason.
   */
  reason: string;
}

/**
 * A document cannot be replicated in any form this protocol has: too large
 * for a sync frame, and the peer cannot carry it over the media path either.
 * The two replicas will not converge until the document gets smaller.
 */
export interface DataDocUnsyncableEvent extends BaseEvent {
  type: 'data_doc_unsyncable';
  space_id: string;
  doc_id: string;
  /** Size of the encoding that had nowhere to go. */
  bytes: number;
  reason: string;
}

/**
 * A raw relay server frame that apps need outside or in addition to
 * SDK-owned processing — invite-link lifecycle responses (`GroupInviteLinkCreated`,
 * `GroupJoinedViaInvite`, `GroupInviteJoinPending`, …), `GroupRoleChanged`,
 * `GroupDeleted`, `RateLimited`, and any future/unknown relay message types.
 * `GroupError`, `GroupInfo`, and `UserGroups` are dual-emitted with their
 * stable typed events so apps can consume request correlation or
 * application-owned extension fields without losing the SDK projection.
 *
 * `json` is the verbatim relay frame; parse it and dispatch on its `type`
 * field. Pair with `sendRawServerCommand` for request/response flows. Typed
 * and raw events have no cross-channel ordering guarantee. Raw frames can
 * contain sensitive profile data, invite tokens, and key packages; avoid
 * logging them indiscriminately.
 */
export interface InternetServerMessageEvent extends BaseEvent {
  type: 'internet_server_message';
  json: string;
}

/**
 * Internet-transport readiness transition. `authenticated: true` is the
 * positive gate for `sendRawServerCommand` — the replacement for app-side
 * `relayStatus === 'authenticated'` tracking against a separate app-owned
 * socket. `connected: true, authenticated: false` is the window where the
 * socket is up but the relay has not accepted the auth token yet (raw
 * sends still return false). Emitted only on actual transitions; query the
 * current value with `isInternetReady()`.
 */
export interface InternetStatusChangedEvent extends BaseEvent {
  type: 'internet_status_changed';
  connected: boolean;
  authenticated: boolean;
}

/**
 * The relay displaced this device's internet connection: a newer
 * registration for the same identity took over the relay slot, and the relay
 * closed this socket with code 4000 (optionally preceded by a
 * `SessionSuperseded` notice). The SDK does **not** auto-reconnect after this
 * — a blind reconnect would just re-displace the other socket in a tight loop
 * (the fleet-wide eviction storm the relay-displacement rollout guards
 * against). The transport is left stopped; the app should surface a
 * "connected elsewhere" state and reconnect only on explicit user action
 * (re-enabling the internet transport), or on foreground with long jitter.
 * `reason` is the relay-supplied close/notice reason when present.
 *
 * **Treat this as state, not as an edge.** Delivery is at-least-once: nothing
 * else ever restates the fact it reports, so every layer works to make sure it
 * is not lost, and the cost is that it can repeat. Android redelivers a copy
 * held while JS could not take it (on your next subscribe or foreground); iOS
 * re-derives it from the live latch on every app foreground while the
 * transport stays superseded; and on both platforms the SDK holds a copy that
 * arrived before you had a listener for it and replays it to your first
 * `on(...)`. Handlers must therefore be idempotent — setting a "connected
 * elsewhere" flag is fine, pushing a screen or firing a notification per event
 * is not. Repeats stop as soon as the transport is re-enabled, which also
 * drops any held copy.
 *
 * The pull-side counterpart is `isInternetSuperseded()`, which answers the
 * same question on demand and survives the windows no in-memory delivery can
 * (a late subscribe, a JS reload, a process restart). An app that reconciles
 * against it on foreground needs nothing from this event but the prompt.
 */
export interface InternetSessionSupersededEvent extends BaseEvent {
  type: 'internet_session_superseded';
  reason?: string;
}

/**
 * The user stopped the mesh from the Android foreground-service notification's
 * "Stop" action rather than through a `stop()` call. The SDK has already torn
 * down the transports, the process scheduler, the keep-alive service and the
 * protocol core by the time this arrives — it is a notification, not a request
 * to act. Apps that track mesh state themselves must reconcile it here, or
 * they will keep reporting an active mesh against stopped transports.
 *
 * **Treat this as state, not as an edge.** Like
 * {@link InternetSessionSupersededEvent}, this is *one-shot* — nothing ever
 * restates it — so it is held for redelivery when it cannot be delivered:
 * natively while JS is unreachable, and by the SDK when it arrives before you
 * have a listener for it (replayed to your first `on(...)`). Delivery is
 * at-least-once and handlers must be idempotent. A held copy is dropped by
 * `start()`, which is a mesh coming back up.
 *
 * Android only; iOS has no equivalent notification affordance.
 */
export interface MeshStoppedByUserEvent extends BaseEvent {
  type: 'mesh_stopped_by_user';
}

/**
 * Relay-side registration state of a group (`groupRelaySyncState`).
 */
export type RelaySyncState = 'synced' | 'pending' | 'unsynced';

/**
 * Union type of all events
 */
export type ProtocolEvent =
  | MessageSentEvent
  | MessageReceivedEvent
  | MessageDeliveredEvent
  | MessageFailedEvent
  | MessageDecryptionFailedEvent
  | TransportSwitchedEvent
  | RelayPromotedEvent
  | RelayDemotedEvent
  | RelayDemotedBatteryEvent
  | NeighborDiscoveredEvent
  | NeighborLostEvent
  | IdentityReadyEvent
  | NetworkMetricsEvent
  | FileProgressEvent
  | FileReceivedEvent
  | FileReceiveFailedEvent
  | MediaSentEvent
  | MediaSendFailedEvent
  | AckEvictedEvent
  | FragmentAssemblyEvictedEvent
  | DiagnosticEvent
  | SecureSessionEstablishedEvent
  | SecureSessionFailedEvent
  | ConvergenceDiagEvent
  | WelcomeSendAttemptedEvent
  | WelcomeSendSucceededEvent
  | WelcomeSendFailedEvent
  | WelcomeSendExpiredEvent
  | ConnectionRequestReceivedEvent
  | ConnectionRequestUndeliverableEvent
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
  | GroupRelaySyncChangedEvent
  | UsernameResolvedEvent
  | GroupMessageSentEvent
  | GroupMessagePartialFailureEvent
  | GroupMessageDeliveryReportEvent
  | GroupRichExtrasDroppedEvent
  | GroupUnauthorizedMembershipChangeEvent
  | GroupEpochForkDetectedEvent
  | GroupEpochForkResolvedEvent
  | GroupRoleChangedEvent
  | GroupRenamedEvent
  | DorsScoreUpdatedEvent
  | DorsTransportSelectedEvent
  | DorsTransportSwitchedEvent
  | DorsEscalationTriggeredEvent
  | ServiceDiscoveredEvent
  | ServiceRequestReceivedEvent
  | ServiceResponseReceivedEvent
  | PresenceUpdatedEvent
  | InternetServerMessageEvent
  | InternetStatusChangedEvent
  | InternetSessionSupersededEvent
  | MeshStoppedByUserEvent
  | TypingIndicatorReceivedEvent
  | ReadReceiptReceivedEvent
  | MessageRelayedEvent
  | MessageDeferredEvent
  | MessageRetryingEvent
  | MessageUndeliverableEvent
  | MediaResendRequiredEvent
  | SecurityWarningEvent
  | UserBlockedEvent
  | UserUnblockedEvent
  | DataChangedEvent
  | DataDocSizeWarningEvent
  | DataAttachmentRequestedEvent
  | DataAttachmentReceivedEvent
  | DataAttachmentUnavailableEvent
  | DataDocUnsyncableEvent;

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
 * Snapshot of whether a rich group send right now would seal its extras.
 * Point-in-time and advisory: capability knowledge changes with key-package
 * exchanges, attested adds, and restarts, and the send path re-evaluates the
 * gate itself — use this to warn before sending (e.g. gray out the attachment
 * button) instead of learning from GroupRichExtrasDropped after the drop.
 */
export interface GroupRichReadiness {
  /** True when every other member is known rich-capable and the local kill switch is on. */
  ready: boolean;
  /**
   * Members not known to parse the sealed rich payload (unknown and
   * known-non-support are indistinguishable). Empty when ready, and also
   * empty when only the local kill switch blocks sealing.
   */
  unknownMembers: string[];
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
 * BLE diagnostic counters, from `getBleDiagnostics()`.
 *
 * Degraded-path counters, not error counters: every frame they count was
 * still sent. That is what makes them worth watching — the failure they
 * detect does not appear in delivery metrics. Monotonic for the lifetime of
 * the protocol instance, so sample the delta rather than the value.
 */
export interface BleDiagnostics {
  /** Frames broadcast to all peers because the intended peer was not matched. */
  fragmentFallbacks: number;
  /** Sends naming a peer that is BLE-connected under a different id. */
  recipientNotAmongPeers: number;
  /** Peers reporting an MTU too small for a fragment header. */
  undersizedMtuReports: number;
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

/**
 * Payload handed to a mesh wake task (Android only).
 *
 * @see registerMeshWakeTask
 */
export interface MeshWakeTaskData {
  /**
   * Why JavaScript was woken. Only `'sticky_restart'` exists today — the system
   * handed the mesh keep-alive service back after the process was killed. Match
   * on it rather than assuming, so a future reason cannot be mistaken for this
   * one.
   */
  reason: 'sticky_restart' | (string & {});
}
