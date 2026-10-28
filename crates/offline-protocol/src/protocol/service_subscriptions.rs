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

/// How a subscription frame left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameSent {
    /// Ephemeral, over a live direct link.
    Live,
    /// Queued on the reliable ladder.
    Queued,
}

/// What [`OfflineProtocol::publish_service_update`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublishReport {
    /// Active subscriptions on the topic.
    pub subscribers: usize,
    /// Sent once over a live direct link (`latest` only).
    pub sent_live: usize,
    /// Queued on the reliable ladder: every `log` update, and a `latest`
    /// update to a subscriber with no live link, superseding the one it
    /// replaces.
    pub queued: usize,
    /// Held back by the update interval; the newest value goes on the next
    /// tick after it.
    pub coalesced: usize,
    /// Not sent because it could not be sealed (no confirmed session). The
    /// subscriber's next renewal catches it up.
    pub failed: usize,
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

    /// Records that `peer` sealed a subscription frame to this device. Bounded
    /// like [`Self::record_svc_sub_advertisement`]: a subset of the capped
    /// encryption-capable set, and refused rather than evicted when full, so
    /// that a stream of fresh identities neither grows it without bound nor
    /// pushes out a peer already proved.
    fn record_svc_sub_proved(&mut self, peer: &str) {
        if !self.is_encryption_capable(peer) {
            return;
        }
        let proved = &mut self.service_subscriptions.proved;
        if !proved.contains(peer) && proved.len() >= MAX_ENCRYPTION_CAPABLE_PEERS {
            return;
        }
        proved.insert(peer.to_string());
    }

    /// Sends every frame in `output` and emits every event, in order. A frame
    /// that cannot be sealed is not sent; the state machines recover it
    /// (the consumer's renewal, the provider's next catch-up).
    pub(crate) fn apply_subscription_output(&mut self, output: SubscriptionOutput) {
        self.send_subscription_frames(output.frames);
        self.emit_subscription_events(output.events);
    }

    /// Sends `frames` in order and returns how each left, or why it did not.
    fn send_subscription_frames(
        &mut self,
        frames: Vec<SubscriptionFrame>,
    ) -> Vec<Result<FrameSent>> {
        frames
            .into_iter()
            .map(|frame| {
                let sent = self.send_subscription_frame(frame);
                if let Err(err) = &sent {
                    debug!(error = %err, "Subscription frame not sent");
                }
                sent
            })
            .collect()
    }

    fn emit_subscription_events(&self, events: Vec<offline_protocol_services::ServiceEvent>) {
        for event in events {
            self.emit_event(Event::from(event));
        }
    }

    /// Seals and sends one subscription frame by the delivery it asks for.
    pub(crate) fn send_subscription_frame(
        &mut self,
        frame: SubscriptionFrame,
    ) -> Result<FrameSent> {
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
        let key = match delivery {
            SubscriptionDelivery::Reliable => None,
            SubscriptionDelivery::Superseding(key) => Some(key),
            SubscriptionDelivery::LiveOrSuperseding(key) => {
                if self.confirmed_sessions.contains(&recipient) {
                    match self.send_subscription_live(&recipient, &content) {
                        Ok(_) => {
                            // Newer than anything queued under the key.
                            self.withdraw_superseded(&recipient, &key);
                            return Ok(FrameSent::Live);
                        }
                        Err(err) => {
                            debug!(error = %err, "No live link for a subscription update; queueing it");
                        }
                    }
                }
                Some(key)
            }
        };
        self.send_subscription_reliable(&recipient, content, key)
            .map(|_| FrameSent::Queued)
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
        self.record_svc_sub_proved(sender);
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

// ============================================================================
// Public API
// ============================================================================

impl OfflineProtocol {
    fn ensure_subscriptions_enabled(&self) -> Result<()> {
        if self.config.service_subscriptions.enabled {
            Ok(())
        } else {
            Err(Error::InvalidArgument(
                "service subscriptions are switched off (service_subscriptions.enabled)"
                    .to_string(),
            ))
        }
    }

    /// Subscribes to `topic` of `service_id` on `provider`, and returns the
    /// subscription's id (ADR 0035).
    ///
    /// The subscription is `pending` until the provider answers, which
    /// arrives as `service_subscription_changed`; updates arrive as
    /// `service_update_received`. With no confirmed session the call starts
    /// one and the subscribe frame leaves when it confirms: every
    /// subscription frame is sealed, and none is ever sent in plaintext.
    ///
    /// Fails with `ServiceError::Unsupported` when the provider's signed key
    /// package says it does not route subscriptions.
    pub fn subscribe_service(
        &mut self,
        provider: &str,
        service_id: &str,
        topic: &str,
        mode: offline_protocol_services::SubscriptionMode,
        params: &str,
    ) -> Result<String> {
        self.subscribe_service_with(
            provider,
            service_id,
            topic,
            offline_protocol_services::SubscribeOptions {
                params: params.to_string(),
                ..offline_protocol_services::SubscribeOptions::new(mode)
            },
        )
    }

    /// [`Self::subscribe_service`] with a key filter, and the place a topic
    /// pattern is explained.
    ///
    /// `topic` may be a pattern, `prefix*` (or `*` for every topic): one
    /// subscription then follows every topic of the service in its mode whose
    /// name starts with `prefix`, up to 64, including topics created after
    /// it; each update names its topic, and each topic is caught up and
    /// recovered on its own. `options.keys`, on a `latest` subscription,
    /// limits the keys sent to those listed, each exact or a prefix ending in
    /// `*`; the provider applies it, so a key outside it never leaves the
    /// provider. A provider on a build that predates either ends the
    /// subscription with the reason `unsupported` rather than send what was
    /// not asked for.
    pub fn subscribe_service_with(
        &mut self,
        provider: &str,
        service_id: &str,
        topic: &str,
        options: offline_protocol_services::SubscribeOptions,
    ) -> Result<String> {
        self.ensure_subscriptions_enabled()?;
        Self::validate_outbound_recipient(provider)?;
        self.ensure_running()?;
        if provider == self.local_id {
            return Err(Error::InvalidArgument(
                "a device cannot subscribe to itself".to_string(),
            ));
        }
        if self.mls_manager.is_none() {
            return Err(Error::MlsNotInitialized);
        }
        if self.peer_routes_subscriptions(provider) == Some(false) {
            return Err(Error::Service(
                offline_protocol_services::ServiceError::Unsupported(
                    "the provider's key package does not advertise subscriptions".to_string(),
                ),
            ));
        }
        let (sub_id, frame) = self
            .service_subscriptions
            .consumer
            .subscribe(provider, service_id, topic, &options, Instant::now())
            .map_err(Error::Service)?;
        match self.send_subscription_frame(frame) {
            Ok(_) => self.service_subscriptions.consumer.mark_sent(&sub_id),
            // Waits for the session; `kick_service_subscriptions` sends it.
            Err(Error::SessionNotReady(_)) => {}
            Err(err) => {
                let _ = self.service_subscriptions.consumer.unsubscribe(&sub_id);
                return Err(err);
            }
        }
        Ok(sub_id)
    }

    /// Ends a subscription this device holds, telling the provider. `false`
    /// when there is no such subscription.
    pub fn unsubscribe_service(&mut self, sub_id: &str) -> Result<bool> {
        match self.service_subscriptions.consumer.unsubscribe(sub_id) {
            Some(output) => {
                self.apply_subscription_output(output);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Asks the provider for a subscription's state again: a full snapshot
    /// of a `latest` topic, or a `log` topic's checkpoint and what follows it
    /// (what follows this device's position when it has no checkpoint).
    pub fn resync_service_subscription(&mut self, sub_id: &str) -> Result<()> {
        self.ensure_subscriptions_enabled()?;
        let frame = self
            .service_subscriptions
            .consumer
            .resync(sub_id)
            .map_err(Error::Service)?;
        self.send_subscription_frame(frame).map(|_| ())
    }

    /// The subscriptions this device holds on other devices.
    pub fn service_subscriptions(&self) -> Vec<offline_protocol_services::SubscriptionInfo> {
        self.service_subscriptions.consumer.subscriptions()
    }

    /// Sets who may subscribe to `service_id`: `Restricted` (the default)
    /// asks the application about each subscriber, `AcceptAll` admits every
    /// peer with a session, and `Rules` admits listed peers and members of
    /// listed groups and asks or refuses the rest.
    pub fn set_service_subscription_policy(
        &mut self,
        service_id: &str,
        policy: SubscriptionPolicy,
    ) -> Result<()> {
        offline_protocol_core::ServiceId::new(service_id)?;
        self.service_subscriptions
            .provider
            .set_policy(service_id, policy);
        Ok(())
    }

    /// Accepts a subscription the application was asked about with
    /// `service_subscribe_received`. The subscriber is answered and caught up.
    pub fn accept_service_subscription(&mut self, sub_id: &str) -> Result<()> {
        let output = self
            .service_subscriptions
            .provider
            .accept(sub_id, Instant::now())
            .map_err(Error::Service)?;
        self.apply_subscription_output(output);
        Ok(())
    }

    /// Rejects a subscription the application was asked about.
    pub fn reject_service_subscription(&mut self, sub_id: &str, reason: &str) -> Result<()> {
        let output = self
            .service_subscriptions
            .provider
            .reject(sub_id, reason)
            .map_err(Error::Service)?;
        self.apply_subscription_output(output);
        Ok(())
    }

    /// Ends a subscription another device holds on this one, telling it.
    /// Stops the updates; the subscriber keeps its 1:1 session.
    pub fn end_service_subscription(&mut self, sub_id: &str, reason: &str) -> Result<()> {
        let output = self
            .service_subscriptions
            .provider
            .end(sub_id, reason)
            .map_err(Error::Service)?;
        self.apply_subscription_output(output);
        Ok(())
    }

    /// The subscriptions other devices hold on `service_id`, or on every
    /// service for `None`.
    pub fn service_subscribers(
        &self,
        service_id: Option<&str>,
    ) -> Vec<offline_protocol_services::SubscriberInfo> {
        self.service_subscriptions.provider.subscribers(service_id)
    }

    /// Publishes to `topic` of `service_id`, which must be registered here.
    /// With a `key` the topic is `latest` (state: one current value per
    /// key); without one it is `log` (events: every update, in order). The
    /// first publish fixes the mode, and a subscription that waited for the
    /// topic in the other mode ends with `mode_mismatch`. A subscriber never
    /// creates a topic.
    ///
    /// Each active subscriber is sent the update once over a live direct
    /// link when it has one, and otherwise through the reliable ladder,
    /// across hops; a `latest` value queued for an absent subscriber is
    /// replaced by the next one for its key rather than queued behind it.
    pub fn publish_service_update(
        &mut self,
        service_id: &str,
        topic: &str,
        key: Option<&str>,
        body: &str,
    ) -> Result<PublishReport> {
        self.ensure_subscriptions_enabled()?;
        if !self.mesh_services.has_service(service_id) {
            return Err(Error::InvalidArgument(format!(
                "register `{service_id}` before publishing to it"
            )));
        }
        let (output, outcome) = self
            .service_subscriptions
            .provider
            .publish(service_id, topic, key, body, Instant::now())
            .map_err(Error::Service)?;
        let mut report = PublishReport {
            subscribers: outcome.subscribers,
            coalesced: outcome.coalesced,
            ..PublishReport::default()
        };
        for sent in self.send_subscription_frames(output.frames) {
            match sent {
                Ok(FrameSent::Live) => report.sent_live += 1,
                Ok(FrameSent::Queued) => report.queued += 1,
                Err(_) => report.failed += 1,
            }
        }
        self.emit_subscription_events(output.events);
        Ok(report)
    }

    /// Sets a `log` topic's checkpoint: a body summarising everything
    /// published to it so far. A subscriber whose position has fallen out of
    /// the buffer, or that asks to resync, is sent this and what follows it.
    pub fn set_service_topic_checkpoint(
        &mut self,
        service_id: &str,
        topic: &str,
        body: &str,
    ) -> Result<()> {
        self.ensure_subscriptions_enabled()?;
        if !self.mesh_services.has_service(service_id) {
            return Err(Error::InvalidArgument(format!(
                "register `{service_id}` before setting its checkpoint"
            )));
        }
        let output = self
            .service_subscriptions
            .provider
            .set_checkpoint(service_id, topic, body)
            .map_err(Error::Service)?;
        self.apply_subscription_output(output);
        Ok(())
    }

    /// Starts a topic over: every key, buffered update and the checkpoint are
    /// dropped, and each subscriber is told so (`stream_reset`) before it is
    /// sent anything new.
    pub fn clear_service_topic(&mut self, service_id: &str, topic: &str) -> Result<()> {
        let output = self
            .service_subscriptions
            .provider
            .clear_topic(service_id, topic)
            .map_err(Error::Service)?;
        self.apply_subscription_output(output);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Hooks
    // ------------------------------------------------------------------------

    /// The session with `peer` confirmed: subscriptions that were waiting
    /// for it send their subscribe frame now.
    pub(crate) fn kick_service_subscriptions(&mut self, peer: &str) {
        let waiting = self
            .service_subscriptions
            .consumer
            .frames_awaiting_session(peer);
        for (sub_id, frame) in waiting {
            if self.send_subscription_frame(frame).is_ok() {
                self.service_subscriptions.consumer.mark_sent(&sub_id);
            }
        }
    }

    /// `peer` was seen again: a paused subscription on it renews at once.
    pub(crate) fn on_subscription_peer_seen(&mut self, peer: &str) {
        let output = self.service_subscriptions.consumer.on_peer_seen(peer);
        self.apply_subscription_output(output);
    }

    /// One tick: renewals, pauses, leases, approval timeouts and coalesced
    /// `latest` values.
    pub(crate) fn tick_service_subscriptions(&mut self) {
        if !self.config.service_subscriptions.enabled {
            return;
        }
        let now = Instant::now();
        let mut output = self.service_subscriptions.consumer.tick(now);
        output.extend(self.service_subscriptions.provider.tick(now));
        self.apply_subscription_output(output);
    }

    /// `service_id` was unregistered: every subscription on it ends.
    pub(crate) fn end_subscriptions_on(&mut self, service_id: &str) {
        let output = self
            .service_subscriptions
            .provider
            .service_unregistered(service_id);
        self.apply_subscription_output(output);
    }
}
