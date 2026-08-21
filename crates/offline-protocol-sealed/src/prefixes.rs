//! The control-frame prefixes a 1:1 sealed conversation is carried on.
//!
//! A frame's type is the reserved prefix its content begins with. The six
//! here are the ones both ends of a pair speak: everything needed to
//! establish a session, carry sealed payloads over it, and confirm it came
//! up. They live in this crate because a leaf node emits and parses them
//! with a different MLS implementation than the engine, and two ends that
//! disagree about a prefix byte do not have a conversation at all.
//!
//! # What is not here
//!
//! The engine reserves many more: connection lifecycle, group frames, relay
//! answers, presence, the sealed rich body, document sync. None of them
//! reach a leaf node, and the registry that gates them is engine
//! machinery. The split is deliberately "what a pair needs" rather than
//! "every prefix", because the second would put group and relay vocabulary
//! into a crate that builds for a part with 1536 KB of flash.
//!
//! # The registry still lives in the engine
//!
//! Naming a prefix here does **not** reserve it. Reservation is the
//! `INTERNAL_PREFIXES` array in `offline-protocol`, which drives refusal of
//! application content that begins with one, and which is generated from the
//! same macro invocation that names these. Adding a prefix to this module
//! without adding it there registers nothing.

/// Carries a key package payload, JSON, signed on the control plane.
///
/// This is the only channel by which capabilities are advertised, which is
/// why a device that cannot mint one is served the protocol floor forever.
pub const KEY_PACKAGE: &str = "__MLS_KEY_PKG__";

/// Carries an MLS Welcome, JSON, signed on the control plane.
///
/// The MLS bytes inside are a JSON byte array rather than base64, unlike
/// every group frame.
pub const WELCOME: &str = "__MLS_WELCOME__";

/// Carries a sealed payload: the compact envelope in base64, or the JSON
/// envelope that is the permanent floor.
///
/// This is a data-plane frame. It carries no control-plane signature,
/// because MLS authenticates its own sender and a second signature would
/// state on the outside what the AEAD already proves on the inside.
pub const ENCRYPTED: &str = "__MLS_ENC__";

/// Asks a peer to confirm it holds a working session. Empty body, signed.
pub const SESSION_CONFIRM_PROBE: &str = "__MLS_CONFIRM_PROBE__";

/// Answers [`SESSION_CONFIRM_PROBE`]. Empty body, signed.
pub const SESSION_CONFIRM_ACK: &str = "__MLS_CONFIRM_ACK__";

/// A confirmation that travels **inside** an [`ENCRYPTED`] envelope and
/// never on the wire as a frame of its own.
///
/// Its whole purpose is to be a group-aware decrypt. A peer that created a
/// session of its own confirms only on a successful decrypt, so a plaintext
/// acknowledgement leaves it unconfirmed no matter how many times it is
/// sent. The receiver consumes this marker and never surfaces it.
pub const SESSION_CONFIRM_ENCRYPTED: &str = "__MLS_ENC_CONFIRM__";

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins every prefix byte for byte.
    ///
    /// These are wire constants shared with an implementation that is not
    /// compiled against this crate, so a rename here is not a refactor: it is
    /// a frame the other end stops recognizing, and it surfaces as a session
    /// that never comes up rather than as an error anyone can read. Nothing
    /// else in the workspace compares these to a literal.
    #[test]
    fn prefixes_are_pinned() {
        assert_eq!(KEY_PACKAGE, "__MLS_KEY_PKG__");
        assert_eq!(WELCOME, "__MLS_WELCOME__");
        assert_eq!(ENCRYPTED, "__MLS_ENC__");
        assert_eq!(SESSION_CONFIRM_PROBE, "__MLS_CONFIRM_PROBE__");
        assert_eq!(SESSION_CONFIRM_ACK, "__MLS_CONFIRM_ACK__");
        assert_eq!(SESSION_CONFIRM_ENCRYPTED, "__MLS_ENC_CONFIRM__");
    }

    /// No prefix may be a prefix of another.
    ///
    /// Dispatch picks a frame type by the first prefix that matches, so an
    /// overlap makes the shorter one shadow the longer one and routes a frame
    /// to the wrong handler. `__MLS_ENC__` and `__MLS_ENC_CONFIRM__` are the
    /// near miss: they differ only after the point where the shorter one ends,
    /// and the trailing underscores are what keep them apart.
    #[test]
    fn no_prefix_shadows_another() {
        let all = [
            KEY_PACKAGE,
            WELCOME,
            ENCRYPTED,
            SESSION_CONFIRM_PROBE,
            SESSION_CONFIRM_ACK,
            SESSION_CONFIRM_ENCRYPTED,
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                if i != j {
                    assert!(
                        !a.starts_with(b),
                        "prefix {b} shadows {a}: dispatch would never reach {a}"
                    );
                }
            }
        }
    }
}
