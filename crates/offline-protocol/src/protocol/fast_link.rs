//! The engine's half of the fast link (ADR 0038).
//!
//! The state machine is [`crate::fast_link`]; this file is everything that
//! needs the engine. It checks a request may be made at all, seals each frame
//! in the 1:1 session and sends it once over the live direct link, routes a
//! decrypted frame in, feeds the machine the stream's proof, loss and
//! traffic, runs its timers on the engine's tick, turns its changes into
//! events, and wakes the platform when a directive is waiting.
//!
//! A frame is sealed with the strict encryptor and is never queued: a
//! confirmed session and a live link, or nothing (invariant 1 of
//! `docs/state-machines/fast-link.md`).

use offline_protocol_core::time::Instant;
use offline_protocol_core::MessagePriority;
use offline_protocol_transport::TransportType;
use rand_core::{OsRng, RngCore};
use tracing::{debug, warn};

use super::prefixes::internal_prefixes;
use super::OfflineProtocol;
use crate::fast_link::{
    CloseReason, FastLinkDirective, FastLinkRefusal, FastLinkRoles, Frame, FreshLink,
};
use crate::{Error, Event, Result};

impl OfflineProtocol {
    /// Declares what this device's platform can do, and whether its
    /// application takes part in links a peer starts. Until it does, all are
    /// false and every request is refused or answered `cannot_*`.
    pub fn set_fast_link_roles(
        &mut self,
        can_host: bool,
        can_join: bool,
        accept_unrequested: bool,
    ) {
        self.fast_links.set_roles(FastLinkRoles {
            can_host,
            can_join,
            accept_unrequested,
        });
    }

    /// Asks for a fast link to `peer`, and returns its id.
    ///
    /// Returns as soon as the first step is taken: this device starts
    /// hosting when it can, and otherwise asks the peer to host. The outcome
    /// arrives as `FastLinkChanged` events for `peer`, matched by peer rather
    /// than by this id, which changes once when the host answers with a
    /// network it already had. Refused when the peer has not
    /// advertised the capability, there is no confirmed session or no live
    /// direct link to it, a peer stream already proves it, this device can
    /// neither host nor join, or a link with another peer is held.
    pub fn request_fast_link(&mut self, peer: &str) -> Result<String> {
        if self.is_user_blocked(peer) {
            return Err(Error::UserBlocked(peer.to_string()));
        }
        if !self.peer_fast_link.contains(peer) {
            return Err(Error::InvalidArgument(format!(
                "{peer} has not advertised fast-link support"
            )));
        }
        if !self.confirmed_sessions.contains(peer) {
            return Err(Error::InvalidState(format!(
                "a fast link needs a confirmed session with {peer}"
            )));
        }
        self.direct_link_to(peer)?;
        if self.fast_links.peer() != Some(peer) && self.peer_stream_proves(peer) {
            return Err(Error::InvalidState(format!(
                "a peer stream already reaches {peer}"
            )));
        }
        let result = self
            .fast_links
            .request(peer, fresh_link(), Instant::now())
            .map_err(|refusal| match refusal {
                FastLinkRefusal::Unsupported => Error::InvalidState(
                    "this device can neither host nor join a fast link".to_string(),
                ),
                FastLinkRefusal::Busy => {
                    Error::InvalidState("a fast link with another peer is in progress".to_string())
                }
            });
        self.flush_fast_link();
        result
    }

    /// Ends the fast link with `peer`, if there is one. Returns whether there
    /// was.
    pub fn close_fast_link(&mut self, peer: &str) -> bool {
        let closed = self.fast_links.close(peer, Instant::now());
        self.flush_fast_link();
        closed
    }

    /// The engine is pausing (`background`) or stopping (`closed`): any link
    /// ends, the peer and this device's platform told.
    pub(crate) fn end_fast_link_for(&mut self, reason: CloseReason) {
        if let Some(peer) = self.fast_links.peer().map(str::to_string) {
            self.fast_links.end_for(&peer, reason, Instant::now());
            self.flush_fast_link();
        }
    }

