//! Constants used throughout the offline-protocol-transport crate.

// BLE Transport Constants
/// BLE service UUID for the Offline Protocol.
pub const BLE_SERVICE_UUID: &str = "6E400001-B5A3-F393-E0A9-E50E24DCCA9E";

/// BLE characteristic UUID for message data.
pub const BLE_MESSAGE_CHAR_UUID: &str = "6E400002-B5A3-F393-E0A9-E50E24DCCA9E";

/// BLE characteristic UUID for device ID.
pub const BLE_DEVICE_ID_CHAR_UUID: &str = "6E400003-B5A3-F393-E0A9-E50E24DCCA9E";

/// Fallback fragment size used when no MTU has been negotiated for a peer.
///
/// Matches the historical iOS CoreBluetooth auto-negotiated minimum ATT MTU
/// (iPhone 5/6 era). Modern iOS and Android links negotiate higher values,
/// which are pushed into the transport via `BleTransport::set_peer_mtu`.
pub const BLE_MAX_FRAGMENT_SIZE: usize = 185;

/// Hard upper bound on a per-peer BLE fragment payload (bytes).
///
/// BLE 5 negotiates an ATT MTU up to 517 bytes; subtracting the 3-byte ATT
/// header yields 514 usable payload bytes. We clamp at 512 for a small safety
/// margin and to keep allocations friendly.
pub const MAX_REASONABLE_BLE_PAYLOAD: usize = 512;

/// Timeout for fragment reassembly in seconds.
/// Fragments older than this are discarded.
pub const BLE_FRAGMENT_TIMEOUT_SECS: u64 = 30;

/// Maximum number of concurrent fragment assemblies.
/// Prevents unbounded memory growth from incomplete fragments.
pub const BLE_MAX_FRAGMENT_ASSEMBLIES: usize = 64;

/// Maximum number of fragments per message.
/// Prevents fragmentation attacks and memory exhaustion.
pub const BLE_MAX_FRAGMENT_COUNT: usize = 512;

/// Magic bytes identifying Offline Protocol fragments.
pub const FRAGMENT_MAGIC: [u8; 2] = *b"OP"; // Offline Protocol

/// Fragment protocol version.
pub const FRAGMENT_VERSION: u8 = 1;

/// Fixed size of fragment header in bytes.
/// Format: magic (2) + version (1) + id_len (1) + index (2) + total (2) + data_len (2)
pub const FRAGMENT_HEADER_FIXED: usize = 2 /*magic*/ + 1 /*version*/ + 1 /*id_len*/ + 2 /*index*/ + 2 /*total*/ + 2 /*data_len*/;

/// ATT overhead bytes for BLE MTU calculation.
pub const ATT_OVERHEAD_BYTES: usize = 3;

/// Heuristic send queue capacity for congestion calculation.
pub const HEURISTIC_SEND_CAPACITY: f32 = 50.0;

/// Weight for new latency values in EMA calculation.
pub const EMA_WEIGHT_NEW_LATENCY: f32 = 0.3;

/// Weight for existing latency values in EMA calculation.
pub const EMA_WEIGHT_EXISTING_LATENCY: f32 = 0.7;

// WiFi Direct Transport Constants
/// Maximum payload size for WiFi Direct transmission (bytes).
pub const WIFI_DIRECT_MAX_PAYLOAD_SIZE: usize = 65535;

/// Connection timeout for WiFi Direct in seconds.
pub const WIFI_DIRECT_CONNECTION_TIMEOUT_SECS: u64 = 30;

/// Default device name for Wi-Fi Direct.
pub const DEFAULT_DEVICE_NAME: &str = "OfflineProtocolDevice";

/// Default group owner intent for Wi-Fi Direct.
pub const DEFAULT_GROUP_OWNER_INTENT: u8 = 7;

// Internet Transport Constants
// Connection timeouts and keepalive are owned by the platform bridge that
// manages the actual socket (OkHttp pingInterval on Android, URLSession
// ping on iOS) — no Rust-side constants for them.
/// Default WebSocket server address for Internet transport.
pub const INTERNET_DEFAULT_SERVER_ADDRESS: &str = "ws://localhost:8080";

