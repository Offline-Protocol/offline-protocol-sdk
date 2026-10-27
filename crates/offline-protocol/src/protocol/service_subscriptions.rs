//! Service subscriptions (ADR 0035): the engine's half.
//!
//! The state machines live in `offline-protocol-services` and perform no I/O.
//! This module is what they cannot be: the sealing, the delivery each frame
//! asks for, the routing of received frames, and the admission policy, which
//! needs group membership.
//!
//! The invariant this module owns is that **every subscription frame travels
//! inside the 1:1 MLS session or not at all.** Requests and responses have a
//! signed-plaintext floor (`service_seal.rs`) because that form predates
//! sealing; subscriptions have none. A send that cannot be sealed returns the
//! error, and a subscription frame that arrives in plaintext is dropped
//! before anything reads it. Copying the request floor here would put a
//! stream of live device state in the clear for every carrier.

use std::collections::{HashMap, HashSet};

use offline_protocol_core::time::{Duration, Instant};
use offline_protocol_core::{ContentType, MessageId, MessagePriority};
use offline_protocol_services::{
    is_subscription_frame, requested_service, Admission, Consumer, Provider, RulesFallback,
    SubscriptionDelivery, SubscriptionFrame, SubscriptionOutput, SubscriptionPolicy, SVC_SUBSCRIBE,
    SVC_SUBSCRIBE_REPLY, SVC_UNSUBSCRIBE, SVC_UPDATE,
};
use tracing::{debug, warn};

use super::{OfflineProtocol, OutboxReseal, MAX_ENCRYPTION_CAPABLE_PEERS};
use crate::config::ServiceSubscriptionConfig;
use crate::{Error, Event, Result};

/// The engine's subscription state: both halves, what waits superseded in
/// the outbox, and which peers route subscription frames.
pub(crate) struct ServiceSubscriptions {
    pub(crate) provider: Provider,
    pub(crate) consumer: Consumer,
    /// The queued message for each (recipient, supersede key). A newer frame
    /// under the same key withdraws it before it is queued, so at most one
    /// value per subscription and key waits for an absent peer.
    superseding: HashMap<(String, String), MessageId>,
    /// Whether each peer's latest signed key package advertised
    /// `svc_versions` entry 2. Absent: not known.
    advertised: HashMap<String, bool>,
    /// Peers that have sent this device a sealed subscription frame, which
    /// proves they route them whatever a replayed key package says.
    proved: HashSet<String>,
}

impl ServiceSubscriptions {
    pub(crate) fn new(config: &ServiceSubscriptionConfig) -> Self {
        let (provider_timing, consumer_timing) = timing(config);
        Self {
            provider: Provider::new(provider_timing.0, provider_timing.1),
            consumer: Consumer::new(consumer_timing.0, consumer_timing.1, consumer_timing.2),
            superseding: HashMap::new(),
            advertised: HashMap::new(),
            proved: HashSet::new(),
        }
    }
}

type ProviderTiming = (Duration, Duration);
type ConsumerTiming = (Duration, Duration, Duration);

fn timing(config: &ServiceSubscriptionConfig) -> (ProviderTiming, ConsumerTiming) {
    (
        (
            Duration::from_millis(config.min_update_interval_ms),
            Duration::from_millis(config.approval_timeout_ms),
        ),
        (
            Duration::from_millis(config.lease_ms),
            Duration::from_millis(config.pending_timeout_ms),
            Duration::from_millis(config.max_pause_ms),
        ),
    )
}

/// Updates travel below the control frames that open, renew and end a
/// subscription, so a burst of state never holds back the answer to a
/// subscribe.
fn priority_for(content: &str) -> MessagePriority {
    if content.starts_with(SVC_UPDATE) {
        MessagePriority::Medium
    } else {
        MessagePriority::High
    }
}

impl OfflineProtocol {
    /// Whether `peer` routes subscription frames: `Some(true)` when it proved
    /// it or its signed key package says so, `Some(false)` when its signed
    /// key package says otherwise, `None` when this device has not seen one.
    pub(crate) fn peer_routes_subscriptions(&self, peer: &str) -> Option<bool> {
        if self.service_subscriptions.proved.contains(peer) {
            return Some(true);
        }
        self.service_subscriptions.advertised.get(peer).copied()
    }

