//! Message receive loop and file chunk handling.

use super::mesh_relay::RelayAdmission as MeshRelayAdmission;
use super::{
    internal_prefixes, lock_shared_state, ChunkOutcome, InternalMessageResult, OfflineProtocol,
    PendingMediaMetadataEntry, ProtocolState, RichPayloadV1, CRITICAL_RELAY_BATTERY_LEVEL,
};
use crate::constants::ACK_FOR_KEY;
use crate::events::{Event, SecurityWarningCode};
use crate::file_transfer::FileChunk;
use crate::media_envelope::{decode_media_envelope, is_media_envelope, MediaChunkPlaintext};
use crate::mls_observability::{DecryptionFailureKind, MlsErrorCategory, MlsOperationContext};
use crate::SessionStateError;
use offline_protocol_core::{ContentType, MediaMetadata, Message};
use offline_protocol_mls::{EncryptedMessage, GroupId};
use offline_protocol_router::relay::RelayPriority;
use offline_protocol_transport::TransportType;
use std::time::Instant;
use tracing::{debug, error, info, warn};

impl OfflineProtocol {
    /// Applies a successful MLS decryption to an inbound message: swaps the
    /// ciphertext for the plaintext, marks the message encrypted, drops the
    /// outer `reply_context`, and restores rich fields from a sealed
    /// `__RICH_V1__` body when present.
    ///
    /// The outer `reply_context` field is hop-visible cleartext that any
    /// relay can inject or rewrite in transit — it sits outside the MLS AEAD
    /// boundary. The strip happens unconditionally *before* the sealed-body
    /// restore below, so the encrypted envelope is the only trusted carrier
    /// for reply context on encrypted messages. Without this strip, a
    /// `MessageReceived { encrypted: true }` event could surface an
    /// attacker-controlled quote preview as if it were part of the
    /// authenticated conversation.
    ///
    /// Sealed-body parsing is never capability-gated (mirroring envelope
    /// parsing): whatever a peer chose to seal, we try to read. On a rich
    /// message the sealed body is authoritative for `reply_context`,
    /// `media_metadata`, and `forwarded_from` — the relay-writable outer
    /// copies are overwritten wholesale, `None`s included — and for
    /// `content_type` when the body carries one (absent from bodies sealed
    /// by senders predating the field, where the outer hint stands). The
    /// sealed `content_type` restore runs before the receive loop's
    /// `FileChunk` routing, so a relay restamping an encrypted rich
    /// message's outer hint can no longer misroute it; `FileChunk` itself
    /// is refused from the sealed body (mirroring the send boundary), or a
    /// hostile sender could steer an ordinary message into the
    /// file-transfer manager, which drops it. A body that fails to parse
    /// (including a hostile `reply_context.sender` rejected by `UserId`
    /// validation) surfaces as raw text with a warning rather than
    /// dropping an authenticated message.
    pub(super) fn apply_decrypted_content(message: &mut Message, plaintext: String) {
        message.reply_context = None;
        if plaintext.starts_with(internal_prefixes::RICH_V1) {
            match RichPayloadV1::parse_sealed(&plaintext, message.sender.as_str()) {
                Some(rich) => {
                    message.content = rich.text;
                    message.reply_context = rich.reply_context;
                    message.media_metadata = rich.media_metadata;
                    message.forwarded_from = rich.forward_info;
                    // `parse_sealed` already refused a FileChunk claim; a
                    // remaining None keeps the outer value (bodies sealed by
                    // senders predating the field).
                    if let Some(content_type) = rich.content_type {
                        message.content_type = content_type;
                    }
                }
                None => {
                    message.content = plaintext;
                }
            }
        } else {
            message.content = plaintext;
        }
        message
            .metadata
            .insert("encrypted".to_string(), "true".to_string());
    }

