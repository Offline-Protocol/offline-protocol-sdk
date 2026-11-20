//! A WebRTC Direct listener: a native device a page on its network dials
//! from a code the device shows.
//!
//! A page cannot open the TCP socket a phone's peer streams use, but it can
//! open a WebRTC data channel. This crate is the listener's side of that
//! dial: an ICE-lite, controlled agent and DTLS server on one UDP port with
//! a self-signed certificate, which writes a [`DialCode`] and turns each
//! dial into an ordinary peer stream. The format and the rules are
//! `docs/spec/stream-framing.md`, section "Dialling a listener from a
//! browser"; the decisions and the alternatives refused are ADR 0039
//! (`docs/adr/0039-a-browser-dials-a-native-device-from-a-code.md`).
//!
//! # Invariants
//!
//! - **The code is the whole setup.** No server, relay or second code is
//!   involved, and the listener never sees the page's session description.
//!   It learns the page's address from the source of its first check and
//!   the page's ICE username from that check's USERNAME.
//! - **The page never edits its own local description.** It builds the
//!   listener's description from the code and nothing else, so the
//!   listener chooses its own ICE username and password and checks every
//!   request under them. A dial that depends on editing the local username
//!   or password breaks wherever a browser refuses that edit, as libwebrtc
//!   is moving to do.
//! - **The listener keeps state only for an authenticated check.** A
//!   datagram that belongs to no dial makes the listener allocate or reply
//!   only if it is a binding request for its ufrag whose MESSAGE-INTEGRITY
//!   verifies under its password. Anything else costs at most one HMAC and
//!   gets no reply, so the listener is neither a memory sink nor a
//!   reflector for the rest of the network.
//! - **A DTLS certificate is never an identity.** The page checks the
//!   listener's certificate against the code; the listener does not check
//!   the page's, having nothing to compare it with. Who is on either end is
//!   what the preamble proves: each side sends its identity assertion first
//!   on the channel, and [`ListenerEvent::Proved`] carries the address the
//!   page's assertion derives to, never a claimed one.
//!
//! # Shape
//!
//! Sans-I/O and std-only. The platform owns the UDP socket (so it can bind
//! it to the Wi-Fi network) and the clock; [`Listener`] takes datagrams and
//! timer ticks and gives back datagrams and three events: a stream proved
//! an address, a body arrived, a proved stream ended. The bridge above it
//! keeps its stream table and its one-stream-per-address rule, and calls
//! `wifi_direct_*` as it does for a TCP stream. Framing and the preamble
//! run through the one `PeerStreamReader` and `frame` in
//! `offline-protocol-transport`; this crate has no codec of its own.
//!
//! WebRTC comes from str0m. On Apple platforms it uses Apple's crypto,
//! elsewhere aws-lc-rs.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod dial_code;
mod listener;
mod nomination;
mod stun;

pub use dial_code::{DialCode, DialCodeError};
pub use listener::{
    Listener, ListenerConfig, ListenerError, ListenerEvent, StreamId, Transmit, CONSENT_TIMEOUT,
    HIGH_WATER_BYTES, MAX_CHUNK_BYTES, MAX_PENDING_DIALS, MAX_QUEUED_BYTES, PROVE_DEADLINE,
};
