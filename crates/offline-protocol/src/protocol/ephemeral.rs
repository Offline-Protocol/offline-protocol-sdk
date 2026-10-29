//! Ephemeral sends: one attempt over a live direct link, never retried.
//!
//! The decision and its reasons are ADR 0032. The invariants:
//!
//! - **Nothing outlives the call.** An ephemeral frame carries
//!   `requires_ack = false`, so the receiver sends no acknowledgement, and the
//!   sender keeps no outbox entry, ACK registration, retry, park or mesh
//!   hand-off for it. The only state written is what encryption itself
//!   writes (the ratchet) and the debounced Lamport clock.
//! - **Only a live direct link carries one.** The relay and Reticulum accept
//!   a frame for anyone and keep it, so an update sent through them would
//!   arrive after the moment it described, possibly days later. With no
//!   direct link to the recipient the send fails at once, and nothing was
//!   encrypted, so no ratchet generation was spent on it.
//! - **Plain content only.** No reply threading, rich extras, content type or
//!   forced transport: each of those is a promise about how the message is
//!   rendered or routed later, and an ephemeral message has no later.
//! - **A group send never strands a member.** A frame some member did not
//!   get still advanced this device's sender ratchet for all of them. Those
//!   frames count against the same budget the directed replication frames
//!   use, and when it is spent the send is made reliable instead, which puts
//!   a rung every member can decrypt into each absent member's outbox.

use std::collections::HashSet;

use offline_protocol_core::{ContentType, MessageId, MessagePriority};
use offline_protocol_transport::TransportType;
use tracing::debug;

use super::{lock_shared_state, OfflineProtocol, ProtocolState, SendMessageOptions};
use crate::{Error, Result};

/// Why an ephemeral send refuses an option.
const EPHEMERAL_OPTIONS_REFUSED: &str = "an ephemeral send carries plain content only: no reply, \
     rich extras, content type or forced transport";

impl OfflineProtocol {
    /// Refuses the options an ephemeral direct message cannot honour.
    pub(super) fn check_ephemeral_options(options: &SendMessageOptions) -> Result<()> {
        let plain = options.reply_to_msg.is_none()
            && options.reply_context.is_none()
            && options.media_metadata.is_none()
            && options.forward_info.is_none()
            && options.via_transport.is_none()
            && options
                .content_type
                .is_none_or(|content_type| content_type == ContentType::Text);
        if plain {
            Ok(())
        } else {
            Err(Error::InvalidArgument(
                EPHEMERAL_OPTIONS_REFUSED.to_string(),
            ))
        }
    }

    /// Refuses the group options an ephemeral group message cannot honour.
    pub(crate) fn check_ephemeral_group_options(
        reply_to_msg: Option<&str>,
        content_type: Option<ContentType>,
        has_media: bool,
    ) -> Result<()> {
        let plain = reply_to_msg.is_none()
            && !has_media
            && content_type.is_none_or(|content_type| content_type == ContentType::Text);
        if plain {
            Ok(())
        } else {
            Err(Error::InvalidArgument(
                EPHEMERAL_OPTIONS_REFUSED.to_string(),
            ))
        }
    }

    /// Sends a direct message once, over the live direct link to
    /// `recipient`, sealed under a confirmed session. Fails with
    /// `PeerNotReachable` when no such link exists and with `SessionNotReady`
    /// when no confirmed session does; neither queues anything.
    pub(super) fn send_ephemeral_message(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
        app_id: Option<offline_protocol_core::AppId>,
    ) -> Result<MessageId> {
        // Asked before encrypting: a frame that cannot leave must not spend a
        // ratchet generation.
        let transport = self.direct_link_to(recipient)?;
        // The same fail-closed rule as every other send: plaintext only on an
        // explicit `require_encryption = false`.
        let body = if self.should_auto_encrypt() {
            self.encrypt_content_for_recipient_strict(recipient, content)?
        } else if self.config.encryption.require_encryption {
            return Err(Error::EncryptFailed(
                "MLS encryption is required (the default) but MLS is not initialized; an \
                 ephemeral send is never sent as plaintext unless require_encryption is false"
                    .to_string(),
            ));
        } else {
            self.warn_plaintext_send(recipient);
            content.to_string()
        };
        let mut message =
            self.create_message_for_app(recipient, body, Some(priority), None, app_id)?;
        message.requires_ack = false;
        let message_id = message.id.clone();
        self.deduplicator.mark_seen_local(message_id.clone());

        let previous_transport = self.transport_manager.current_transport();
        if let Err(err) = self
            .transport_manager
            .send_via_transport(&message, transport)
        {
            self.transport_manager.record_retry_failure(transport);
            return Err(err);
        }
        self.emit_transport_switch_event(previous_transport, Some(transport))?;
        self.emit_message_sent_event(&message)?;
        Ok(message_id)
    }