    /// Receives the next available message.
    pub fn receive_message(&mut self) -> Option<Message> {
        let Ok(mut state) = lock_shared_state(&self.shared_state) else {
            error!("Failed to lock shared state in receive_message");
            return None;
        };
        let protocol_running = state.state == ProtocolState::Running;

        if !state.received_messages.is_empty() {
            return state.received_messages.pop_front();
        }

        drop(state);

        // Drive confirmation maintenance from receive polling as an additional
        // liveness source when the app does not call process() on a timer.
        // Uses the same throttle as process() to avoid redundant storage I/O.
        if protocol_running {
            self.run_throttled_reconciliation("receive_message_poll");
        }

        loop {
            match self.transport_manager.receive() {
                Ok(Some((transport_used, mut message))) => {
                    // The peer whose link this frame physically arrived on.
                    // Distinct from `message.sender`, which is who wrote it:
                    // the two agree only at the first hop. `None` when the
                    // carrier does not identify links (Nostr) or the platform
                    // did not supply an id.
                    let arrival_peer = message.transport_peer_id().map(str::to_string);

                    // Traffic for someone else is forwarded, not processed, and
                    // that decision comes first.
                    //
                    // Everything below this point treats the frame as part of
                    // our own exchange: it merges the sender's clock into ours,
                    // settles our outbox from acknowledgements, and consumes the
                    // frame. None of that is true of a frame merely passing
                    // through us — its acknowledgements belong to the pair it
                    // travels between, and once a node carries the whole
                    // neighborhood's traffic, absorbing every clock it sees
                    // would drag ours to the network's maximum.
                    //
                    // Never forward a frame claiming our own origin: a genuine
                    // self-originated frame is never received inbound with a
                    // foreign recipient (the send path does not loop back), so
                    // this is a routing loop or a forgery aimed at
                    // `internet_control_op`'s self-origination gate — re-issuing
                    // it from our outbox would let a mesh peer drive
                    // relay-native control ops on our authenticated relay
                    // connection. (A sender==self frame addressed *to* us is the
                    // legitimate relay echo and falls through unchanged.)
                    if message.recipient.as_str() != self.local_id {
                        if message.sender.as_str() == self.local_id {
                            debug!(
                                message_id = %message.id,
                                recipient = %message.recipient,
                                "Dropping inbound frame forging our own origin"
                            );
                            continue;
                        }
                        self.try_relay_message(&message, arrival_peer.as_deref());
                        continue;
                    }

                    // Block filter (early): check before any side-effects so
                    // that blocked users cannot advance our Lamport clock,
                    // leak our presence via re-ACK, or trigger any processing.
                    let sender_blocked = self.is_user_blocked(message.sender.as_str());

                    // Merge Lamport clock for every non-blocked received message
                    // — including duplicates, ACKs, and internal protocol
                    // messages — so the local clock always advances past any
                    // observed peer value.
                    if !sender_blocked && message.lamport_clock.value() > 0 {
                        self.lamport_clock.merge(message.lamport_clock);
                        self.persist_lamport_clock();
                    }

                    if message.metadata.contains_key(ACK_FOR_KEY) {
                        if !sender_blocked {
                            self.handle_ack_message(&message);
                        }
                        continue;
                    }

                    if self.deduplicator.is_duplicate_mut(&message.id) {
                        // Re-ACK duplicate packets so the sender can stop
                        // retrying — but NOT for blocked users, to avoid
                        // leaking presence information.
                        //
                        // Every copy is answered, including the several that
                        // one delivery produces when it travels through nearby
                        // devices. Answering only the first was tried and
                        // rejected: a copy arriving moments later and a
                        // retransmission arriving because the answer was lost
                        // are not distinguishable here, and staying quiet for
                        // the second turns a delivered message into a failed
                        // one. The redundant answers are bounded — a delivery
                        // arrives by at most as many paths as neighbors carried
                        // it, answers never provoke further answers, and every
                        // frame that leaves is rate-capped like any other.
                        if !sender_blocked && message.requires_ack {
                            if let Err(err) = self.send_delivery_ack(&message, transport_used) {
                                error!(
                                    message_id = %message.id,
                                    error = %err,
                                    "Failed to send delivery ACK for duplicate message"
                                );
                            }
                        }
                        continue;
                    }

                    // Block filter: silently drop messages from blocked users
                    // addressed to us. No ACK, no event, no side-effects.
                    // Checked before mark_seen so that if the user is later
                    // unblocked, retransmissions can still be delivered.
                    if sender_blocked {
                        debug!(
                            sender = %message.sender,
                            message_id = %message.id,
                            "Dropping message from blocked user"
                        );
                        continue;
                    }

                    self.deduplicator.mark_seen(message.id.clone());

                    // Handle internal MLS messages
                    let mut was_decrypted = false;
                    if let Some(result) =
                        self.process_internal_message_via(&message, Some(transport_used))
                    {
                        match result {
                            InternalMessageResult::Consumed => {
                                // Internal control messages are still delivery-sensitive for
                                // the sender (invites/accept/welcome). ACK before consume.
                                if message.requires_ack {
                                    if let Err(err) =
                                        self.send_delivery_ack(&message, transport_used)
                                    {
                                        error!(
                                            message_id = %message.id,
                                            error = %err,
                                            "Failed to send delivery ACK for internal message"
                                        );
                                    }
                                }
                                // Internal message handled, don't surface to app
                                continue;
                            }
                            InternalMessageResult::SecurityRejected => {
                                // Security gate rejected this message (spoofed sender,
                                // bad signature, unsigned control traffic, or a signing
                                // key that does not derive to the claimed sender
                                // address). Do NOT send a delivery
                                // ACK — acknowledging would confirm to the attacker that
                                // the target peer is online and processing messages.
                                // Forget the id too, or an exact replay would hit the
                                // duplicate re-ACK path above and leak that presence
                                // anyway; reprocessing a replay costs no more than a
                                // fresh forged message.
                                self.deduplicator.unmark_seen(&message.id);
                                continue;
                            }
                            InternalMessageResult::Deferred => {
                                // The message was not delivered, but the sender
                                // can still recover it by resending: it could not
                                // be decrypted *yet* (session not ready, so queued
                                // for delayed decryption), or it is undecryptable
                                // as it stands (epoch desync, or a hard crypto
                                // failure while `crypto_recovery_enabled`) and was
                                // dropped without queueing. See the three
                                // conditions on `InternalMessageResult::Deferred`.
                                //
                                // Do NOT send a delivery ACK and do NOT keep the id
                                // dedup-marked: the message is not delivered, so the
                                // sender must be free to retry and that retry must
                                // re-enter processing rather than hit the duplicate
                                // re-ACK path above. For a queued copy, the session
                                // confirming drains it: the message is surfaced and
                                // the id re-marked (see `process_pending_decryption`),
                                // so the sender's next resend is then deduped +
                                // re-ACKed. For the other two, recovery is that
                                // resend itself — re-sealed against a live
                                // generation by Tier 2.
                                self.deduplicator.unmark_seen(&message.id);
                                continue;
                            }
                            InternalMessageResult::Decrypted(plaintext) => {
                                was_decrypted = true;
                                Self::apply_decrypted_content(&mut message, plaintext);
                            }
                        }
                    }

                    // Policy gate for inbound plaintext (SEC: inbound text must
                    // honor require_encryption exactly like legacy media does).
                    // A message that reaches this point without decryption is
                    // unauthenticated cleartext with an attacker-controllable
                    // sender. FileChunk messages are exempt: encrypted media
                    // envelopes carry no content prefix (the ciphertext rides
                    // in binary_content), so they are indistinguishable from
                    // plaintext here — handle_incoming_file_chunk applies this
                    // same gate after telling the two apart. Rejection sends
                    // no delivery ACK, mirroring SecurityRejected: don't
                    // confirm to an injector that the target processes their
                    // messages. The id is forgotten so a replay re-enters
                    // this gate instead of the duplicate re-ACK path.
                    if !was_decrypted
                        && message.content_type != ContentType::FileChunk
                        && !self.accept_plaintext_content(message.sender.as_str())
                    {
                        warn!(
                            message_id = %message.id,
                            sender = %message.sender,
                            "Rejecting unencrypted inbound message (encryption policy)"
                        );
                        self.warn_plaintext_receive_rejected(
                            message.sender.as_str(),
                            "Inbound plaintext message rejected by encryption policy",
                        );
                        self.deduplicator.unmark_seen(&message.id);
                        continue;
                    }

                    // Route file-chunk messages to the transfer manager BEFORE
                    // the delivery ACK, and never surface them to the app as
                    // regular messages. An encrypted chunk that was not
                    // delivered but that the sender can still recover by
                    // resending — not decryptable yet (session not ready, so
                    // queued for delayed decryption), or undecryptable as it
                    // stands (epoch desync, crypto failure) — returns
                    // `Deferred`: it must NOT be ACKed and its id is unmarked,
                    // so the sender keeps retrying and the resend re-enters
                    // processing, matching the text `Deferred` path. Every other
                    // outcome (`Handled`: decrypted/assembled, or a terminal
                    // drop) is ACKed as before, since the sender cannot recover
                    // it by retrying.
                    if message.content_type == ContentType::FileChunk {
                        match self.handle_incoming_file_chunk_via(&message, Some(transport_used)) {
                            ChunkOutcome::Deferred => {
                                self.deduplicator.unmark_seen(&message.id);
                            }
                            // Plaintext chunk rejected by encryption policy:
                            // withhold the ACK and unmark the id, exactly like
                            // the plaintext-text rejection above and the text
                            // `SecurityRejected` path — don't confirm to an
                            // injector that we process their messages, and let a
                            // replay re-enter the gate rather than hit the
                            // duplicate re-ACK path.
                            ChunkOutcome::Rejected => {
                                self.deduplicator.unmark_seen(&message.id);
                            }
                            ChunkOutcome::Handled => {
                                if message.requires_ack {
                                    if let Err(err) =
                                        self.send_delivery_ack(&message, transport_used)
                                    {
                                        error!(
                                            message_id = %message.id,
                                            error = %err,
                                            "Failed to send delivery ACK for media chunk"
                                        );
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    if message.requires_ack {
                        if let Err(err) = self.send_delivery_ack(&message, transport_used) {
                            error!(
                                message_id = %message.id,
                                error = %err,
                                "Failed to send delivery ACK"
                            );
                        }
                    }

                    let forward_info = message
                        .forwarded_from
                        .as_ref()
                        .map(crate::events::ForwardInfoEvent::from);

                    let event = Event::MessageReceived {
                        message_id: message.id.as_str(),
                        sender: message.sender.as_str().to_string(),
                        recipient: message.recipient.as_str().to_string(),
                        content: message.content.clone(),
                        hop_count: message.hop_count.value(),
                        transport: transport_used.to_string(),
                        timestamp: message.timestamp.as_millis(),
                        lamport_clock: message.lamport_clock.value(),
                        reply_to_msg: message
                            .reply_to_msg
                            .as_ref()
                            .map(|id| id.as_str().to_string()),
                        reply_context: message
                            .reply_context
                            .as_ref()
                            .map(|rc| Box::new(crate::events::ReplyContextEvent::from(rc))),
                        content_type: message.content_type.to_string(),
                        media_metadata: message.media_metadata.clone(),
                        forward_info,
                        encrypted: was_decrypted,
                    };

                    let Ok(state) = lock_shared_state(&self.shared_state) else {
                        error!("Failed to lock shared state for message received event");
                        return None;
                    };
                    state.emit_event(event);
                    drop(state);

                    return Some(message);
                }
                Ok(None) => return None,
                Err(err) => {
                    error!(error = %err, "Transport receive error");
                    return None;
                }
            }
        }
    }

    /// Considers an inbound frame addressed to a third party for forwarding.
    ///
    /// Learns the route back toward the sender, then offers the frame to the
    /// governor, which decides whether it travels any further. Nothing is
    /// transmitted here: an accepted frame is queued and goes out from
    /// [`Self::flush_mesh_relays`] once its delay elapses, which is what gives
    /// a neighbor the chance to cover it first.
    ///
    /// `arrival_peer` is the neighbor that handed us this frame, when the
    /// carrier identified the link.
    fn try_relay_message(&mut self, message: &Message, arrival_peer: Option<&str>) {
        // Cheapest answer first. This device sees the whole neighborhood's
        // traffic, so in a crowded room most third-party arrivals are copies of
        // a frame it has already dealt with — and everything below would be
        // thrown away by the suppression check inside `admit`: a route-table
        // write, a battery snapshot that locks and allocates across every
        // transport, and an enumeration of every link. Answering a duplicate
        // costs a hash lookup instead.
        //
        // Route learning is skipped along with the rest, which is deliberate:
        // the copy teaches nothing the first one did not, and the table is not
        // what forwarding decisions are made from. A duplicate we still hold a
        // pending copy of is *not* absorbed here — standing down for a neighbor
        // needs the neighbor set — so that case still takes the full path.
        if self
            .mesh_relay
            .absorb_settled_duplicate(&message.id.as_str())
        {
            return;
        }

        // Carrying traffic for others costs this device's radio and battery,
        // so it stays a local decision.
        let relay_allowed = self.config.relay.allow_relay
            && self.config.relay.relay_priority != RelayPriority::Never;

        if !relay_allowed {
            debug!(
                message_id = %message.id,
                "Not forwarding: relaying is disabled on this device"
            );
            return;
        }

        // Carrying other people's messages is the first thing to give up when
        // the battery is going: a device that spends its last few percent
        // relaying cannot send its own message when its owner needs to. The
        // device keeps sending and receiving its own traffic either way.
        //
        // An unknown battery level is treated as willing — most platforms
        // report one, and refusing to carry anything on a device that simply
        // does not publish a level would quietly remove it from the network.
        if !self.battery_allows_relaying() {
            debug!(
                message_id = %message.id,
                "Not forwarding: battery is below the level for carrying traffic"
            );
            return;
        }

        let neighbors = self.transport_manager.mesh_neighbors();
        let degree = neighbors.len();
        // Whether we hold the link to the recipient ourselves. At the last hop
        // no other device can be assumed to have it, so our copy must not be
        // dropped in favour of a neighbor's.
        let is_last_hop = neighbors
            .iter()
            .any(|n| n.peer_id == message.recipient.as_str());
        match self
            .mesh_relay
            .admit(message, arrival_peer, degree, is_last_hop)
        {
            MeshRelayAdmission::Queued => {
                debug!(
                    message_id = %message.id,
                    recipient = %message.recipient,
                    degree,
                    "Queued frame for forwarding"
                );
            }
            MeshRelayAdmission::Rejected(reason) => {
                debug!(
                    message_id = %message.id,
                    ?reason,
                    "Not forwarding"
                );
            }
        }
    }

    /// Whether the battery is healthy enough to carry other people's traffic.
    ///
    /// Takes its own battery reading; the per-tick caller already holds one and
    /// uses [`Self::battery_allows_relaying_with`] instead.
    fn battery_allows_relaying(&self) -> bool {
        let (_statuses, available) = self.transport_manager.snapshot_status_and_available();
        let (battery_level, is_charging) =
            crate::telemetry::aggregator::device_battery_from_available(
                self.transport_manager.current_transport(),
                &available,
            );
        self.battery_allows_relaying_with(battery_level, is_charging)
    }

    /// [`Self::battery_allows_relaying`] against a reading the caller already
    /// has.
    ///
    /// An unknown level means yes — see [`Self::try_relay_message`].
    pub(super) fn battery_allows_relaying_with(
        &self,
        battery_level: Option<u8>,
        is_charging: bool,
    ) -> bool {
        let Some(level) = battery_level else {
            return true;
        };
        level >= self.relay_battery_floor(is_charging)
    }

    /// The battery level below which this device stops carrying other people's
    /// traffic.
    ///
    /// [`RelayConfig::min_battery_for_relay`] is the floor in the ordinary
    /// case, relaxed to the hard [`CRITICAL_RELAY_BATTERY_LEVEL`] for a device
    /// that is either charging or configured [`RelayPriority::Always`] — the
    /// two ways of saying "keep relaying anyway". The hard floor is never
    /// crossed: it applies even to a configured minimum set below it, because
    /// spending the last few percent on strangers' traffic leaves a device
    /// unable to send its own.
    pub(super) fn relay_battery_floor(&self, is_charging: bool) -> u8 {
        let eager = matches!(self.config.relay.relay_priority, RelayPriority::Always);
        if is_charging || eager {
            CRITICAL_RELAY_BATTERY_LEVEL
        } else {
            self.config
                .relay
                .min_battery_for_relay
                .max(CRITICAL_RELAY_BATTERY_LEVEL)
        }
    }

    /// Transmits the forwards whose delay has elapsed.
    ///
    /// Called from the process tick. Each frame goes to a bounded set of
    /// neighbors chosen by the governor, never back to the peer it came from or
    /// to the peer that wrote it. A frame whose recipient is a neighbor of ours
    /// is handed straight to them instead — the shortest path we can see.
    pub(super) fn flush_mesh_relays(&mut self) {
        let due = self.mesh_relay.take_due(Instant::now());
        if due.is_empty() {
            return;
        }

        let neighbors = self.transport_manager.mesh_neighbors();

        // `relay` is borrowed rather than destructured throughout: a frame that
        // reaches no neighbor has to be handed back whole, and cloning the
        // message to keep that option open would copy the payload — up to a
        // whole media chunk — on every flush.
        for relay in due {
            let message = &relay.message;
            let recipient = message.recipient.as_str().to_string();
            let message_id = message.id.as_str();
            let hop_count = message.hop_count.value();
            let remaining_ttl = message.ttl.value();

            let mut exclude: Vec<&str> = vec![message.sender.as_str()];
            if let Some(peer) = relay.arrival_peer.as_deref() {
                exclude.push(peer);
            }
            let onward = self.mesh_relay.select_targets(
                neighbors
                    .iter()
                    .map(|n| (n.peer_id.as_str(), n.link_quality())),
                &exclude,
                &message_id,
            );

            // If the destination is one of our own neighbors, hand it over
            // directly: no fan-out is worth more than arriving. Should that
            // link fail between choosing it and writing to it, fall back to
            // carrying it onward rather than dropping a frame we could still
            // move.
            let targets = if neighbors.iter().any(|n| n.peer_id == recipient) {
                let mut ordered = vec![recipient.clone()];
                ordered.extend(onward.into_iter().filter(|peer| peer != &recipient));
                ordered
            } else {
                onward
            };
            let deliver_direct = targets.first() == Some(&recipient);

            if targets.is_empty() {
                // Nowhere to hand it right now: we hold no link, or the only
                // ones we hold are the peers this frame must not go back to.
                // Falls through to the hand-back below rather than being
                // dropped — a neighbor may well appear within the few seconds
                // the frame is still worth carrying.
                debug!(
                    message_id = %message.id,
                    "No onward neighbor for this frame"
                );
            }

            let mut delivered_to = 0usize;
            for target in &targets {
                // Each link this frame crosses is one transmission against the
                // device's ceiling. Running out mid-fan-out stops the fan-out
                // rather than the frame: the neighbors already reached carry it
                // on, and the sender's retry covers the rest.
                if !self.mesh_relay.take_send_token() {
                    debug!(
                        message_id = %message.id,
                        "At the forwarding limit; stopping this fan-out here"
                    );
                    break;
                }

                match self.transport_manager.send_to_neighbor(target, message) {
                    Ok(transport) => {
                        delivered_to += 1;
                        // Handing it to the recipient themselves ends the
                        // journey; the remaining neighbors are only a fallback
                        // for that link failing.
                        if deliver_direct && target == &recipient {
                            debug!(
                                message_id = %message.id,
                                next_hop = %target,
                                transport = ?transport,
                                "Delivered to its recipient directly"
                            );
                            break;
                        }
                        debug!(
                            message_id = %message.id,
                            next_hop = %target,
                            transport = ?transport,
                            hop_count,
                            remaining_ttl,
                            "Forwarded frame to neighbor"
                        );
                    }
                    Err(err) => {
                        // A link that went away between selection and send.
                        // The other targets still carry the frame.
                        debug!(
                            message_id = %message.id,
                            next_hop = %target,
                            error = %err,
                            "Could not forward to neighbor"
                        );
                    }
                }
            }

            // Nothing reached a neighbor. Hand it back instead of dropping it:
            // its id is already recorded as handled here, so a drop would lose
            // this copy *and* refuse both the copies arriving behind it and the
            // sender's retransmissions, for the whole retention window.
            //
            // Why it got nowhere does not change that. The budget ran out (the
            // frames released alongside this one spent the allowance between
            // them), every link chosen for it went away between being picked
            // and being written to, or there was no usable link to pick. In all
            // three the frame has travelled nowhere and this queue is the only
            // thing that still remembers it. It keeps its due time, so one that
            // stays stuck is abandoned by the overdue cut-off rather than
            // retried forever.
            if delivered_to == 0 {
                self.mesh_relay.requeue(relay);
                continue;
            }

            self.mesh_relay.record_forwarded();
            self.emit_event(Event::message_relayed(
                message.id.as_str(),
                message.sender.as_str().to_string(),
                recipient,
                hop_count,
                remaining_ttl,
            ));
        }
    }

    /// [`Self::handle_incoming_file_chunk_via`] with no known arrival transport
    /// (a deferred chunk falls back to the resend-driven ACK). Used only by
    /// tests; production always routes through the `_via` form with the inbound
    /// transport.
    #[cfg(test)]
    pub(super) fn handle_incoming_file_chunk(&mut self, message: &Message) -> ChunkOutcome {
        self.handle_incoming_file_chunk_via(message, None)
    }

    /// Routes an inbound file-chunk message through the transfer manager,
    /// recording `arrival_transport` on the pending entry if the chunk must be
    /// deferred, so the drain can ACK it directly once the session confirms.
    pub(super) fn handle_incoming_file_chunk_via(
        &mut self,
        message: &Message,
        arrival_transport: Option<TransportType>,
    ) -> ChunkOutcome {
        let sender = message.sender.as_str().to_string();

        // SEC-H1: encrypted chunks carry the chunk bytes, media metadata, and
        // original content type inside the MLS ciphertext; legacy plaintext
        // chunks carry them on the wire Message and are accepted only where
        // policy allows. Rich extras (caption, reply, forward) only ever come
        // from the sealed plaintext — never from the wire Message.
        let (chunk, media_metadata, original_content_type, rich_extras, data_purpose) =
            if let Some(ref binary) = message.binary_content {
                if is_media_envelope(binary) {
                    let encrypted = match decode_media_envelope(binary) {
                        Ok(e) => e,
                        // A frame whose magic byte still reads as a media envelope
                        // but whose encoding does not decode — in-transit
                        // corruption past the magic, or injected garbage. Under
                        // crypto recovery this is the pre-decrypt sibling of a hard
                        // decrypt failure: the frame as it stands is dead, but the
                        // sender's resend carries a fresh encoding that would
                        // decode, so withhold the ACK (`Deferred`) instead of
                        // telling the sender "delivered" for a chunk we dropped.
                        // Do NOT enqueue: an unparseable frame can never become
                        // parseable, so a queued copy could never drain — the same
                        // reasoning as the spent-generation arm below.
                        Err(e) => {
                            warn!(
                                message_id = %message.id,
                                error = %e,
                                "Failed to decode encrypted media envelope"
                            );
                            if !self.config.encryption.crypto_recovery_enabled {
                                return ChunkOutcome::Handled;
                            }
                            // Advisory, not terminal — the chunk was never ACKed, so
                            // the transfer is *stalled* pending a resend rather than
                            // failed. The terminal media signal stays
                            // `FileReceiveFailed`.
                            if let Ok(state) = lock_shared_state(&self.shared_state) {
                                state.emit_event(Event::message_decryption_failed(
                                message.id.clone(),
                                sender.clone(),
                                crate::events::DecryptionFailureCode::InvalidPayload,
                                "encrypted media envelope failed to decode; its file transfer is stalled until the sender resends".to_string(),
                            ));
                            }
                            return ChunkOutcome::Deferred;
                        }
                    };
                    let plaintext = match self.decrypt_media_chunk(
                        &sender,
                        &encrypted,
                        message,
                        arrival_transport,
                    ) {
                        MediaChunkDecrypt::Plaintext(p) => p,
                        // Not delivered and recoverable by a resend (queued for
                        // delayed decryption, or an undecryptable frame the
                        // sender can re-send): defer the ACK so the sender
                        // retries and the resend re-enters processing.
                        MediaChunkDecrypt::Deferred => return ChunkOutcome::Deferred,
                        // Illegitimate frame (foreign session slot, or an MLS
                        // credential naming a different sender): answer with
                        // silence, exactly like the text `SecurityRejected`
                        // path. ACKing here would tell an injector that the
                        // target is online and processing their frames — the
                        // one thing the text path withholds.
                        MediaChunkDecrypt::SecurityRejected => return ChunkOutcome::Rejected,
                        // Terminal drop (permanent refusal, empty plaintext, MLS
                        // unavailable): ACK as before — the sender cannot
                        // recover it by retrying.
                        MediaChunkDecrypt::Dropped => return ChunkOutcome::Handled,
                    };
                    let inner = match MediaChunkPlaintext::decode(&plaintext) {
                        Ok(i) => i,
                        Err(e) => {
                            warn!(
                                message_id = %message.id,
                                error = %e,
                                "Failed to parse decrypted media chunk plaintext, dropping"
                            );
                            return ChunkOutcome::Handled;
                        }
                    };
                    let chunk = match FileChunk::from_bytes(&inner.chunk_bytes) {
                        Ok(c) => c,
                        Err(e) => {
                            warn!(
                                message_id = %message.id,
                                error = %e,
                                "Failed to deserialize decrypted file chunk, dropping"
                            );
                            return ChunkOutcome::Handled;
                        }
                    };
                    (
                        chunk,
                        inner.media_metadata,
                        inner.original_content_type,
                        inner.rich_extras,
                        inner.data_purpose,
                    )
                } else {
                    if !self.accept_plaintext_content(&sender) {
                        warn!(
                            message_id = %message.id,
                            sender = %sender,
                            "Rejecting unencrypted media chunk (encryption policy)"
                        );
                        self.warn_plaintext_receive_rejected(
                            &sender,
                            "Inbound plaintext media chunk rejected by encryption policy",
                        );
                        return ChunkOutcome::Rejected;
                    }
                    let chunk = match FileChunk::from_bytes(binary) {
                        Ok(c) => c,
                        Err(e) => {
                            warn!(
                                message_id = %message.id,
                                error = %e,
                                "Failed to deserialize binary file chunk, dropping"
                            );
                            return ChunkOutcome::Handled;
                        }
                    };
                    let (meta, oct) = Self::wire_media_metadata(message);
                    (chunk, meta, oct, None, None)
                }
            } else {
                if !self.accept_plaintext_content(&sender) {
                    warn!(
                        message_id = %message.id,
                        sender = %sender,
                        "Rejecting unencrypted media chunk (encryption policy)"
                    );
                    self.warn_plaintext_receive_rejected(
                        &sender,
                        "Inbound plaintext media chunk rejected by encryption policy",
                    );
                    return ChunkOutcome::Rejected;
                }
                let chunk = match FileChunk::from_json(&message.content) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(
                            message_id = %message.id,
                            error = %e,
                            "Failed to deserialize file chunk, dropping"
                        );
                        return ChunkOutcome::Handled;
                    }
                };
                let (meta, oct) = Self::wire_media_metadata(message);
                (chunk, meta, oct, None, None)
            };

        let file_id = chunk.file_id.clone();
        let file_name = chunk.file_name.clone();
        let file_size = chunk.file_size;
        let is_first_chunk = chunk.chunk_index == 0;

        // Refuse a blob nobody asked for at the door, not at the end.
        //
        // The check also runs on completion, which is where the hash is
        // verified, but by then the whole transfer has been buffered,
        // reassembled and checksummed. That is the storage and the battery
        // this rule exists to protect: an authenticated peer could otherwise
        // open as many transfers as the resource caps allow, hold them for
        // the stale timeout, and pay one frame per transfer to do it.
        //
        // Chunk 0 carries the purpose, so this is where the question can
        // first be asked. Answered with an ACK rather than silence: the
        // sender is wrong rather than unlucky, and a retry would be refused
        // exactly the same way.
        if is_first_chunk && !self.admits_data_media_transfer(&sender, data_purpose.as_ref()) {
            // Refused, not merely skipped. Chunks need not arrive in order,
            // so any that landed before this one already opened an assembly,
            // and any behind it would open another. The refusal has to
            // outlive the chunk that prompted it, which is what the
            // tombstone is for.
            self.file_transfer_manager.refuse_transfer(&file_id);
            self.pending_media_metadata.remove(&file_id);
            return ChunkOutcome::Handled;
        }

        // Metadata is recorded only for chunks the manager accepts — a
        // rejected chunk must leave no state behind (SEC-H2).
        match self.file_transfer_manager.process_chunk(&sender, chunk) {
            Ok(progress) => {
                if is_first_chunk {
                    self.pending_media_metadata.insert(
                        file_id.clone(),
                        PendingMediaMetadataEntry {
                            content_type: original_content_type.unwrap_or(ContentType::File),
                            media_metadata,
                            last_updated_at: Instant::now(),
                            sender: sender.clone(),
                            rich_extras,
                            timestamp_ms: message.timestamp.as_millis(),
                            data_purpose,
                        },
                    );
                } else if let Some(entry) = self.pending_media_metadata.get_mut(&file_id) {
                    entry.last_updated_at = Instant::now();
                }
                // A data-purposed transfer is invisible to the application
                // for its whole life, not merely at the end. Reporting
                // progress on it would put a download nobody started in
                // front of a person, counting up to a file that never
                // appears.
                //
                // Positive knowledge, not absence of it: the purpose rides
                // chunk 0, so a chunk arriving before it leaves this device
                // unable to say what the transfer is. Reading "no entry yet"
                // as "not data-purposed" would emit exactly the phantom
                // progress this suppression exists to prevent, on any
                // transport that delivers chunk 1 first.
                //
                // The cost is that an ordinary transfer whose chunk 0 is
                // delayed loses the progress events until it lands. That is
                // cheap: progress is advisory and each event supersedes the
                // last, so the app sees the count resume rather than a gap.
                let reports_progress = self
                    .pending_media_metadata
                    .get(&file_id)
                    .is_some_and(|entry| entry.data_purpose.is_none());
                if reports_progress {
                    if let Ok(state) = lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::file_progress(
                            file_id.clone(),
                            progress.chunks_completed,
                            progress.total_chunks,
                        ));
                    }
                }
            }
            Err(rejection) if rejection.is_resource_exhaustion() => {
                // A well-formed transfer was dropped by a receiver-side
                // resource limit. The chunk was already ACKed and will not
                // be retransmitted, so the transfer is unrecoverable —
                // surface that to the application instead of going silent.
                let dropped = self.pending_media_metadata.remove(&file_id);
                // Positive knowledge, the same rule the progress event
                // follows, reached by a different road. On chunk 0 the
                // purpose is in hand right here and the stored entry is not:
                // an entry is written only once the manager accepts a chunk,
                // which is exactly what did not happen. Off chunk 0 the
                // stored entry is the only knowledge there is.
                let identified = is_first_chunk || dropped.is_some();
                let purpose = if is_first_chunk { data_purpose } else { None }
                    .or_else(|| dropped.and_then(|entry| entry.data_purpose));
                if let Some(purpose) = purpose {
                    self.report_data_media_transfer_failure(&sender, &purpose, rejection.as_str());
                    return ChunkOutcome::Handled;
                }
                if !identified {
                    // A transfer whose chunk 0 has not landed cannot be named
                    // as anything, so it must not be failed in front of a
                    // person: it may be the document layer's. The app has
                    // heard nothing about it either (progress is withheld by
                    // the same rule), so there is no report to correct.
                    debug!(
                        file_id = %file_id,
                        "Dropping a transfer whose purpose is not yet known; no failure reported"
                    );
                    return ChunkOutcome::Handled;
                }
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::file_receive_failed(
                        file_id,
                        file_name,
                        sender,
                        rejection.as_str().to_string(),
                    ));
                }
                return ChunkOutcome::Handled;
            }
            // Malformed or mismatched chunks are logged by the manager; the
            // assembly they targeted (if any) stays intact. Chunks of an
            // already-failed transfer land here too (`previously_failed`) —
            // its FileReceiveFailed event already fired exactly once.
            Err(_) => return ChunkOutcome::Handled,
        }