/// Timeout for pending Internet send confirmations in seconds.
/// Messages awaiting platform confirmation beyond this duration are treated as failed.
pub const INTERNET_PENDING_CONFIRMATION_TIMEOUT_SECS: u64 = 15;

// Reticulum Transport Constants
/// Connection timeout for reaching the Reticulum daemon (seconds).
pub const RETICULUM_CONNECTION_TIMEOUT_SECS: u64 = 60;

/// Timeout for pending Reticulum send confirmations (seconds).
/// Higher than Internet because Reticulum paths can be high-latency
/// (especially LoRa multi-hop).
pub const RETICULUM_PENDING_CONFIRMATION_TIMEOUT_SECS: u64 = 120;

/// Default maximum payload size for Reticulum (bytes).
/// Reticulum's encrypted MDU is 383 bytes per single packet (plain MDU is
/// 464 bytes). Larger payloads are handled transparently by Reticulum's
/// Resource mechanism over an established Link.
pub const RETICULUM_MAX_PAYLOAD_SIZE: usize = 65536;

// Nostr Transport Constants
/// Connection timeout for Nostr relay WebSocket connections (seconds).
pub const NOSTR_CONNECTION_TIMEOUT_SECS: u64 = 30;

/// Timeout for pending Nostr send confirmations (seconds).
/// Higher than Internet (relay propagation can be slower) but lower than
/// Reticulum (no multi-hop mesh delays).
pub const NOSTR_PENDING_CONFIRMATION_TIMEOUT_SECS: u64 = 30;

/// Default maximum payload size for Nostr events (bytes).
/// Nostr relays typically accept events up to 64KB–128KB.
///
/// Measured against the complete `["EVENT", {...}]` relay message, since that
/// is what a relay accepts or rejects — not the protocol message inside it,
/// which is smaller by the base64 (and, once the envelope is sealed, the
/// encryption) overhead.
pub const NOSTR_MAX_PAYLOAD_SIZE: usize = 65536;

/// Cap on the stored events a relay returns for the initial Nostr subscription.
///
/// NIP-01 scopes `limit` to the initial query — relays MUST ignore it once
/// they start streaming live events — so it bounds how much history each
/// (re)connect replays without capping ongoing delivery.
///
/// Treat it as advisory in both directions: `limit` is a SHOULD, and NIP-11
/// `max_limit` lets a relay clamp it silently, so a short result set is not
/// evidence that the relay had nothing more to send.
pub const NOSTR_INITIAL_QUERY_LIMIT: usize = 500;

/// Backwards window the outgoing gift wrap's `created_at` is jittered into,
/// and therefore the amount the subscription's `since` must reach back past
/// the receive watermark.
///
/// **These two uses must stay the same number.** NIP-59 has the sender draw
/// `created_at` uniformly from `[now − window, now]`, past-only — so an event
/// published now can carry a timestamp up to a window old, and a `since`
/// computed from "the newest `created_at` we have seen" would sit *above* it
/// and skip it. Subtracting the window on the way out is what makes the
/// watermark safe against that; if the wrap ever jitters further back than
/// this, messages go missing silently.
///
/// The value is a straight trade: NIP-59's reference is 2 days, which buys
/// unlinkability against timing correlation, but every hour of jitter is an
/// hour of replay overlap on every reconnect. One hour keeps the overlap
/// small; the anonymity difference against a relay that already sees arrival
/// time is marginal.
///
/// Applied by the `since` computation since the watermark landed; the wrap
/// side arrives with the sealed envelope.
pub const NOSTR_CREATED_AT_JITTER_SECS: i64 = 3600;

/// Extra slack subtracted from the subscription's `since`, on top of
/// [`NOSTR_CREATED_AT_JITTER_SECS`], to absorb clock disagreement between the
/// sending peer, the relay, and this device.
///
/// `created_at` is written by the *sender*, so a peer whose clock runs behind
/// ours stamps events below where our watermark thinks "now" is. Without this
/// margin those events fall under `since` and are never fetched.
pub const NOSTR_CLOCK_SKEW_MARGIN_SECS: i64 = 300;