    /// Records what a signed key package from `peer` said about entry 2.
    /// Bounded like the other capability sets: a subset of the capped
    /// encryption-capable set, and refused rather than evicted when full.
    pub(crate) fn record_svc_sub_advertisement(&mut self, peer: &str, advertised: bool) {
        if !self.is_encryption_capable(peer) {
            return;
        }
        let known = &mut self.service_subscriptions.advertised;
        if !known.contains_key(peer) && known.len() >= MAX_ENCRYPTION_CAPABLE_PEERS {
            return;
        }
        known.insert(peer.to_string(), advertised);
    }

    /// Sends every frame in `output` and emits every event, in order. A frame
    /// that cannot be sealed is not sent; the state machines recover it
    /// (the consumer's renewal, the provider's next catch-up).
    pub(crate) fn apply_subscription_output(&mut self, output: SubscriptionOutput) {
        for frame in output.frames {
            if let Err(err) = self.send_subscription_frame(frame) {
                debug!(error = %err, "Subscription frame not sent");
            }
        }
        for event in output.events {
            self.emit_event(Event::from(event));
        }
    }

    /// Seals and sends one subscription frame by the delivery it asks for.
    /// Returns the message id when it was queued or sent.
    pub(crate) fn send_subscription_frame(
        &mut self,
        frame: SubscriptionFrame,
    ) -> Result<MessageId> {
        if !self.config.service_subscriptions.enabled {
            return Err(Error::InvalidArgument(
                "service subscriptions are switched off".to_string(),
            ));
        }
        if !self.should_auto_encrypt() {
            return Err(Error::EncryptFailed(
                "a subscription frame is sealed or not sent, and MLS is not available".to_string(),
            ));
        }
        let SubscriptionFrame {
            recipient,
            content,
            delivery,
        } = frame;
        match delivery {
            SubscriptionDelivery::Reliable => {
                self.send_subscription_reliable(&recipient, content, None)
            }
            SubscriptionDelivery::Superseding(key) => {
                self.send_subscription_reliable(&recipient, content, Some(key))
            }
            SubscriptionDelivery::LiveOrSuperseding(key) => {
                if self.confirmed_sessions.contains(&recipient) {
                    match self.send_subscription_live(&recipient, &content) {
                        Ok(message_id) => {
                            // Newer than anything queued under the key.
                            self.withdraw_superseded(&recipient, &key);
                            return Ok(message_id);
                        }
                        Err(err) => {
                            debug!(error = %err, "No live link for a subscription update; queueing it");
                        }
                    }
                }
                self.send_subscription_reliable(&recipient, content, Some(key))
            }
        }
    }

    /// Sealed, on the reliable ladder. With `key`, whatever is still queued
    /// to `recipient` under it is withdrawn first, once the new frame has
    /// sealed: a frame that fails to seal must not cost the one before it.
    fn send_subscription_reliable(
        &mut self,
        recipient: &str,
        content: String,
        key: Option<String>,
    ) -> Result<MessageId> {
        let priority = priority_for(&content);
        // Starts the session when there is none, and returns
        // `SessionNotReady` until it confirms. Never a plaintext fallback.
        let sealed = self.encrypt_content_for_recipient(recipient, &content, priority)?;
        if let Some(key) = &key {
            self.withdraw_superseded(recipient, key);
        }
        let reseal = OutboxReseal {
            content,
            priority,
            reply_to_msg: None,
            forwarded_from: None,
            content_type: ContentType::Text,
            media_metadata: None,
            rich: None,
            seal_mark: None,
        };
        let message_id =
            self.send_sealed_internal_message(recipient, sealed, priority, Some(reseal))?;
        if let Some(key) = key {
            self.service_subscriptions
                .superseding
                .insert((recipient.to_string(), key), message_id.clone());
        }
        Ok(message_id)
    }

