//! Peer-stream framing: where a frame ends, and who the peer is.
//!
//! The chapter is `docs/spec/stream-framing.md`. A peer stream is a byte
//! stream the platform established to exactly one other device (a Wi-Fi
//! Direct group socket, a Multipeer session, a TCP connection over a LAN or a
//! routed mesh), and the engine holds every such stream in the slot the FFI
//! names `wifi_direct`. A stream gets two things from its platform for free,
//! bytes in order and a close; this module is the one Rust implementation of
//! the two it does not: a frame boundary, and the peer's proved address.
//!
//! # Invariants
//!
//! - **Every frame is length-prefixed.** A `u32` big-endian length, then that
//!   many bytes. A length of zero, or one above
//!   [`PEER_STREAM_MAX_FRAME_BYTES`], closes the stream before a byte of the
//!   body is read, so a hostile prefix costs four bytes and never an
//!   allocation.
//! - **The first frame in each direction is the identity assertion.** It is
//!   verified before anything is announced, and a stream whose preamble did
//!   not verify carries nothing: no frame from it is ever delivered, and the
//!   stream is closed. There is no accept-but-flag state, because the address
//!   a preamble proves becomes the transport-verified peer id the receiving
//!   core matches `Message.sender` against.
//! - **Position decides what a body is.** The first body is the preamble and
//!   every later body is a message. No byte in a body announces its kind.
//! - **The framing is not authenticated.** The prefix comes from whoever
//!   holds the other end. Every bound here keeps a hostile length from costing
//!   memory; none of them establishes trust, which is the preamble's job and
//!   then the message's own.
//!
//! # Who calls this
//!
//! The platform managers that already speak this framing in Kotlin, Swift and
//! Python are conforming implementations of the chapter, not callers of this
//! module: bodies cross the FFI whole (`wifi_direct_message_received`,
//! `wifi_direct_get_next_message`), so a manager reads and writes the prefix
//! itself and verifies the preamble through `verify_identity_assertion`. This
//! module serves a Rust host that owns its own sockets, and it is the
//! reference the chapter's conformance vectors are checked against, in
//! `tests/stream_framing_vectors.rs`.
//!
//! # What this module does not do
//!
//! It never verifies a signature. The verifier is injected as a
//! [`PreambleVerifier`], because the one verifier lives beside the identity
//! key in `offline-protocol-mls` and this crate holds no Ed25519. And it never
//! decodes a message body: what the core does with a delivered body (decode,
//! verify, route, refuse) is above this layer.

use core::fmt;

use crate::constants::{
    PEER_STREAM_LENGTH_PREFIX_LEN, PEER_STREAM_MAX_FRAME_BYTES, PEER_STREAM_PREAMBLE_FLOOR,
};
use crate::{Error, Result};

/// Wraps one body as one frame: the big-endian `u32` length, then the body.
///
/// Refuses what a conforming receiver would refuse on the prefix, an empty
/// body or one above [`PEER_STREAM_MAX_FRAME_BYTES`], so a sender never
/// emits a frame that closes the stream at the other end. The ceiling is
/// inclusive.
///
/// # Errors
///
/// [`Error::SendFailed`] for an empty body, [`Error::MessageTooLarge`] above
/// the ceiling.
pub fn frame(body: &[u8]) -> Result<Vec<u8>> {
    if body.is_empty() {
        return Err(Error::SendFailed(
            "peer stream: a frame body is at least one byte".to_string(),
        ));
    }
    if body.len() > PEER_STREAM_MAX_FRAME_BYTES {
        return Err(Error::MessageTooLarge(
            body.len(),
            PEER_STREAM_MAX_FRAME_BYTES,
        ));
    }
    let mut out = Vec::with_capacity(PEER_STREAM_LENGTH_PREFIX_LEN + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    Ok(out)
}

/// Why a reader closed its stream.
///
/// Every variant is terminal: a closed reader delivers nothing more, and the
/// host closes the socket. None of them is a peer's fault the host should
/// report upward; a stream that never proved a peer was never announced, so
/// there is nothing to report lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// A prefix of zero. No encoding produces an empty message.
    ZeroLength,
    /// A prefix above [`PEER_STREAM_MAX_FRAME_BYTES`], refused before the body.
    OverCeiling(usize),
    /// A preamble prefix under [`PEER_STREAM_PREAMBLE_FLOOR`], refused before
    /// the body: a key and most of a signature cannot verify.
    PreambleUnderFloor(usize),
    /// The first body did not verify as an identity assertion. A well-formed
    /// message arriving first lands here too: position decides what a body
    /// is, and a message is not a preamble.
    PreambleRejected,
    /// The preamble verified, but the stream was opened toward a different
    /// address (a peer-list entry, an invite, a discovery record's `addr`).
    /// The comparison is exact, never case-folded.
    AddressMismatch {
        /// The address the stream was opened toward.
        expected: String,
        /// The address the preamble proved.
        derived: String,
    },
}