        if self.file_transfer_manager.is_complete(&file_id) {
            let Some(file_data) = self.file_transfer_manager.finalize_file(&file_id) else {
                warn!(
                    file_id = %file_id,
                    "File transfer marked complete but reassembly or integrity checks failed"
                );
                let dropped = self.pending_media_metadata.remove(&file_id);
                // Completion implies chunk 0 was accepted, so this is
                // normally `true`. Checked anyway, and for the same reason as
                // the arm above: the alternative is naming a transfer this
                // device cannot identify as a file the app never saw begin.
                let identified = dropped.is_some();
                if let Some(purpose) = dropped.and_then(|entry| entry.data_purpose) {
                    self.report_data_media_transfer_failure(
                        &sender,
                        &purpose,
                        "integrity_check_failed",
                    );
                    return ChunkOutcome::Handled;
                }
                if !identified {
                    debug!(
                        file_id = %file_id,
                        "Dropping a transfer whose purpose is not yet known; no failure reported"
                    );
                    return ChunkOutcome::Handled;
                }
                if let Ok(state) = lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::file_receive_failed(
                        file_id,
                        file_name,
                        sender,
                        "integrity_check_failed".to_string(),
                    ));
                }
                return ChunkOutcome::Handled;
            };
            let metadata_entry = self.pending_media_metadata.remove(&file_id);
            // The one branch that makes a data-purposed transfer worth
            // having: the bytes go to the document layer and the person is
            // told nothing, because nothing happened that concerns them.
            if let Some(purpose) = metadata_entry
                .as_ref()
                .and_then(|entry| entry.data_purpose.clone())
            {
                self.route_data_media_payload(&sender, &purpose, file_data);
                return ChunkOutcome::Handled;
            }
            let (content_type, media_metadata, rich_extras, timestamp_ms) = metadata_entry
                .map(|entry| {
                    (
                        entry.content_type,
                        entry.media_metadata,
                        entry.rich_extras,
                        Some(entry.timestamp_ms),
                    )
                })
                .unwrap_or((ContentType::File, None, None, None));
            let rich_extras = rich_extras.unwrap_or_default();

            if let Ok(state) = lock_shared_state(&self.shared_state) {
                state.emit_event(Event::file_received(
                    file_id,
                    file_name,
                    file_size,
                    sender,
                    content_type,
                    media_metadata,
                    file_data,
                    timestamp_ms,
                    rich_extras.caption,
                    rich_extras.reply_to_msg,
                    rich_extras.reply_context.as_ref(),
                    rich_extras.forward_info.as_ref(),
                ));
            }
        }

        ChunkOutcome::Handled
    }

    /// Whether a data-purposed transfer may be admitted at all.
    ///
    /// Only the attachment case is answerable by the fetch record: an
    /// attachment is bytes this device asked a named peer for, so the request
    /// either exists or it does not. A snapshot is unsolicited by design (it
    /// answers a version exchange rather than a fetch) and is bounded instead
    /// by the size check and the containment on the import path.
    ///
    /// The runtime kill switch is checked ahead of both, and what it saves is
    /// the buffering rather than the import. A device with the layer off was
    /// never going to write anything: `require_data_storage` fails closed
    /// with `DataDisabled` further down. But a transfer admitted at chunk 0
    /// is reassembled in full before it reaches that seam, so without this
    /// gate a device that opted out still holds a peer's whole blob in memory
    /// on its way to a refusal that was certain from the start.
    ///
    /// A conforming peer never sends one, because a build with the layer off
    /// never advertises the capability that permits it. This is the arm for
    /// the peer whose knowledge of us is stale or wrong.
    fn admits_data_media_transfer(
        &self,
        sender: &str,
        purpose: Option<&crate::media_envelope::DataPurpose>,
    ) -> bool {
        let Some(purpose) = purpose else {
            return true;
        };
        #[cfg(feature = "data")]
        {
            if !self.config.data.enabled {
                warn!(
                    peer = %sender,
                    "Refusing a document-layer transfer: the data layer is off"
                );
                return false;
            }
            if let crate::media_envelope::DataPurpose::Attachment { hash } = purpose {
                if !self.awaiting_attachment(sender, hash) {
                    warn!(
                        peer = %sender,
                        "Refusing attachment bytes for a fetch this device never made"
                    );
                    return false;
                }
            }
            true
        }
        #[cfg(not(feature = "data"))]
        {
            let _ = (sender, purpose);
            // Nothing here can use these bytes, so nothing here should buffer
            // them either.
            false
        }
    }

    /// Hand a completed data-purposed transfer to the document layer.
    ///
    /// Split behind a feature gate because the layer is optional and the
    /// bytes are not: a build compiled without it still has to recognise
    /// such a transfer, since recognising it is what keeps a document
    /// snapshot from being handed to a person as a downloaded file. It has
    /// nowhere to put the bytes, so it drops them. A conforming peer never
    /// sends one, because a build without the layer never advertises the
    /// capability that permits it.
    pub(super) fn route_data_media_payload(
        &mut self,
        sender: &str,
        purpose: &crate::media_envelope::DataPurpose,
        bytes: Vec<u8>,
    ) {
        #[cfg(feature = "data")]
        {
            self.accept_data_media_payload(sender, purpose, bytes);
        }
        #[cfg(not(feature = "data"))]
        {
            let _ = bytes;
            warn!(
                peer = %sender,
                ?purpose,
                "Dropping a document-layer transfer: this build has no data layer"
            );
        }
    }

    /// Report a failed data-purposed transfer, where there is anything to
    /// report it to. See [`Self::route_data_media_payload`] for the gate.
    pub(super) fn report_data_media_transfer_failure(
        &mut self,
        sender: &str,
        purpose: &crate::media_envelope::DataPurpose,
        reason: &str,
    ) {
        #[cfg(feature = "data")]
        {
            self.report_data_media_failure(sender, purpose, reason);
        }
        #[cfg(not(feature = "data"))]
        {
            debug!(
                peer = %sender,
                ?purpose,
                reason,
                "A document-layer transfer failed; this build has no data layer"
            );
        }
    }

    /// Extracts the chunk-0 metadata a legacy (unencrypted) chunk carries on
    /// the wire `Message`.
    fn wire_media_metadata(message: &Message) -> (Option<MediaMetadata>, Option<ContentType>) {
        use crate::constants::ORIGINAL_CONTENT_TYPE_KEY;
        let original_ct = message
            .metadata
            .get(ORIGINAL_CONTENT_TYPE_KEY)
            .map(|s| ContentType::parse(s));
        (message.media_metadata.clone(), original_ct)
    }

    /// Policy gate for inbound plaintext content — text messages and legacy
    /// (unencrypted) media chunks alike: rejected when this node requires
    /// encryption, and rejected once the sender is known to run MLS — an
    /// encryption-capable peer sending plaintext is a downgrade/forgery attempt
    /// (plaintext carries no sender authentication, so anyone could inject it
    /// under a contact's name).
    ///
    /// # Capability, not confirmation
    ///
    /// This asks whether the peer *can* encrypt, not whether a session is
    /// confirmed. The distinction is the whole gate: a sender only ever emits
    /// plaintext when its own `should_auto_encrypt()` is false, which precludes
    /// it having established a session with us — while the session is merely
    /// pending it *queues* (`prepare_outbound_content`) rather than downgrading.
    /// So no honest peer sends cleartext while we know it speaks MLS, and
    /// asking only about *confirmed* sessions left a hole: an attacker with
    /// app-container write access deletes the `session_states` record, restore
    /// re-bootstraps it as `Pending` (which it must), the peer silently drops
    /// out of `confirmed_sessions`, and this gate re-opens. See
    /// [`OfflineProtocol::encryption_capable_peers`] for why that signal now
    /// comes from the credential store instead.
    ///
    /// The capability check is also *cheaper* than the confirmation one it
    /// precedes: it is an in-memory set lookup, where `is_session_confirmed`
    /// falls through to a protocol-state read plus a sealed-record open for any
    /// sender not already in `confirmed_sessions` — a path driven by the
    /// attacker-controlled `message.sender`. Ordering the cheap check first
    /// makes a flood of forged sender ids cost less than it used to.
    ///
    /// The group path has gated on existence rather than confirmation since it
    /// was written (`has_mls_group_state`); this brings the 1:1 path into line.
    fn accept_plaintext_content(&mut self, sender: &str) -> bool {
        if self.config.encryption.require_encryption {
            return false;
        }
        // `should_auto_encrypt()` guards both checks: a node with encryption
        // disabled or MLS uninitialized has no standing to call anything a
        // downgrade, and removing this guard would reject legitimate plaintext
        // on every plaintext-only deployment.
        if !self.should_auto_encrypt() {
            return true;
        }
        if self.is_encryption_capable(sender) {
            return false;
        }
        // Fail closed: if the confirmation lookup errors (storage failure),
        // treat the session as confirmed and reject — accepting plaintext on
        // error would let a storage fault disable the downgrade gate.
        if self.is_session_confirmed(sender).unwrap_or(true) {
            return false;
        }
        true
    }

    /// Decrypts an encrypted media chunk envelope. Four failure dispositions,
    /// mirroring the text path in [`Self::handle_encrypted_message`]:
    ///
    /// - **Session not ready**: the whole message is queued for delayed
    ///   decryption and [`MediaChunkDecrypt::Deferred`] is returned.
    /// - **Recoverable** (epoch desync, or a crypto/transport failure while
    ///   `crypto_recovery_enabled`): [`MediaChunkDecrypt::Deferred`] *without*
    ///   queueing — the ciphertext is dead either way, so recovery is the
    ///   sender's resend, driven by the withheld ACK.
    /// - **Security rejection** (the envelope names another pair's session
    ///   slot, or the MLS credential authenticates a different sender than the
    ///   wire envelope claims): [`MediaChunkDecrypt::SecurityRejected`], which
    ///   the caller answers with silence. Deliberately *not* gated on
    ///   `crypto_recovery_enabled` — this is about what the receiver reveals,
    ///   not about recovery, and the text path's equivalent is unconditional.
    /// - **Terminal** (a permanent refusal, an empty plaintext, or any crypto
    ///   failure with recovery switched off): decryption telemetry plus
    ///   [`MediaChunkDecrypt::Dropped`], which the caller still ACKs.
    ///
    /// On `Deferred` or `SecurityRejected` the caller must skip the ACK and
    /// unmark the id — for the former so the sender keeps retrying, for the
    /// latter so a replay re-enters this gate rather than hitting the duplicate
    /// re-ACK path. A successful decrypt doubles as a session confirmation
    /// signal, exactly like text decrypts.
    fn decrypt_media_chunk(
        &mut self,
        sender: &str,
        encrypted: &EncryptedMessage,
        message: &Message,
        arrival_transport: Option<TransportType>,
    ) -> MediaChunkDecrypt {
        let group_id = encrypted.group_id.as_str().to_string();

        // Media envelopes are only ever produced for the sender's 1:1 session,
        // whose MLS group id is deterministic. Enforce that binding before
        // decrypting: MLS authenticates group membership, not the wire sender
        // claim, so without this check any peer holding a valid session with
        // us could deliver its own ciphertext under an arbitrary
        // `message.sender` and have the file attributed to that identity.
        let Ok(expected_group) = GroupId::for_session(&self.local_id, sender) else {
            warn!(
                sender = %sender,
                "Cannot derive 1:1 session id for media chunk sender, dropping"
            );
            return MediaChunkDecrypt::Dropped;
        };
        if encrypted.group_id != expected_group {
            error!(
                sender = %sender,
                group_id = %group_id,
                expected = %expected_group,
                "SECURITY: encrypted media chunk MLS group does not match the claimed sender's session, rejecting"
            );
            self.emit_security_warning(
                sender,
                SecurityWarningCode::MediaSenderGroupMismatch,
                "Encrypted media chunk MLS group does not match the claimed sender",
            );
            // Not ACKed: the text path answers the identical condition
            // (`MlsError::SessionIdentityMismatch` →
            // `InternalMessageResult::SecurityRejected`) with silence, and an
            // ACK here would leak exactly what that silence protects — an
            // injector who gets an ACK for a media chunk but nothing for the
            // same text frame learns the target is live either way.
            return MediaChunkDecrypt::SecurityRejected;
        }

        let Some(mls) = self.mls_manager.clone() else {
            warn!(
                sender = %sender,
                "Encrypted media chunk received but MLS is not initialized, dropping"
            );
            self.emit_mls_decryption_failed(
                sender,
                Some(&group_id),
                DecryptionFailureKind::NotInitialized,
                MlsOperationContext::Receive,
            );
            return MediaChunkDecrypt::Dropped;
        };

        let decrypt_result = {
            let manager = match mls.read() {
                Ok(m) => m,
                Err(_) => {
                    error!("MLS lock poisoned while decrypting media chunk");
                    return MediaChunkDecrypt::Dropped;
                }
            };
            manager.decrypt(encrypted, sender)
        };

        match decrypt_result {
            Ok(Some(plaintext)) => {
                // A group-aware decrypt is the same confirmation signal the
                // text path uses; skip when already cache-confirmed to avoid
                // per-chunk storage I/O.
                if !self.confirmed_sessions.contains(sender) {
                    self.confirm_session_from_successful_decrypt(sender, &group_id);
                }
                MediaChunkDecrypt::Plaintext(plaintext)
            }
            Ok(None) => {
                warn!(sender = %sender, "Media chunk decryption returned empty, dropping");
                MediaChunkDecrypt::Dropped
            }
            // Intercepted BEFORE classification, mirroring the text path in
            // `handle_encrypted_message`. Both variants classify as
            // `SessionStateError::Unknown`, whose disposition must stay
            // terminal-drop-and-ACK for the refusals that genuinely belong
            // there (`CommitNotAuthorized`), so the split has to happen here
            // rather than in the classifier.
            Err(ref e) if is_media_security_rejection(e) => {
                error!(
                    sender = %sender,
                    error = %e,
                    "SECURITY: encrypted media chunk failed its sender/session identity check, rejecting"
                );
                MediaChunkDecrypt::SecurityRejected
            }
            Err(e) => {
                let classification = SessionStateError::from(&e);
                match classification {
                    SessionStateError::SessionNotReady | SessionStateError::GroupNotFound => {
                        info!(
                            sender = %sender,
                            error_code = classification.code(),
                            "Encrypted media chunk received before session ready, queuing"
                        );
                        self.emit_mls_session_missing(
                            Some(sender),
                            Some(&group_id),
                            MlsOperationContext::SessionLookup,
                            MlsErrorCategory::SessionStateMissing,
                        );
                        self.enqueue_pending_decryption_via(sender, message, arrival_transport);
                        MediaChunkDecrypt::Deferred
                    }
                    SessionStateError::SessionDesync
                        if self.config.encryption.crypto_recovery_enabled =>
                    {
                        // Epoch desync (see the text path in `handle_encrypted_message`):
                        // heal the channel with a rate-limited re-key and return
                        // Deferred so the chunk is not ACKed and its id is
                        // unmarked. We do NOT enqueue — the chunk is sealed to the
                        // dead epoch and can never decrypt after the re-key.
                        //
                        // Unlike a DM, an in-flight media chunk cannot be
                        // re-sealed for recovery: Tier 2 reseal is a deliberate
                        // no-op for media (the media outbox is not persisted and
                        // chunks are re-encoded, not replayed). Withholding the
                        // ACK here is what drives recovery: the sender's media
                        // outbox keeps retrying and, once its retry/ACK-timeout
                        // budget lapses, surfaces `MediaResendRequired`, and the
                        // app re-supplies the bytes via `send_media_with` — which
                        // re-encodes fresh chunks against the now-healed session.
                        // So media recovers via descriptor-based resend, not reseal.
                        info!(
                            sender = %sender,
                            error_code = classification.code(),
                            "Encrypted media chunk failed to decrypt due to epoch desync, re-keying"
                        );
                        self.schedule_session_rekey(sender);
                        MediaChunkDecrypt::Deferred
                    }
                    SessionStateError::TransportFailure | SessionStateError::CryptoFailure
                        if self.config.encryption.crypto_recovery_enabled =>
                    {
                        // A genuine crypto/transport failure on a chunk (AEAD
                        // failure, spent ratchet generation, malformed frame).
                        // Mirrors the text path in `handle_encrypted_message`:
                        // withhold the ACK and unmark the id rather than lying
                        // "delivered" for a chunk we dropped. Do NOT enqueue —
                        // the generation is spent, so a queued copy could never
                        // drain — and do NOT re-key, which stays desync-only.
                        //
                        // Recovery differs from a DM: chunks have no Tier 2
                        // re-seal (they are re-encoded, not replayed), so the
                        // un-ACKed chunk drives the media outbox's retry/ACK
                        // budget to lapse and surface `MediaResendRequired`,
                        // and the app re-supplies the bytes via `send_media_with`
                        // — re-encoded against a live generation. Same
                        // descriptor-based recovery as the media desync arm.
                        warn!(
                            sender = %sender,
                            error = %e,
                            error_code = classification.code(),
                            "Failed to decrypt media chunk; withholding ACK so the transfer can be resent"
                        );
                        let kind = DecryptionFailureKind::from_mls_error(&e);
                        self.emit_mls_decryption_failed(
                            sender,
                            Some(&group_id),
                            kind,
                            MlsOperationContext::Receive,
                        );
                        // Advisory, not terminal — matching a pending-queue
                        // eviction: the chunk was never ACKed, so the transfer
                        // is *stalled* pending a resend, not failed. The
                        // terminal media signal is still `FileReceiveFailed`.
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::message_decryption_failed(
                                message.id.clone(),
                                sender.to_string(),
                                Self::decryption_failure_code_from_kind(kind),
                                format!(
                                    "encrypted media chunk failed to decrypt ({}); its file transfer is stalled until the sender resends",
                                    classification.code()
                                ),
                            ));
                        }
                        MediaChunkDecrypt::Deferred
                    }
                    _ => {
                        warn!(
                            sender = %sender,
                            error = %e,
                            error_code = classification.code(),
                            "Failed to decrypt media chunk, dropping"
                        );
                        let kind = DecryptionFailureKind::from_mls_error(&e);
                        self.emit_mls_decryption_failed(
                            sender,
                            Some(&group_id),
                            kind,
                            MlsOperationContext::Receive,
                        );
                        // Permanently undecryptable (or crypto recovery is
                        // switched off): the chunk is ACKed and stays
                        // dedup-marked, so this loss is terminal for the
                        // transfer — surface it to the app, not just as MLS
                        // telemetry.
                        if let Ok(state) = lock_shared_state(&self.shared_state) {
                            state.emit_event(Event::message_decryption_failed(
                                message.id.clone(),
                                sender.to_string(),
                                Self::decryption_failure_code_from_kind(kind),
                                format!(
                                    "encrypted media chunk failed to decrypt ({}); its file transfer cannot complete",
                                    classification.code()
                                ),
                            ));
                        }
                        MediaChunkDecrypt::Dropped
                    }
                }
            }
        }
    }
}