    /// Sealed and ephemeral, once, over the live direct link (ADR 0032): no
    /// outbox, acknowledgement or retry. Fails before encrypting when there
    /// is no link, so no ratchet generation is spent on a frame that cannot
    /// leave.
    fn send_subscription_live(&mut self, recipient: &str, content: &str) -> Result<MessageId> {
        let transport = self.direct_link_to(recipient)?;
        let sealed = self.encrypt_content_for_recipient_strict(recipient, content)?;
        let mut message =
            self.create_message(recipient, sealed, Some(priority_for(content)), None)?;
        message.requires_ack = false;
        let message_id = message.id.clone();
        self.deduplicator.mark_seen_local(message_id.clone());
        if let Err(err) = self
            .transport_manager
            .send_via_transport(&message, transport)
        {
            self.transport_manager.record_retry_failure(transport);
            return Err(err);
        }
        Ok(message_id)
    }

    /// Withdraws the frame queued to `recipient` under `key`, if it is still
    /// waiting. It was superseded, not failed, so no failure is recorded.
    fn withdraw_superseded(&mut self, recipient: &str, key: &str) {
        if let Some(previous) = self
            .service_subscriptions
            .superseding
            .remove(&(recipient.to_string(), key.to_string()))
        {
            self.forget_outbound_message(&previous);
        }
    }

    /// Forgets superseding entries whose message has left the outbox:
    /// delivered, expired or given up, there is nothing left to withdraw.
    pub(crate) fn prune_superseded(&mut self) {
        let outbox = &self.outbox;
        self.service_subscriptions
            .superseding
            .retain(|_, message_id| outbox.contains_key(message_id));
    }

    /// Routes a subscription frame that decrypted in the 1:1 session with
    /// `sender`, who is therefore its authenticated sender.
    pub(crate) fn handle_subscription_frame(&mut self, sender: &str, content: &str) {
        if !self.config.service_subscriptions.enabled {
            debug!(sender = %sender, "Service subscriptions are off; dropping a subscription frame");
            return;
        }
        self.service_subscriptions.proved.insert(sender.to_string());
        let now = Instant::now();
        let output = if content.starts_with(SVC_SUBSCRIBE) {
            let Some(service_id) = requested_service(content) else {
                return;
            };
            let registered = self.mesh_services.has_service(&service_id);
            let admission = self.admission_for(&service_id, sender);
            self.service_subscriptions.provider.handle_subscribe(
                content,
                sender,
                now,
                &|_| registered,
                &|_| admission,
            )
        } else if content.starts_with(SVC_SUBSCRIBE_REPLY) {
            self.service_subscriptions
                .consumer
                .handle_reply(content, sender, now)
        } else if content.starts_with(SVC_UPDATE) {
            self.service_subscriptions
                .consumer
                .handle_update(content, sender)
        } else if content.starts_with(SVC_UNSUBSCRIBE) {
            match self
                .service_subscriptions
                .provider
                .handle_unsubscribe(content, sender)
            {
                Some(output) => output,
                None => self
                    .service_subscriptions
                    .consumer
                    .handle_unsubscribe(content, sender)
                    .unwrap_or_default(),
            }
        } else {
            return;
        };
        self.apply_subscription_output(output);
    }

    /// Drops a subscription frame that arrived as plaintext. Never routed:
    /// any carrier could have written it.
    pub(crate) fn drop_plaintext_subscription_frame(&self, sender: &str, content: &str) -> bool {
        if !is_subscription_frame(content) {
            return false;
        }
        warn!(sender = %sender, "Dropping a subscription frame that arrived in plaintext");
        true
    }

    /// What `service_id`'s policy says about `subscriber`.
    pub(crate) fn admission_for(&self, service_id: &str, subscriber: &str) -> Admission {
        match self.service_subscriptions.provider.policy(service_id) {
            SubscriptionPolicy::Restricted => Admission::Ask,
            SubscriptionPolicy::AcceptAll => Admission::Accept,
            SubscriptionPolicy::Rules {
                allow_peers,
                allow_groups,
                fallback,
            } => {
                let allowed = allow_peers.iter().any(|peer| peer == subscriber)
                    || allow_groups.iter().any(|group_id| {
                        self.get_group_roles(group_id)
                            .is_ok_and(|members| members.contains_key(subscriber))
                    });
                match (allowed, fallback) {
                    (true, _) => Admission::Accept,
                    (false, RulesFallback::Ask) => Admission::Ask,
                    (false, RulesFallback::Reject) => Admission::Reject,
                }
            }
        }
    }
}
