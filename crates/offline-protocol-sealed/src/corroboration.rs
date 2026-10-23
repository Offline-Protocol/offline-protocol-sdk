//! The body of a session confirmation probe or acknowledgement, which is how
//! the two sides of a 1:1 session show each other they hold the same group.
//!
//! A probe that only says "I hold a session" cannot tell a peer holding our
//! group from a peer holding a group of its own for the same slot. Two groups
//! for one slot agree on the slot and on the epoch, so every frame between
//! them passes framing validation and fails as an ordinary decryption
//! failure, which nothing re-keys on. A plaintext probe confirming such a
//! pair is how one forms. The tag below is what tells the two apart: it is an
//! MLS exporter value, so only a member of the same group at the same epoch
//! computes the same one.
//!
//! Both ends must agree on the label and the lengths, and a leaf builds its
//! answer with a different MLS implementation, so they live here.
//!
//! An empty or unparseable body is an older peer, and selects the plaintext
//! rules it always had.

use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// The MLS exporter label the tag is derived under.
pub const SESSION_CORROBORATION_LABEL: &str = "offline-protocol session corroboration v1";

/// Length of the exporter context each frame carries, in bytes.
pub const SESSION_CORROBORATION_NONCE_LEN: usize = 16;

/// Length of the tag, in bytes.
pub const SESSION_CORROBORATION_TAG_LEN: usize = 16;

/// Body of a [`SESSION_CONFIRM_PROBE`] or [`SESSION_CONFIRM_ACK`] frame.
///
/// The nonce is fresh per frame and is the exporter context, so a tag says
/// nothing about the group and links no two frames. The frame itself is
/// signed by its sender's identity key, so a tag cannot be put in a peer's
/// mouth by anyone else.
///
/// [`SESSION_CONFIRM_PROBE`]: crate::prefixes::SESSION_CONFIRM_PROBE
/// [`SESSION_CONFIRM_ACK`]: crate::prefixes::SESSION_CONFIRM_ACK
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCorroboration {
    /// The sender's MLS epoch for the session.
    pub epoch: u64,
    /// The exporter context, [`SESSION_CORROBORATION_NONCE_LEN`] bytes.
    pub nonce: Vec<u8>,
    /// The exporter output, [`SESSION_CORROBORATION_TAG_LEN`] bytes.
    pub tag: Vec<u8>,
}

impl SessionCorroboration {
    /// Reads a frame body, the text after the prefix. `None` for an empty
    /// body, which an older peer sends, and for anything that is not this
    /// shape with both fields at their lengths.
    pub fn parse(body: &str) -> Option<Self> {
        let parsed: Self = serde_json::from_str(body).ok()?;
        (parsed.nonce.len() == SESSION_CORROBORATION_NONCE_LEN
            && parsed.tag.len() == SESSION_CORROBORATION_TAG_LEN)
            .then_some(parsed)
    }

    /// The frame body for this value.
    pub fn to_body(&self) -> alloc::string::String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn a_body_round_trips() {
        let value = SessionCorroboration {
            epoch: 1,
            nonce: vec![7; SESSION_CORROBORATION_NONCE_LEN],
            tag: vec![9; SESSION_CORROBORATION_TAG_LEN],
        };
        assert_eq!(SessionCorroboration::parse(&value.to_body()), Some(value));
    }

    /// An older peer's empty body, and anything short, are not evidence.
    #[test]
    fn only_a_complete_body_parses() {
        assert_eq!(SessionCorroboration::parse(""), None);
        assert_eq!(SessionCorroboration::parse("{}"), None);
        let short = SessionCorroboration {
            epoch: 1,
            nonce: vec![7; SESSION_CORROBORATION_NONCE_LEN],
            tag: vec![9; SESSION_CORROBORATION_TAG_LEN - 1],
        };
        assert_eq!(SessionCorroboration::parse(&short.to_body()), None);
    }

    /// The label is a wire constant shared with an implementation not
    /// compiled against this crate: a change is a tag nobody else computes.
    #[test]
    fn the_label_is_pinned() {
        assert_eq!(
            SESSION_CORROBORATION_LABEL,
            "offline-protocol session corroboration v1"
        );
    }
}
