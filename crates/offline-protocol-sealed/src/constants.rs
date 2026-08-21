//! Numbers a phone and a leaf node must configure identically.
//!
//! Each of these is a value that appears on both sides of a sealed
//! conversation. A copy that drifts does not fail a build or a test: it
//! produces a peer that is configured differently from the one it is talking
//! to, and the symptom arrives later, on a device, as a message that will not
//! decrypt.

use core::time::Duration;

/// How many generations behind the latest decrypted message a sender ratchet
/// key is kept, so late/reordered messages remain decryptable.
///
/// The OpenMLS default (5) is smaller than the windowed media transfer's
/// in-flight budget (up to 8 chunks on internet transports, interleaved with
/// text on the same 1:1 session ratchet), which would make a sufficiently
/// delayed chunk *permanently* undecryptable and stall the transfer. 32 gives
/// 4x headroom over the largest window at the cost of retaining up to 32
/// unused message keys per sender ratchet.
///
/// The protocol layer keeps the combined in-flight budget within this bound
/// by capping concurrent media transfers per peer
/// (`MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER` in `offline-protocol`); revisit
/// both together if either changes.
pub const SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE: u32 = 32;

/// How far ahead of the highest seen generation a sender ratchet may be
/// fast-forwarded when messages are lost (OpenMLS default).
pub const SENDER_RATCHET_MAXIMUM_FORWARD_DISTANCE: u32 = 1000;

/// How long a leaf node's key package claims to be valid.
///
/// mls-rs defaults to a year. 28 days is leaf-side policy rather than
/// something the phone makes a device do: it is what RFC 9420 asks an
/// application to define, it sits inside the cap OpenMLS declares in
/// `MAX_LEAF_NODE_LIFETIME_RANGE_SECONDS`, and it bounds how long an unused
/// init key stays usable.
///
/// The phone does not enforce that cap today. `tools/mls-interop` step 0.3
/// pins that as current behaviour and fails if OpenMLS starts applying its
/// own; ADR 0021 records it as a gap that is ours to close.
pub const LEAF_KEY_PACKAGE_LIFETIME: Duration = Duration::from_secs(28 * 24 * 3600);

/// How far into the past a leaf's key package validity window has to start.
///
/// OpenMLS tests `not_before < now`, strictly. mls-rs's client builder writes
/// `not_before` as exactly the timestamp it is handed, without the backdating
/// its own `Lifetime::seconds` helper applies, so a package stamped with the
/// current second is refused for being not yet valid. This is also the margin
/// that absorbs clock skew between the two devices.
///
/// A leaf has no clock of its own to stamp with. ADR 0021 records the
/// consequence: it needs a time source at pairing.
pub const LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS: u64 = 3600;