    /// The user blocked `peer`: its link ends here and on this platform,
    /// with nothing sent to it.
    pub(crate) fn end_fast_link_with_blocked(&mut self, peer: &str) {
        if self.fast_links.forget(peer, Instant::now()) {
            self.flush_fast_link();
        }
    }

    /// The next instruction for the platform. Drained whenever the peer-stream
    /// callback fires.
    pub fn next_fast_link_directive(&mut self) -> Option<FastLinkDirective> {
        self.fast_links.next_directive(Instant::now())
    }

    /// The host's platform brought its network up, with its listener on
    /// `host:port`. The offer goes to the peer.
    pub fn fast_link_hosting(&mut self, link_id: &str, host: &str, port: u16) -> Result<()> {
        let accepted = self.fast_links.hosting(
            link_id,
            host,
            port,
            Instant::now(),
            chrono::Utc::now().timestamp_millis(),
        );
        self.flush_fast_link();
        if accepted {
            Ok(())
        } else {
            Err(Error::InvalidArgument(format!(
                "no fast link {link_id} waiting for its network, or {host}:{port} is not a \
                 private or link-local address"
            )))
        }
    }

    /// The platform could not do what it was told, or can no longer hold the
    /// link (`reason`: `declined`, `failed`, `host_failed`, `background`, ...).
    pub fn fast_link_ended(&mut self, link_id: &str, reason: &str) {
        self.fast_links
            .platform_ended(link_id, CloseReason::parse(reason), Instant::now());
        self.flush_fast_link();
    }

    /// A peer stream proved `peer`.
    pub fn fast_link_stream_proved(&mut self, peer: &str) {
        self.fast_links.stream_proved(peer, Instant::now());
        self.flush_fast_link();
    }

    /// The peer stream that proved `peer` ended.
    pub fn fast_link_stream_lost(&mut self, peer: &str) {
        self.fast_links.stream_lost(peer, Instant::now());
        self.flush_fast_link();
    }

    /// The peer-stream slot carried a body of `bytes` for `peer`.
    pub fn fast_link_activity(&mut self, peer: &str, bytes: usize) {
        self.fast_links.activity(peer, bytes, Instant::now());
    }

    /// A fast-link frame decrypted in the 1:1 session with `sender`, who is
    /// therefore its authenticated sender. `plaintext` still carries the
    /// prefix.
    pub(crate) fn handle_fast_link_frame(&mut self, sender: &str, plaintext: &str) {
        if self.is_user_blocked(sender) {
            return;
        }
        let Some(body) = plaintext.strip_prefix(internal_prefixes::FAST_LINK) else {
            return;
        };
        let Some(frame) = Frame::parse(body) else {
            debug!(sender = %sender, "Dropping a fast-link frame this version does not read");
            return;
        };
        let Some(local) = self.local_address().map(str::to_string) else {
            return;
        };
        self.fast_links.on_frame(
            sender,
            &local,
            frame,
            fresh_link(),
            Instant::now(),
            chrono::Utc::now().timestamp_millis(),
        );
        self.flush_fast_link();
    }

    /// The machine's timers, on the engine's tick.
    pub(crate) fn tick_fast_link(&mut self) {
        if self.fast_links.peer().is_none() {
            return;
        }
        let busy: std::collections::HashSet<String> = self
            .outbound_media_transfers
            .values()
            .filter(|transfer| Self::holds_fast_link(transfer))
            .map(|transfer| transfer.recipient.clone())
            .collect();
        self.fast_links
            .tick(Instant::now(), |peer| busy.contains(peer));
        self.flush_fast_link();
    }

    /// Whether an outbound transfer holds its recipient's fast link off the
    /// idle limit: only one pinned to the peer-stream slot. A transfer's
    /// carrier is fixed when it starts, so one begun over Bluetooth before
    /// the link came up never moves to it, and holding the link for it kept
    /// an iPhone off its own Wi-Fi for nothing.
    pub(crate) fn holds_fast_link(transfer: &super::types::OutboundMediaTransfer) -> bool {
        transfer.pinned_transport == TransportType::WiFiDirect
    }