/// How far back the first subscription reaches when no receive watermark
/// exists yet — a fresh install, a `wipePersistedState` logout, or any
/// subscription created before protocol-state storage has been restored.
///
/// It is deliberately *not* zero. A zero (or absent) `since` is exactly the
/// unbounded filter this watermark exists to remove, and the no-watermark case
/// is common rather than exotic: the bridges subscribe on every relay connect,
/// which can happen before `initialize_mls` has restored anything.
///
/// A day of backfill is enough for the case that matters — the same user id
/// reinstalling or signing back in, whose peers have been publishing to a
/// routing tag nobody was listening on — and `NOSTR_INITIAL_QUERY_LIMIT`
/// bounds what that window can actually pull down.
pub const NOSTR_FIRST_RUN_BACKFILL_SECS: i64 = 86_400;

/// How far ahead of local time an event's `created_at` may sit and still
/// advance the receive watermark.
///
/// The routing tag is publicly derivable, so **anyone** can publish an event
/// addressed to us, and `created_at` is attacker-chosen: a single event dated
/// years ahead would otherwise pin the watermark to that value and every
/// subsequent subscription would ask for events `since` the far future —
/// silently receiving nothing, forever. Values beyond this tolerance are
/// ignored outright rather than clamped: an absurd timestamp says nothing
/// about how far our receive progress has actually reached.
pub const NOSTR_FUTURE_DATED_TOLERANCE_SECS: i64 = 900;

/// Maximum number of peers whose per-install Nostr public key the transport
/// remembers for sealing.
///
/// The map is fed from key packages, whose sender id is wire-claimed, so it is
/// bounded like every other wire-keyed map in the engine. It resets at capacity
/// rather than evicting selectively: the consequence of forgetting a peer is
/// that the next frame to them takes the bootstrap leg — a privacy degradation,
/// never a delivery failure.
///
/// How long that degradation lasts depends on whether the peer has published
/// key packages. A forgotten peer who has is re-resolved on the next miss, so
/// the window is one frame. A peer who has not (an older build, or Nostr
/// publication disabled) stays on the bootstrap leg until their key package
/// arrives again by some other route, which for a flood-induced reset means
/// until restart — the restore re-marks every peer from `PeerCapabilities` — or
/// until re-exchange. That second case is what makes a reset-at-capacity flood
/// a real, if bounded, downgrade vector rather than a one-frame blip.
pub const NOSTR_MAX_TRACKED_PEER_KEYS: usize = 1000;

/// Number of single-use MLS key packages published for cold contact.
///
/// Each occupies its own addressable slot (a distinct `d` tag), because an MLS
/// key package's init key is consumed by the first peer who uses it: one
/// replaceable record would mean a stranger who fetches it after it was spent
/// builds a Welcome that can never be processed.
///
/// What the count buys is coverage of the *sequential* gap — packages consumed
/// between one refresh and the next, which runs on the process tick. It does
/// **not** absorb concurrent cold contacts: nothing distributes simultaneous
/// fetchers across slots, so two strangers arriving at once generally race for
/// the same init key and the loser recovers through the reverse key-package
/// exchange, not through slot multiplicity. Do not raise this expecting
/// concurrency headroom it does not provide.
///
/// Raising it costs one published event and one key package held in provider
/// storage per slot; lowering it shortens the window a burst of sequential
/// contacts can consume before every slot is stale, which degrades cold contact
/// until the next tick refills rather than silently reusing a spent package.
pub const NOSTR_KEY_PACKAGE_SLOTS: usize = 5;

/// Maximum discovery records one relay may return for a username query.
///
/// A username resolves to the set of devices claiming it, so this bounds how
/// many claimants a single relay can put in front of a user. Sized well above
/// any real user's device count and well below
/// `nostr::MAX_QUERY_EVENTS`, which bounds the whole query across every
/// connected relay.
///
/// The cost of this being too low is not a dropped record but a *displaced*
/// one: the tag is public, anyone may publish to it, and a squatter who floods
/// it pushes legitimate claimants out of the answer. That is crowding, which
/// the design accepts as the price of a non-authoritative directory — the user
/// still confirms out of band, and the invite path is unaffected.
pub const NOSTR_DISCOVERY_QUERY_LIMIT: usize = 16;

// Transport-wide Constants
/// Default maximum message size in bytes (1 MB).
/// Applied at the transport layer before JSON deserialization to prevent
/// memory exhaustion from oversized payloads.
pub const DEFAULT_MAX_MESSAGE_SIZE: usize = 1_048_576;