impl fmt::Display for CloseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLength => write!(f, "zero-length frame"),
            Self::OverCeiling(len) => write!(
                f,
                "frame length {len} is above the {PEER_STREAM_MAX_FRAME_BYTES}-byte ceiling"
            ),
            Self::PreambleUnderFloor(len) => write!(
                f,
                "preamble length {len} is under the {PEER_STREAM_PREAMBLE_FLOOR}-byte floor"
            ),
            Self::PreambleRejected => {
                write!(f, "the first frame is not a verifiable identity assertion")
            }
            Self::AddressMismatch { expected, derived } => write!(
                f,
                "the preamble proves {derived} but the stream was opened toward {expected}"
            ),
        }
    }
}

impl std::error::Error for CloseReason {}

/// What one direction of a stream yielded, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// The preamble verified. The peer is announced to the core under this
    /// address, the derived one, never a claimed string: it is what
    /// `wifi_direct_peer_connected` takes.
    Announced(String),
    /// A message body from the announced peer, to hand to the core attributed
    /// to `from`: what `wifi_direct_message_received` takes.
    Message {
        /// The address the stream's preamble proved.
        from: String,
        /// Exactly one message in the hop-local encoding, undecoded.
        body: Vec<u8>,
    },
    /// The stream is closed. Always the last event a reader yields, and
    /// nothing after it is delivered. Whether the peer was announced before
    /// this decides whether the host reports it lost.
    Closed(CloseReason),
}

/// Steps one to three of the chapter's preamble verification: parse, verify
/// the signature under the key, derive the address. `None` refuses.
///
/// The one implementation is `offline_protocol_mls::verify_identity_assertion`;
/// a host passes it as `Box::new(|b| verify_identity_assertion(b).ok().map(|a| a.to_string()))`.
pub type PreambleVerifier = Box<dyn Fn(&[u8]) -> Option<String> + Send + Sync>;

/// The receiving side of one direction of a peer stream.
///
/// Feed it the bytes each socket read returned, in order, and act on the
/// events it yields. It holds at most one incomplete frame between calls,
/// whose prefix has already passed the bounds, so memory per stream is
/// bounded by the caller's read buffer plus one body.
///
/// One reader is one direction of one stream. It has no view of other
/// streams, so the chapter's one-announced-stream-per-address rule (a second
/// stream proving an address already announced is refused or supersedes the
/// first, and either way the core sees one announcement and one loss) is the
/// host's, which is the only place that can see both streams.
pub struct PeerStreamReader {
    verify: PreambleVerifier,
    expected: Option<String>,
    buffer: Vec<u8>,
    announced: Option<String>,
    closed: bool,
}

impl fmt::Debug for PeerStreamReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerStreamReader")
            .field("expected", &self.expected)
            .field("buffered", &self.buffer.len())
            .field("announced", &self.announced)
            .field("closed", &self.closed)
            .finish()
    }
}

impl PeerStreamReader {
    /// A reader for a stream with no claim about who is at the other end: an
    /// inbound connection, or a peer-list entry naming only a host and port.
    /// The derived address is the peer's id, and there is nothing to compare.
    pub fn new(verify: PreambleVerifier) -> Self {
        Self {
            verify,
            expected: None,
            buffer: Vec::new(),
            announced: None,
            closed: false,
        }
    }

    /// Requires the preamble to prove exactly `address`, the chapter's fourth
    /// step, for a stream opened toward a known address. A mismatch closes
    /// the stream with [`CloseReason::AddressMismatch`]. The announced address
    /// is still the derived one; the claim is a hint about whom to expect,
    /// never a name to announce.
    pub fn expecting(mut self, address: impl Into<String>) -> Self {
        self.expected = Some(address.into());
        self
    }

    /// The address this stream proved, once it has.
    pub fn peer(&self) -> Option<&str> {
        self.announced.as_deref()
    }