/// Four-way outcome of [`OfflineProtocol::decrypt_media_chunk`]: the plaintext,
/// a deferral (not delivered, but recoverable by the sender's resend — either
/// queued because the session is not ready, or dropped as undecryptable with
/// the ACK withheld), a security rejection (an illegitimate frame the caller
/// must answer with silence), or a terminal drop the caller ACKs.
enum MediaChunkDecrypt {
    Plaintext(Vec<u8>),
    Deferred,
    /// The frame is not a legitimate message from the claimed sender: its
    /// envelope named another pair's session slot, or the MLS credential
    /// authenticated a different member than the wire envelope claims. Mirrors
    /// the text path's [`InternalMessageResult::SecurityRejected`] — the caller
    /// must NOT ACK (an ACK confirms to an injector that the target is online
    /// and processing their frames, which is exactly what the text path refuses
    /// to reveal) and must unmark the id.
    ///
    /// [`InternalMessageResult::SecurityRejected`]: super::InternalMessageResult::SecurityRejected
    SecurityRejected,
    Dropped,
}

/// Whether an MLS decrypt error is a *security* rejection rather than a session
/// state problem — the two classes for which the receiver must stay silent
/// instead of ACKing.
///
/// Used as the match guard that intercepts these variants *before*
/// [`SessionStateError`] classification, exactly as the text path does inline in
/// `handle_encrypted_message`. The intercept cannot be replaced by a
/// classification arm: both variants map to [`SessionStateError::Unknown`], and
/// `Unknown` must keep its terminal drop-and-ACK disposition for the classes
/// that genuinely belong there (notably `CommitNotAuthorized`, a permanent
/// refusal whose sender gains nothing from retrying).
///
/// Kept as a named predicate rather than an inline pattern so the classification
/// itself is testable: `SenderIdentityMismatch` is not reachable through any
/// SDK-built ciphertext (see
/// `test_media_security_rejection_classifies_both_identity_mismatches`).
pub(super) fn is_media_security_rejection(error: &offline_protocol_mls::MlsError) -> bool {
    matches!(
        error,
        offline_protocol_mls::MlsError::SenderIdentityMismatch { .. }
            | offline_protocol_mls::MlsError::SessionIdentityMismatch { .. }
            // The leaf-binding refusals join them for the same reason: an
            // `__MLS_ENC__` envelope naming a `group:` id reaches group decrypt
            // through this path, both classify as `Unknown`, and `Unknown` must
            // keep drop-and-ACK for `CommitNotAuthorized`.
            | offline_protocol_mls::MlsError::LeafAddressMismatch { .. }
            | offline_protocol_mls::MlsError::UnsupportedSender { .. }
    )
}
