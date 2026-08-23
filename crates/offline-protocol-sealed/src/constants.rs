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
/// mls-rs defaults to a year. 28 days is what RFC 9420 asks an application to
/// define, and it bounds how long an unused init key stays usable.
///
/// It clears [`MAX_ACCEPTED_KEY_PACKAGE_LIFETIME`] with three weeks to spare,
/// which is not a coincidence to be tidied away: a leaf that minted a window
/// wider than the phone accepts would be refused at the one moment it has no
/// way to hear why, and the pairing would fail on a device with no screen.
/// The assertion below is what stops one number moving without the other.
pub const LEAF_KEY_PACKAGE_LIFETIME: Duration = Duration::from_secs(28 * 24 * 3600);

/// The widest validity window this SDK will admit in a key package it did not
/// mint, at import and on every read of the cache.
///
/// RFC 9420 requires an application to define this and reject anything longer,
/// and until issue 396 nothing here did. OpenMLS ships both halves of the rule
/// and wires up neither: it declares `MAX_LEAF_NODE_LIFETIME_RANGE_SECONDS`
/// (an hour plus three months) and never calls `Lifetime::has_acceptable_range`,
/// so `KeyPackageIn::validate` checks only that *now* falls inside the window.
/// A package claiming a century passed.
///
/// # Why the window is what ages a key package out
///
/// A key package is admitted once and cached in the install-scoped
/// protocol-state store, where it stays usable for establishing new sessions
/// until the window closes. That window is the only thing that expires it, so
/// an unbounded one turns a single leaked init key into a permanent capability
/// to open a session as its owner. Bounding what we mint and admitting anything
/// inbound bounds nothing.
///
/// # Why 90 days and not the OpenMLS constant
///
/// The tempting cap is the one OpenMLS declares, and it is the wrong number by
/// exactly zero seconds. An `openmls` key package built without an explicit
/// lifetime carries the library's default of three months plus a one-hour skew
/// margin, which *is* `MAX_LEAF_NODE_LIFETIME_RANGE_SECONDS`. Every key package
/// this SDK minted before issue 396 is one of those, so a cap set there admits
/// them with no margin at all, and any peer whose skew margin is a second wider
/// is refused. 90 days sits above every window this SDK has ever put on the
/// wire and far below the year mls-rs hands out by default.
pub const MAX_ACCEPTED_KEY_PACKAGE_LIFETIME: Duration = Duration::from_secs(90 * 24 * 3600);

// A leaf's own package has to survive the phone's gate. Checked at compile time
// because the failure is otherwise a pairing that fails in the field, on the
// one device that cannot report why.
const _: () = assert!(
    LEAF_KEY_PACKAGE_LIFETIME.as_secs() <= MAX_ACCEPTED_KEY_PACKAGE_LIFETIME.as_secs(),
    "a leaf mints a window the phone refuses"
);

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