    /// Whether the reader has closed the stream. A closed reader yields
    /// nothing more, whatever it is fed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Feeds the bytes one read returned and yields every event they complete.
    ///
    /// Frames are yielded in stream order. A [`StreamEvent::Closed`] is
    /// terminal and always last: frames decoded before the fault are still
    /// delivered, because they arrived on a stream whose peer was proved,
    /// and nothing after it ever is.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        if self.closed {
            return events;
        }
        self.buffer.extend_from_slice(bytes);

        // Frames are consumed through an offset and the buffer is compacted
        // once per call: draining per frame would move the remaining bytes
        // once for every frame, quadratic in the frames one read carries.
        let mut consumed = 0;
        loop {
            let pending = &self.buffer[consumed..];
            if pending.len() < PEER_STREAM_LENGTH_PREFIX_LEN {
                break;
            }
            let mut prefix = [0u8; PEER_STREAM_LENGTH_PREFIX_LEN];
            prefix.copy_from_slice(&pending[..PEER_STREAM_LENGTH_PREFIX_LEN]);
            let length = u32::from_be_bytes(prefix) as usize;

            // Bounds on the prefix, before a byte of the body: the whole
            // point of reading the length first.
            if let Some(reason) = self.refuse_prefix(length) {
                events.push(self.close(reason));
                return events;
            }

            let end = PEER_STREAM_LENGTH_PREFIX_LEN + length;
            if pending.len() < end {
                break;
            }
            let body = pending[PEER_STREAM_LENGTH_PREFIX_LEN..end].to_vec();
            consumed += end;

            match self.accept(body) {
                Ok(event) => events.push(event),
                Err(reason) => {
                    events.push(self.close(reason));
                    return events;
                }
            }
        }
        self.buffer.drain(..consumed);
        events
    }

    fn refuse_prefix(&self, length: usize) -> Option<CloseReason> {
        if length == 0 {
            return Some(CloseReason::ZeroLength);
        }
        if length > PEER_STREAM_MAX_FRAME_BYTES {
            return Some(CloseReason::OverCeiling(length));
        }
        if self.announced.is_none() && length < PEER_STREAM_PREAMBLE_FLOOR {
            return Some(CloseReason::PreambleUnderFloor(length));
        }
        None
    }

    fn accept(&mut self, body: Vec<u8>) -> core::result::Result<StreamEvent, CloseReason> {
        if let Some(from) = &self.announced {
            return Ok(StreamEvent::Message {
                from: from.clone(),
                body,
            });
        }
        let derived = (self.verify)(&body).ok_or(CloseReason::PreambleRejected)?;
        if let Some(expected) = &self.expected {
            if *expected != derived {
                return Err(CloseReason::AddressMismatch {
                    expected: expected.clone(),
                    derived,
                });
            }
        }
        self.announced = Some(derived.clone());
        Ok(StreamEvent::Announced(derived))
    }

    fn close(&mut self, reason: CloseReason) -> StreamEvent {
        self.closed = true;
        self.buffer.clear();
        StreamEvent::Closed(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A verifier that accepts one fixed body and names it; everything else
    /// is refused. The real verifier is exercised by the conformance test
    /// in `tests/stream_framing_vectors.rs`.
    fn verifier(accept: &'static [u8], as_address: &'static str) -> PreambleVerifier {
        Box::new(move |b| (b == accept).then(|| as_address.to_string()))
    }

    const PREAMBLE: &[u8] = &[7u8; 96];

    #[test]
    fn frame_prefixes_the_length_big_endian() {
        assert_eq!(frame(b"abc").unwrap(), [0, 0, 0, 3, b'a', b'b', b'c']);
        assert!(frame(b"").is_err(), "an empty body is not a frame");
        let at_ceiling = vec![0u8; PEER_STREAM_MAX_FRAME_BYTES];
        assert_eq!(
            &frame(&at_ceiling).unwrap()[..4],
            &[0x00, 0x10, 0x00, 0x00],
            "the ceiling is inclusive"
        );
        assert!(matches!(
            frame(&vec![0u8; PEER_STREAM_MAX_FRAME_BYTES + 1]),
            Err(Error::MessageTooLarge(_, PEER_STREAM_MAX_FRAME_BYTES))
        ));
    }

    #[test]
    fn the_first_frame_announces_and_later_frames_deliver() {
        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer"));
        assert_eq!(r.peer(), None);
        let mut bytes = frame(PREAMBLE).unwrap();
        bytes.extend(frame(b"hello").unwrap());
        bytes.extend(frame(b"world").unwrap());
        assert_eq!(
            r.feed(&bytes),
            vec![
                StreamEvent::Announced("off1peer".into()),
                StreamEvent::Message {
                    from: "off1peer".into(),
                    body: b"hello".to_vec()
                },
                StreamEvent::Message {
                    from: "off1peer".into(),
                    body: b"world".to_vec()
                },
            ]
        );
        assert_eq!(r.peer(), Some("off1peer"));
        assert!(!r.is_closed());
    }

    #[test]
    fn chunking_does_not_change_what_is_delivered() {
        let mut whole = frame(PREAMBLE).unwrap();
        whole.extend(frame(b"hello").unwrap());
        let expected = PeerStreamReader::new(verifier(PREAMBLE, "off1peer")).feed(&whole);

        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer"));
        let mut byte_at_a_time = Vec::new();
        for b in &whole {
            byte_at_a_time.extend(r.feed(std::slice::from_ref(b)));
        }
        assert_eq!(byte_at_a_time, expected);
        assert!(r.buffer.is_empty(), "nothing left over after whole frames");
    }

    #[test]
    fn a_preamble_that_does_not_verify_closes_the_stream_with_nothing_announced() {
        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer"));
        let mut bytes = frame(&[9u8; 100]).unwrap();
        bytes.extend(frame(b"never delivered").unwrap());
        assert_eq!(
            r.feed(&bytes),
            vec![StreamEvent::Closed(CloseReason::PreambleRejected)]
        );
        assert!(r.is_closed());
        assert_eq!(r.peer(), None);
        assert!(
            r.feed(&frame(PREAMBLE).unwrap()).is_empty(),
            "closed is closed"
        );
    }

    #[test]
    fn a_preamble_under_the_floor_is_refused_on_the_prefix() {
        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer"));
        // Only the prefix arrives; the body never has to.
        assert_eq!(
            r.feed(&[0, 0, 0, 95]),
            vec![StreamEvent::Closed(CloseReason::PreambleUnderFloor(95))]
        );
    }

    #[test]
    fn a_message_frame_may_be_short_once_the_peer_is_announced() {
        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer"));
        r.feed(&frame(PREAMBLE).unwrap());
        assert_eq!(
            r.feed(&frame(b"x").unwrap()),
            vec![StreamEvent::Message {
                from: "off1peer".into(),
                body: b"x".to_vec()
            }],
            "the floor applies to the preamble only"
        );
    }

    #[test]
    fn a_zero_or_oversize_prefix_closes_before_the_body() {
        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer"));
        r.feed(&frame(PREAMBLE).unwrap());
        assert_eq!(
            r.feed(&[0, 0, 0, 0]),
            vec![StreamEvent::Closed(CloseReason::ZeroLength)]
        );

        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer"));
        r.feed(&frame(PREAMBLE).unwrap());
        assert_eq!(
            r.feed(&[0x00, 0x10, 0x00, 0x01]),
            vec![StreamEvent::Closed(CloseReason::OverCeiling(
                PEER_STREAM_MAX_FRAME_BYTES + 1
            ))]
        );
        assert!(r.buffer.is_empty(), "a closed reader holds nothing");
    }

    #[test]
    fn a_prefix_of_exactly_the_ceiling_is_accepted() {
        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer"));
        r.feed(&frame(PREAMBLE).unwrap());
        assert!(r.feed(&[0x00, 0x10, 0x00, 0x00]).is_empty());
        assert!(
            !r.is_closed(),
            "the ceiling is inclusive; the body is awaited"
        );
    }

    #[test]
    fn frames_before_a_fault_are_still_delivered() {
        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer"));
        let mut bytes = frame(PREAMBLE).unwrap();
        bytes.extend(frame(b"first").unwrap());
        bytes.extend([0, 0, 0, 0]);
        bytes.extend(frame(b"after the close").unwrap());
        let events = r.feed(&bytes);
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[1], StreamEvent::Message { body, .. } if body == b"first"));
        assert_eq!(events[2], StreamEvent::Closed(CloseReason::ZeroLength));
    }

    #[test]
    fn an_expected_address_must_match_exactly() {
        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer")).expecting("off1other");
        assert_eq!(
            r.feed(&frame(PREAMBLE).unwrap()),
            vec![StreamEvent::Closed(CloseReason::AddressMismatch {
                expected: "off1other".into(),
                derived: "off1peer".into(),
            })]
        );
        assert_eq!(r.peer(), None, "a mismatched preamble announces nothing");

        let mut r = PeerStreamReader::new(verifier(PREAMBLE, "off1peer")).expecting("off1peer");
        assert_eq!(
            r.feed(&frame(PREAMBLE).unwrap()),
            vec![StreamEvent::Announced("off1peer".into())]
        );
    }

    #[test]
    fn close_reasons_name_the_bound() {
        assert!(CloseReason::OverCeiling(5).to_string().contains("1048576"));
        assert!(CloseReason::PreambleUnderFloor(1)
            .to_string()
            .contains("96"));
    }
}