    /// Hands one built group frame to `member` over its live direct link,
    /// once. Signed like every group frame; never acknowledged.
    pub(crate) fn send_ephemeral_group_frame(
        &mut self,
        member: &str,
        content: String,
        priority: MessagePriority,
    ) -> Result<MessageId> {
        let transport = self.direct_link_to(member)?;
        // `create_message`, not the alerting `create_message_for_app` the
        // reliable chat path uses: the alert marker only tells the relay
        // whether to push, and this frame only ever takes a direct link.
        let mut message = self.create_message(member, content, Some(priority), None)?;
        message.requires_ack = false;
        self.sign_control_message(&mut message)?;
        let message_id = message.id.clone();
        self.deduplicator.mark_seen_local(message_id.clone());
        if let Err(err) = self
            .transport_manager
            .send_via_transport(&message, transport)
        {
            self.transport_manager.record_retry_failure(transport);
            debug!(member = %member, error = %err, "Ephemeral group frame not sent");
            return Err(err);
        }
        Ok(message_id)
    }

    /// `NotStarted` unless the engine is running.
    pub(crate) fn ensure_running(&self) -> Result<()> {
        let state = lock_shared_state(&self.shared_state)?;
        if state.state == ProtocolState::Running {
            Ok(())
        } else {
            Err(Error::NotStarted)
        }
    }

    /// The members of `group_id` still waiting for a rung an ephemeral
    /// send parked for them. Forgets every rung that has left the outbox:
    /// acknowledged, expired or failed, it no longer has to arrive first.
    pub(crate) fn members_awaiting_a_rung(&mut self, group_id: &str) -> HashSet<String> {
        let Some(rungs) = self.group_mesh.ephemeral_rungs.get_mut(group_id) else {
            return HashSet::new();
        };
        let outbox = &self.outbox;
        rungs.retain(|_, ids| {
            ids.retain(|id| outbox.contains_key(id));
            !ids.is_empty()
        });
        let held: HashSet<String> = rungs.keys().cloned().collect();
        if rungs.is_empty() {
            self.group_mesh.ephemeral_rungs.remove(group_id);
        }
        held
    }

    /// Remembers the copies of a reliable group send that wait in the outbox
    /// for members an ephemeral send could not reach (`linked` is who it
    /// could). A member it reached got the copy in order on a live link
    /// and needs no hold. A relay broadcast parks nothing here, so it
    /// records nothing.
    pub(crate) fn record_ephemeral_rungs(
        &mut self,
        group_id: &str,
        ids: &[MessageId],
        linked: &[String],
    ) {
        for id in ids {
            let Some(entry) = self.outbox.get(id) else {
                continue;
            };
            let member = entry.message.recipient.as_str().to_string();
            if linked.contains(&member) {
                continue;
            }
            self.group_mesh
                .ephemeral_rungs
                .entry(group_id.to_string())
                .or_default()
                .entry(member)
                .or_default()
                .push(id.clone());
        }
    }

    pub(super) fn direct_link_to(&self, recipient: &str) -> Result<TransportType> {
        self.transport_manager
            .mesh_transport_for(recipient)
            .ok_or_else(|| {
                Error::Transport(offline_protocol_transport::Error::PeerNotReachable(
                    format!("{recipient}: an ephemeral send needs a live direct link"),
                ))
            })
    }
}