    /// A link to `peer` that was up has ended: each transfer to it pinned to
    /// the slot fails now, with its chunks withdrawn. Its carrier is fixed
    /// when it starts, so it would otherwise retry on a slot that no longer
    /// reaches the peer until its retries ran out, and its chunks are sized
    /// for the stream, larger than Bluetooth carries. The application hears
    /// `media_send_failed` and can send again over what remains.
    fn fail_transfers_on_ended_link(&mut self, peer: &str) {
        let files: Vec<String> = self
            .outbound_media_transfers
            .iter()
            .filter(|(_, transfer)| transfer.recipient == peer && Self::holds_fast_link(transfer))
            .map(|(file_id, _)| file_id.clone())
            .collect();
        for file_id in files {
            let chunks: Vec<_> = self
                .outbound_media_chunks
                .iter()
                .filter(|(_, (chunk_file, _))| *chunk_file == file_id)
                .map(|(message_id, _)| message_id.clone())
                .collect();
            for message_id in &chunks {
                self.forget_outbound_message(message_id);
            }
            self.abort_outbound_media_transfer(&file_id, "the fast link carrying it closed");
        }
    }

    /// Whether a peer stream holds `peer` now.
    fn peer_stream_proves(&self, peer: &str) -> bool {
        self.transport_manager
            .get_transport(TransportType::WiFiDirect)
            .is_some_and(|transport| {
                transport
                    .connected_peers()
                    .iter()
                    .any(|link| link.peer_id == peer)
            })
    }

    /// Sends what the machine queued for the peer, emits its changes, and
    /// wakes the platform when a directive waits.
    fn flush_fast_link(&mut self) {
        for (peer, frame) in self.fast_links.take_frames() {
            let content = format!("{}{}", internal_prefixes::FAST_LINK, frame.encode());
            if let Err(err) = self.send_fast_link_frame(&peer, &content) {
                // Not retried: a frame that cannot leave now names a moment
                // that has passed. The other side's timers end its half.
                debug!(peer = %peer, error = %err, "Fast-link frame not sent");
            }
        }
        for change in self.fast_links.take_changes() {
            self.emit_event(Event::fast_link_changed(change));
        }
        for peer in self.fast_links.take_released() {
            self.fail_transfers_on_ended_link(&peer);
        }
        if self.fast_links.has_directives() {
            self.wake_peer_stream_platform();
        }
    }

    /// Sealed with the strict encryptor and sent once over a live direct
    /// link, Bluetooth LE first, with no acknowledgement, outbox or retry (ADR 0032). Fails
    /// before encrypting when there is no link, so no ratchet generation is
    /// spent on a frame that cannot leave.
    fn send_fast_link_frame(&mut self, peer: &str, content: &str) -> Result<()> {
        // Bluetooth when the peer is linked there: the link this frame
        // starts, stops or replaces may be the peer stream itself, and a
        // close sent down a stream being torn down, or a request down a LAN
        // stream the peer has just left, arrived 22 to 63 s late on two
        // phones, or not at all.
        let transport = if self
            .transport_manager
            .holds_mesh_link(TransportType::BLE, peer)
        {
            TransportType::BLE
        } else {
            self.direct_link_to(peer)?
        };
        let sealed = self.encrypt_content_for_recipient_strict(peer, content)?;
        let mut message = self.create_message(peer, sealed, Some(MessagePriority::High), None)?;
        message.requires_ack = false;
        self.deduplicator.mark_seen_local(message.id.clone());
        if let Err(err) = self
            .transport_manager
            .send_via_transport(&message, transport)
        {
            self.transport_manager.record_retry_failure(transport);
            return Err(err);
        }
        Ok(())
    }

    /// Wakes the platform through the peer-stream transport's callback, the
    /// one it already drains bodies on.
    fn wake_peer_stream_platform(&self) {
        let Some(transport) = self
            .transport_manager
            .get_transport(TransportType::WiFiDirect)
        else {
            warn!("A fast-link directive waits, but there is no peer-stream transport to wake");
            return;
        };
        if let Some(wifi_direct) = transport
            .as_any()
            .downcast_ref::<offline_protocol_transport::WifiDirectTransport>()
        {
            wifi_direct.wake_platform();
        }
    }
}

/// A link id and credentials from the operating system's random source.
fn fresh_link() -> FreshLink {
    FreshLink::generate(|buf| OsRng.fill_bytes(buf))
}
