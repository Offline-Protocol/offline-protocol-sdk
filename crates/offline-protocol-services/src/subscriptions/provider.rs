use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use offline_protocol_core::time::{Duration, Instant};
use tracing::{debug, warn};

use super::{
    changed, clamp_reason, decode, digest, encode, is_topic_pattern, key_matches, topic_matches,
    validate_key, validate_key_filter, validate_subscription_topic, validate_topic, Admission,
    SubscriptionDelivery, SubscriptionFrame, SubscriptionOutput, SubscriptionPolicy,
    SubscriptionRole, SubscriptionState,
};
use crate::error::ServiceError;
use crate::events::ServiceEvent;
use crate::payloads::{
    ResumePosition, SubscribePayload, SubscribeReplyPayload, SubscribeStatus, SubscriptionMode,
    TopicReply, UnsubscribePayload, UpdatePayload, MAX_LATEST_BYTES, MAX_LATEST_KEYS,
    MAX_LOG_BYTES, MAX_LOG_ENTRIES, MAX_PATTERN_TOPICS, MAX_PROVIDER_SUBSCRIPTIONS,
    MAX_SERVICE_BODY_SIZE, MAX_SUBSCRIPTIONS_PER_PEER, MAX_SUBSCRIPTION_LEASE_MS,
    MAX_SUBSCRIPTION_PARAMS_SIZE, MIN_SUBSCRIPTION_LEASE_MS, SUBSCRIPTION_WIRE_VERSION,
    SVC_SUBSCRIBE, SVC_SUBSCRIBE_REPLY, SVC_UNSUBSCRIBE, SVC_UPDATE,
};

/// Longest subscription id accepted from a consumer. Ours are UUIDs.
const MAX_SUB_ID_LEN: usize = 64;

/// What one publish did, for the engine's report to the caller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublishOutcome {
    /// Active subscriptions on the topic.
    pub subscribers: usize,
    /// Of those, how many were held back by the update interval; the newest
    /// value goes on the next tick after it.
    pub coalesced: usize,
}

/// One subscription held on this device, for the application to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriberInfo {
    /// The subscription's identifier.
    pub sub_id: String,
    /// The subscriber.
    pub subscriber: String,
    /// The subscribed service.
    pub service_id: String,
    /// The subscribed topic, or a pattern (`prefix*`).
    pub topic: String,
    /// The topic's mode.
    pub mode: SubscriptionMode,
    /// `Pending` while the application decides, `Active` after.
    pub state: SubscriptionState,
    /// The subscriber's parameters.
    pub params: String,
    /// The `latest` keys the subscriber asked for; empty for every key.
    pub keys: Vec<String>,
}

/// One topic's state. Kept whether or not anyone is subscribed, so the first
/// subscriber gets state without the application publishing it again.
struct Topic {
    mode: SubscriptionMode,
    /// Minted per topic per process; a consumer that sees a new one knows the
    /// state it holds belongs to a previous life of this topic.
    stream: String,
    /// The seq the next publish takes. Seqs start at 1, so 0 means "nothing".
    next_seq: u64,
    /// `latest`: the current value of every key, with the seq it took it at.
    latest: BTreeMap<String, (u64, String)>,
    latest_bytes: usize,
    /// `log`: recent updates, oldest first.
    log: VecDeque<(u64, String)>,
    log_bytes: usize,
    /// `log`: a body summarising everything up to its seq.
    checkpoint: Option<(u64, String)>,
}

impl Topic {
    fn new(mode: SubscriptionMode) -> Self {
        Self {
            mode,
            stream: uuid::Uuid::new_v4().simple().to_string(),
            next_seq: 1,
            latest: BTreeMap::new(),
            latest_bytes: 0,
            log: VecDeque::new(),
            log_bytes: 0,
            checkpoint: None,
        }
    }

    fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }

    /// The digest of the keys `filter` passes: what a subscriber with that
    /// filter holds when it is caught up.
    fn digest(&self, filter: &[String]) -> u64 {
        digest(
            self.latest
                .iter()
                .filter(|(key, _)| key_matches(filter, key))
                .map(|(key, (seq, _))| (key.as_str(), *seq)),
        )
    }

    /// Whether the buffer holds every update after `seq`.
    fn buffer_covers_after(&self, seq: u64) -> bool {
        if seq >= self.last_seq() {
            return true;
        }
        self.log.front().is_some_and(|(first, _)| *first <= seq + 1)
    }
}

/// One subscription another device holds on this one.
struct Sub {
    subscriber: String,
    service_id: String,
    topic: String,
    mode: SubscriptionMode,
    params: String,
    /// `latest` keys to send; empty for every key.
    keys: Vec<String>,
    /// A pattern subscription: the topics it follows, at most
    /// [`MAX_PATTERN_TOPICS`]. A topic joins when it first exists in the
    /// subscription's mode, and stays.
    followed: BTreeSet<String>,
    /// `false` while the application decides.
    admitted: bool,
    asked_at: Instant,
    lease: Duration,
    lease_until: Instant,
    /// Where the subscriber said it stood, kept while the application decides
    /// so that its acceptance can catch the subscriber up from there.
    pending_resume: Option<ResumePosition>,
    pending_resync: bool,
    /// When the last `latest` update went to this subscriber.
    last_sent: Option<Instant>,
    /// `latest` (topic, key) pairs published faster than the update
    /// interval, waiting for the next tick.
    dirty: BTreeSet<(String, String)>,
    /// A pattern subscription passed over a matching topic for want of room.
    pattern_full: bool,
    /// The application has been told so; once per subscription.
    pattern_full_reported: bool,
}

impl Sub {
    fn is_pattern(&self) -> bool {
        is_topic_pattern(&self.topic)
    }

    /// Whether an update to `topic` in `mode` goes to this subscription. A
    /// pattern subscription starts following a matching topic here when it
    /// has room.
    fn follows(&mut self, topic: &str, mode: SubscriptionMode) -> bool {
        if !self.is_pattern() {
            return self.topic == topic;
        }
        if mode != self.mode || !topic_matches(&self.topic, topic) {
            return false;
        }
        if self.followed.contains(topic) {
            return true;
        }
        if self.followed.len() >= MAX_PATTERN_TOPICS {
            debug!(
                topic,
                "A pattern subscription follows as many topics as it may"
            );
            self.pattern_full = true;
            return false;
        }
        self.followed.insert(topic.to_string());
        true
    }

    /// Adds every topic that exists and matches, in name order, while there
    /// is room.
    fn follow_existing(&mut self, topics: &HashMap<(String, String), Topic>) {
        if !self.is_pattern() {
            return;
        }
        let mut names: Vec<&String> = topics
            .iter()
            .filter(|((service, _), topic)| *service == self.service_id && topic.mode == self.mode)
            .map(|((_, name), _)| name)
            .collect();
        names.sort();
        for name in names {
            self.follows(name, self.mode);
        }
    }

    /// The topic an update frame names: only a pattern subscription needs
    /// telling which of its topics the update belongs to.
    fn label<'a>(&self, topic: &'a str) -> Option<&'a str> {
        self.is_pattern().then_some(topic)
    }
}

/// The provider half of subscriptions: who is subscribed to this device's
/// services, and the per-topic state that catches them up.
pub struct Provider {
    topics: HashMap<(String, String), Topic>,
    subs: HashMap<String, Sub>,
    policies: HashMap<String, SubscriptionPolicy>,
    min_update_interval: Duration,
    approval_timeout: Duration,
}

impl Provider {
    /// A provider that sends a `latest` subscriber at most one update per
    /// `min_update_interval`, and rejects a subscription the application has
    /// not answered within `approval_timeout`.
    pub fn new(min_update_interval: Duration, approval_timeout: Duration) -> Self {
        Self {
            topics: HashMap::new(),
            subs: HashMap::new(),
            policies: HashMap::new(),
            min_update_interval,
            approval_timeout,
        }
    }

    /// Changes the update interval and approval timeout for what follows.
    pub fn set_timing(&mut self, min_update_interval: Duration, approval_timeout: Duration) {
        self.min_update_interval = min_update_interval;
        self.approval_timeout = approval_timeout;
    }

    /// Sets who may subscribe to `service_id`. A subscription already held is
    /// re-examined at its next renewal, at most half a lease away, and ends
    /// with `policy` if the policy now refuses its subscriber.
    pub fn set_policy(&mut self, service_id: &str, policy: SubscriptionPolicy) {
        self.policies.insert(service_id.to_string(), policy);
    }

    /// The policy for `service_id`: [`SubscriptionPolicy::Restricted`] unless
    /// one was set.
    pub fn policy(&self, service_id: &str) -> SubscriptionPolicy {
        self.policies.get(service_id).cloned().unwrap_or_default()
    }

    /// Whether `sub_id` is a subscription held on this device.
    pub fn holds(&self, sub_id: &str) -> bool {
        self.subs.contains_key(sub_id)
    }

    /// The subscriptions held on `service_id`, or on every service for `None`.
    pub fn subscribers(&self, service_id: Option<&str>) -> Vec<SubscriberInfo> {
        let mut list: Vec<SubscriberInfo> = self
            .subs
            .iter()
            .filter(|(_, sub)| service_id.is_none_or(|id| sub.service_id == id))
            .map(|(sub_id, sub)| SubscriberInfo {
                sub_id: sub_id.clone(),
                subscriber: sub.subscriber.clone(),
                service_id: sub.service_id.clone(),
                topic: sub.topic.clone(),
                mode: sub.mode,
                state: if sub.admitted {
                    SubscriptionState::Active
                } else {
                    SubscriptionState::Pending
                },
                params: sub.params.clone(),
                keys: sub.keys.clone(),
            })
            .collect();
        list.sort_by(|a, b| a.sub_id.cmp(&b.sub_id));
        list
    }

    // ------------------------------------------------------------------
    // Frames from subscribers
    // ------------------------------------------------------------------

    /// Handles a `__SVC_SUB__` from `sender`, the MLS-authenticated
    /// subscriber. `registered` says whether a service is registered here,
    /// and `admit` applies the service's policy to `sender`; the engine
    /// supplies both because it holds the registry and group membership.
    pub fn handle_subscribe(
        &mut self,
        content: &str,
        sender: &str,
        now: Instant,
        registered: &dyn Fn(&str) -> bool,
        admit: &dyn Fn(&str) -> Admission,
    ) -> SubscriptionOutput {
        let Some(payload) = decode::<SubscribePayload>(content, SVC_SUBSCRIBE) else {
            return SubscriptionOutput::default();
        };
        let mut out = SubscriptionOutput::default();
        if payload.sub_id.is_empty() || payload.sub_id.len() > MAX_SUB_ID_LEN {
            warn!(sender = %sender, "Dropping a subscribe frame with an unusable id");
            return out;
        }
        let refuse = |status: SubscribeStatus, reason: &str| SubscriptionFrame {
            recipient: sender.to_string(),
            content: reply_content(&payload.sub_id, status, Some(reason), 0, "", 0, false),
            delivery: SubscriptionDelivery::Superseding(reply_key(&payload.sub_id)),
        };
        if validate_subscription_topic(&payload.topic).is_err()
            || validate_key_filter(&payload.keys, payload.mode).is_err()
            || payload.params.len() > MAX_SUBSCRIPTION_PARAMS_SIZE
        {
            out.frames
                .push(refuse(SubscribeStatus::Rejected, "invalid_subscription"));
            return out;
        }
        if !registered(&payload.service_id) {
            out.frames
                .push(refuse(SubscribeStatus::NotFound, "not_found"));
            return out;
        }
        let lease = clamp_lease(payload.lease_ms);

        // A subscription this device already holds: a renewal, or a resync.
        if let Some(sub) = self.subs.get_mut(&payload.sub_id) {
            if sub.subscriber != sender {
                // Someone else's id. Never acted on: an update stream is
                // bound to the authenticated subscriber that opened it.
                warn!(sender = %sender, "Dropping a subscribe frame naming another peer's subscription");
                return out;
            }
            // The policy is applied to every renewal, not only the first
            // subscribe: a subscriber renews every half lease, so one never
            // re-examined would keep a peer the policy now refuses (removed
            // from an allowed group, or dropped from the peer list) streamed
            // to for as long as it keeps renewing.
            if admit(&payload.service_id) == Admission::Reject {
                let sub_id = payload.sub_id.clone();
                if let Some(sub) = self.subs.remove(&sub_id) {
                    out.frames.push(if sub.admitted {
                        unsubscribe_frame(sender, &sub_id, "policy")
                    } else {
                        refuse(SubscribeStatus::Rejected, "policy")
                    });
                    out.events.push(provider_changed(
                        &sub_id,
                        &sub,
                        SubscriptionState::Ended,
                        "policy",
                    ));
                }
                return out;
            }
            sub.lease = lease;
            sub.lease_until = now + lease;
            // The filter is the subscriber's to change; a renewal carries it.
            sub.keys = payload.keys.clone();
            if !sub.admitted {
                // Still with the application; the answer follows its decision.
                sub.pending_resume = payload.resume;
                sub.pending_resync |= payload.resync;
                return out;
            }
            let sub_id = payload.sub_id.clone();
            self.answer(
                &sub_id,
                payload.resume.as_ref(),
                payload.resync,
                false,
                now,
                &mut out,
            );
            return out;
        }

        // A subscription this device does not hold: new, or a renewal of one
        // it lost to a restart or a lapsed lease.
        let renewal = payload.resume.is_some();
        let pattern = is_topic_pattern(&payload.topic);
        let topic_key = (payload.service_id.clone(), payload.topic.clone());
        // A pattern takes the topics in its mode and passes over the rest.
        if let Some(topic) = self.topics.get(&topic_key).filter(|_| !pattern) {
            if topic.mode != payload.mode {
                out.frames
                    .push(refuse(SubscribeStatus::ModeMismatch, "mode_mismatch"));
                return out;
            }
        }

        // One subscription per (subscriber, service, topic): the newest wins,
        // so a consumer that restarted is not streamed to twice.
        let replaced: Vec<String> = self
            .subs
            .iter()
            .filter(|(_, sub)| {
                sub.subscriber == sender
                    && sub.service_id == payload.service_id
                    && sub.topic == payload.topic
            })
            .map(|(id, _)| id.clone())
            .collect();
        for old in replaced {
            if let Some(sub) = self.subs.remove(&old) {
                out.events.push(provider_changed(
                    &old,
                    &sub,
                    SubscriptionState::Ended,
                    "replaced",
                ));
            }
        }

        // At capacity the newcomer is refused; nobody is evicted for it.
        let from_sender = self
            .subs
            .values()
            .filter(|s| s.subscriber == sender)
            .count();
        if self.subs.len() >= MAX_PROVIDER_SUBSCRIPTIONS
            || from_sender >= MAX_SUBSCRIPTIONS_PER_PEER
        {
            out.frames.push(refuse(SubscribeStatus::Full, "full"));
            return out;
        }

        let admission = admit(&payload.service_id);
        if admission == Admission::Reject {
            out.frames.push(refuse(SubscribeStatus::Rejected, "policy"));
            return out;
        }

        // No topic is made here: only this device's application makes one,
        // by publishing. A subscription to a topic not yet published waits
        // for it, as a pattern waits for the topics it will match, so a
        // subscriber can neither fix a topic's mode before the application
        // publishes nor leave topics behind it.
        let sub_id = payload.sub_id.clone();
        let mut sub = Sub {
            subscriber: sender.to_string(),
            service_id: payload.service_id.clone(),
            topic: payload.topic.clone(),
            mode: payload.mode,
            params: payload.params.clone(),
            keys: payload.keys.clone(),
            followed: BTreeSet::new(),
            admitted: admission == Admission::Accept,
            asked_at: now,
            lease,
            lease_until: now + lease,
            pending_resume: None,
            pending_resync: false,
            last_sent: None,
            dirty: BTreeSet::new(),
            pattern_full: false,
            pattern_full_reported: false,
        };
        sub.follow_existing(&self.topics);
        self.subs.insert(sub_id.clone(), sub);

        if admission == Admission::Ask {
            if let Some(sub) = self.subs.get_mut(&sub_id) {
                sub.pending_resume = payload.resume;
                sub.pending_resync = payload.resync;
            }
            out.events.push(ServiceEvent::ServiceSubscribeReceived {
                sub_id,
                service_id: payload.service_id,
                topic: payload.topic,
                mode: payload.mode.as_str().to_string(),
                params: payload.params,
                sender: sender.to_string(),
                renewal,
            });
            return out;
        }

        if let Some(sub) = self.subs.get(&sub_id) {
            out.events.push(provider_changed(
                &sub_id,
                sub,
                SubscriptionState::Active,
                "accepted",
            ));
        }
        self.answer(
            &sub_id,
            payload.resume.as_ref(),
            payload.resync,
            true,
            now,
            &mut out,
        );
        out
    }

    /// Handles a `__SVC_UNSUB__` from `sender`. `None` when it names no
    /// subscription `sender` holds here, so the engine can offer it to the
    /// consumer half instead.
    pub fn handle_unsubscribe(
        &mut self,
        content: &str,
        sender: &str,
    ) -> Option<SubscriptionOutput> {
        let payload = decode::<UnsubscribePayload>(content, SVC_UNSUBSCRIBE)?;
        if self.subs.get(&payload.sub_id)?.subscriber != sender {
            return None;
        }
        let sub = self.subs.remove(&payload.sub_id)?;
        let mut out = SubscriptionOutput::default();
        out.events.push(provider_changed(
            &payload.sub_id,
            &sub,
            SubscriptionState::Ended,
            "unsubscribed",
        ));
        Some(out)
    }

    // ------------------------------------------------------------------
    // Calls from the application
    // ------------------------------------------------------------------

    /// Accepts a subscription the application was asked about.
    pub fn accept(
        &mut self,
        sub_id: &str,
        now: Instant,
    ) -> Result<SubscriptionOutput, ServiceError> {
        let sub = self
            .subs
            .get_mut(sub_id)
            .filter(|sub| !sub.admitted)
            .ok_or_else(|| ServiceError::UnknownSubscription(sub_id.to_string()))?;
        sub.admitted = true;
        sub.lease_until = now + sub.lease;
        let resume = sub.pending_resume.take();
        let resync = std::mem::take(&mut sub.pending_resync);
        let mut out = SubscriptionOutput::default();
        out.events.push(provider_changed(
            sub_id,
            sub,
            SubscriptionState::Active,
            "accepted",
        ));
        self.answer(sub_id, resume.as_ref(), resync, true, now, &mut out);
        Ok(out)
    }

    /// Rejects a subscription the application was asked about.
    pub fn reject(
        &mut self,
        sub_id: &str,
        reason: &str,
    ) -> Result<SubscriptionOutput, ServiceError> {
        if !self.subs.get(sub_id).is_some_and(|sub| !sub.admitted) {
            return Err(ServiceError::UnknownSubscription(sub_id.to_string()));
        }
        let Some(sub) = self.subs.remove(sub_id) else {
            return Err(ServiceError::UnknownSubscription(sub_id.to_string()));
        };
        let reason = clamp_reason(if reason.is_empty() {
            "rejected"
        } else {
            reason
        });
        let mut out = SubscriptionOutput::default();
        out.frames.push(SubscriptionFrame {
            recipient: sub.subscriber.clone(),
            content: reply_content(
                sub_id,
                SubscribeStatus::Rejected,
                Some(&reason),
                0,
                "",
                0,
                false,
            ),
            delivery: SubscriptionDelivery::Superseding(reply_key(sub_id)),
        });
        out.events.push(provider_changed(
            sub_id,
            &sub,
            SubscriptionState::Ended,
            "rejected",
        ));
        Ok(out)
    }

    /// Ends a subscription from this side, telling the subscriber. A promise
    /// rather than cryptography: the subscriber keeps its session with us.
    pub fn end(&mut self, sub_id: &str, reason: &str) -> Result<SubscriptionOutput, ServiceError> {
        let sub = self
            .subs
            .remove(sub_id)
            .ok_or_else(|| ServiceError::UnknownSubscription(sub_id.to_string()))?;
        let reason = clamp_reason(if reason.is_empty() { "revoked" } else { reason });
        let mut out = SubscriptionOutput::default();
        out.frames
            .push(unsubscribe_frame(&sub.subscriber, sub_id, &reason));
        out.events.push(provider_changed(
            sub_id,
            &sub,
            SubscriptionState::Ended,
            "revoked",
        ));
        Ok(out)
    }

    /// Ends every subscription on `service_id`, which was unregistered, and
    /// forgets its topics.
    pub fn service_unregistered(&mut self, service_id: &str) -> SubscriptionOutput {
        let mut out = SubscriptionOutput::default();
        let ended: Vec<String> = self
            .subs
            .iter()
            .filter(|(_, sub)| sub.service_id == service_id)
            .map(|(id, _)| id.clone())
            .collect();
        for sub_id in ended {
            if let Some(sub) = self.subs.remove(&sub_id) {
                out.frames.push(unsubscribe_frame(
                    &sub.subscriber,
                    &sub_id,
                    "service_unregistered",
                ));
                out.events.push(provider_changed(
                    &sub_id,
                    &sub,
                    SubscriptionState::Ended,
                    "service_unregistered",
                ));
            }
        }
        self.topics.retain(|(service, _), _| service != service_id);
        out
    }

    /// Publishes to a topic, creating it on first use: a `latest` topic when
    /// `key` is given, a `log` topic when it is not.
    pub fn publish(
        &mut self,
        service_id: &str,
        topic_name: &str,
        key: Option<&str>,
        body: &str,
        now: Instant,
    ) -> Result<(SubscriptionOutput, PublishOutcome), ServiceError> {
        validate_topic(topic_name)?;
        if body.len() > MAX_SERVICE_BODY_SIZE {
            return Err(ServiceError::PayloadTooLarge(format!(
                "update body size {} exceeds maximum {MAX_SERVICE_BODY_SIZE}",
                body.len()
            )));
        }
        let mode = if let Some(key) = key {
            validate_key(key)?;
            SubscriptionMode::Latest
        } else {
            SubscriptionMode::Log
        };
        let topic_key = (service_id.to_string(), topic_name.to_string());
        let mut out = SubscriptionOutput::default();
        if !self.topics.contains_key(&topic_key) {
            self.topics.insert(topic_key.clone(), Topic::new(mode));
            self.end_mismatched(service_id, topic_name, mode, &mut out);
        }
        let topic = self
            .topics
            .get_mut(&topic_key)
            .ok_or_else(|| ServiceError::InvalidArgument(format!("no topic `{topic_name}`")))?;
        if topic.mode != mode {
            return Err(ServiceError::InvalidArgument(
                match topic.mode {
                    SubscriptionMode::Latest => "this topic is `latest`: publish with a key",
                    SubscriptionMode::Log => "this topic is `log`: publish without a key",
                }
                .to_string(),
            ));
        }

        let seq = topic.next_seq;
        match key {
            Some(key) => {
                let previous = topic.latest.get(key).map(|(_, old)| key.len() + old.len());
                if previous.is_none() && topic.latest.len() >= MAX_LATEST_KEYS {
                    return Err(ServiceError::PayloadTooLarge(format!(
                        "a `latest` topic holds at most {MAX_LATEST_KEYS} keys; clear it to start over"
                    )));
                }
                let bytes = topic.latest_bytes - previous.unwrap_or(0) + key.len() + body.len();
                if bytes > MAX_LATEST_BYTES {
                    return Err(ServiceError::PayloadTooLarge(format!(
                        "a `latest` topic holds at most {MAX_LATEST_BYTES} bytes; clear it to start over"
                    )));
                }
                topic.latest_bytes = bytes;
                topic
                    .latest
                    .insert(key.to_string(), (seq, body.to_string()));
            }
            None => {
                topic.log.push_back((seq, body.to_string()));
                topic.log_bytes += body.len();
                while topic.log.len() > MAX_LOG_ENTRIES || topic.log_bytes > MAX_LOG_BYTES {
                    match topic.log.pop_front() {
                        Some((_, dropped)) => topic.log_bytes -= dropped.len(),
                        None => break,
                    }
                }
            }
        }
        topic.next_seq += 1;
        let stream = topic.stream.clone();

        let mut outcome = PublishOutcome::default();
        let interval = self.min_update_interval;
        for (sub_id, sub) in self.subs.iter_mut() {
            if !sub.admitted
                || sub.service_id != service_id
                || key.is_some_and(|key| !key_matches(&sub.keys, key))
                || !sub.follows(topic_name, mode)
            {
                continue;
            }
            outcome.subscribers += 1;
            let label = sub.label(topic_name);
            match key {
                Some(key) => {
                    let due = sub
                        .last_sent
                        .is_none_or(|at| now.saturating_duration_since(at) >= interval);
                    if !due {
                        sub.dirty.insert((topic_name.to_string(), key.to_string()));
                        outcome.coalesced += 1;
                        continue;
                    }
                    sub.dirty.remove(&(topic_name.to_string(), key.to_string()));
                    sub.last_sent = Some(now);
                    out.frames.push(update_frame(
                        &sub.subscriber,
                        sub_id,
                        label,
                        &stream,
                        seq,
                        Some(key),
                        body,
                        false,
                    ));
                }
                None => out.frames.push(update_frame(
                    &sub.subscriber,
                    sub_id,
                    label,
                    &stream,
                    seq,
                    None,
                    body,
                    false,
                )),
            }
        }
        self.report_pattern_full(&mut out);
        Ok((out, outcome))
    }

    /// Sets a `log` topic's checkpoint: a body that summarises everything
    /// published to it so far. A subscriber whose position has fallen out of
    /// the buffer is sent this and what follows it, instead of being told the
    /// history is lost. Creates the `log` topic when there is none, which
    /// ends any subscription waiting for it in `latest` with `mode_mismatch`.
    pub fn set_checkpoint(
        &mut self,
        service_id: &str,
        topic_name: &str,
        body: &str,
    ) -> Result<SubscriptionOutput, ServiceError> {
        validate_topic(topic_name)?;
        if body.len() > MAX_SERVICE_BODY_SIZE {
            return Err(ServiceError::PayloadTooLarge(format!(
                "checkpoint size {} exceeds maximum {MAX_SERVICE_BODY_SIZE}",
                body.len()
            )));
        }
        let topic_key = (service_id.to_string(), topic_name.to_string());
        let mut out = SubscriptionOutput::default();
        if !self.topics.contains_key(&topic_key) {
            self.topics
                .insert(topic_key.clone(), Topic::new(SubscriptionMode::Log));
            self.end_mismatched(service_id, topic_name, SubscriptionMode::Log, &mut out);
        }
        let topic = self
            .topics
            .get_mut(&topic_key)
            .ok_or_else(|| ServiceError::InvalidArgument(format!("no topic `{topic_name}`")))?;
        if topic.mode != SubscriptionMode::Log {
            return Err(ServiceError::InvalidArgument(
                "only a `log` topic has a checkpoint; a `latest` topic is its own".to_string(),
            ));
        }
        topic.checkpoint = Some((topic.last_seq(), body.to_string()));
        Ok(out)
    }

    /// `topic_name` was just created in `mode`: every exact subscription that
    /// waited for it in the other mode ends with `mode_mismatch`, as it would
    /// have been answered had the topic existed when it subscribed.
    fn end_mismatched(
        &mut self,
        service_id: &str,
        topic_name: &str,
        mode: SubscriptionMode,
        out: &mut SubscriptionOutput,
    ) {
        let mismatched: Vec<String> = self
            .subs
            .iter()
            .filter(|(_, sub)| {
                !sub.is_pattern()
                    && sub.service_id == service_id
                    && sub.topic == topic_name
                    && sub.mode != mode
            })
            .map(|(id, _)| id.clone())
            .collect();
        for sub_id in mismatched {
            let Some(sub) = self.subs.remove(&sub_id) else {
                continue;
            };
            out.frames.push(SubscriptionFrame {
                recipient: sub.subscriber.clone(),
                content: reply_content(
                    &sub_id,
                    SubscribeStatus::ModeMismatch,
                    Some("mode_mismatch"),
                    0,
                    "",
                    0,
                    false,
                ),
                delivery: SubscriptionDelivery::Superseding(reply_key(&sub_id)),
            });
            out.events.push(provider_changed(
                &sub_id,
                &sub,
                SubscriptionState::Ended,
                "mode_mismatch",
            ));
        }
    }

    /// Starts a topic over under a new stream: every key, buffered update and
    /// the checkpoint are dropped. Each subscriber is told the new stream, and
    /// resets what it holds for the topic.
    pub fn clear_topic(
        &mut self,
        service_id: &str,
        topic_name: &str,
    ) -> Result<SubscriptionOutput, ServiceError> {
        let topic_key = (service_id.to_string(), topic_name.to_string());
        let mode = self
            .topics
            .get(&topic_key)
            .map(|topic| topic.mode)
            .ok_or_else(|| ServiceError::InvalidArgument(format!("no topic `{topic_name}`")))?;
        let topic = Topic::new(mode);
        let stream = topic.stream.clone();
        self.topics.insert(topic_key, topic);
        let mut out = SubscriptionOutput::default();
        for (sub_id, sub) in self.subs.iter_mut().filter(|(_, sub)| {
            sub.admitted
                && sub.service_id == service_id
                && (sub.topic == topic_name || sub.followed.contains(topic_name))
        }) {
            sub.dirty.retain(|(topic, _)| topic != topic_name);
            let lease_ms = sub.lease.as_millis() as u64;
            let content = if sub.is_pattern() {
                pattern_reply_content(
                    sub_id,
                    lease_ms,
                    vec![TopicReply {
                        topic: topic_name.to_string(),
                        stream: stream.clone(),
                        seq: 0,
                        history_lost: false,
                    }],
                )
            } else {
                reply_content(
                    sub_id,
                    SubscribeStatus::Accepted,
                    None,
                    lease_ms,
                    &stream,
                    0,
                    false,
                )
            };
            // Not under the reply key: a pattern subscription can be told of
            // several topics starting over, and each must arrive.
            out.frames.push(SubscriptionFrame {
                recipient: sub.subscriber.clone(),
                content,
                delivery: if sub.is_pattern() {
                    SubscriptionDelivery::Reliable
                } else {
                    SubscriptionDelivery::Superseding(reply_key(sub_id))
                },
            });
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // Time
    // ------------------------------------------------------------------

    /// Expires leases, times out unanswered approvals, and sends the newest
    /// value of every key held back by the update interval.
    pub fn tick(&mut self, now: Instant) -> SubscriptionOutput {
        let mut out = SubscriptionOutput::default();
        let approval_timeout = self.approval_timeout;
        let expired: Vec<(String, &'static str)> = self
            .subs
            .iter()
            .filter_map(|(id, sub)| {
                if !sub.admitted && now.saturating_duration_since(sub.asked_at) >= approval_timeout
                {
                    Some((id.clone(), "approval_timeout"))
                } else if sub.admitted && now >= sub.lease_until {
                    Some((id.clone(), "expired"))
                } else {
                    None
                }
            })
            .collect();
        for (sub_id, reason) in expired {
            let Some(sub) = self.subs.remove(&sub_id) else {
                continue;
            };
            if reason == "approval_timeout" {
                out.frames.push(SubscriptionFrame {
                    recipient: sub.subscriber.clone(),
                    content: reply_content(
                        &sub_id,
                        SubscribeStatus::Rejected,
                        Some(reason),
                        0,
                        "",
                        0,
                        false,
                    ),
                    delivery: SubscriptionDelivery::Superseding(reply_key(&sub_id)),
                });
            }
            // An expired lease is dropped without a frame: a subscriber that
            // is still there renews, and the renewal subscribes it again.
            debug!(sub_id = %sub_id, reason, "Subscription ended on the provider");
            out.events.push(provider_changed(
                &sub_id,
                &sub,
                SubscriptionState::Ended,
                reason,
            ));
        }

        let interval = self.min_update_interval;
        for (sub_id, sub) in self.subs.iter_mut() {
            if sub.dirty.is_empty()
                || sub
                    .last_sent
                    .is_some_and(|at| now.saturating_duration_since(at) < interval)
            {
                continue;
            }
            for (topic_name, key) in std::mem::take(&mut sub.dirty) {
                let Some(topic) = self
                    .topics
                    .get(&(sub.service_id.clone(), topic_name.clone()))
                else {
                    continue;
                };
                if let Some((seq, body)) = topic.latest.get(&key) {
                    out.frames.push(update_frame(
                        &sub.subscriber,
                        sub_id,
                        sub.label(&topic_name),
                        &topic.stream,
                        *seq,
                        Some(&key),
                        body,
                        false,
                    ));
                }
            }
            sub.last_sent = Some(now);
        }
        out
    }

    // ------------------------------------------------------------------
    // Catch-up
    // ------------------------------------------------------------------

    /// Answers an admitted subscription with `__SVC_SUB_R__` and whatever
    /// catches the subscriber up from `resume` (see [`catch_up_topic`]).
    fn answer(
        &mut self,
        sub_id: &str,
        resume: Option<&ResumePosition>,
        resync: bool,
        fresh: bool,
        now: Instant,
        out: &mut SubscriptionOutput,
    ) {
        let Some(sub) = self.subs.get_mut(sub_id) else {
            return;
        };
        let lease_ms = sub.lease.as_millis() as u64;
        let mut catch_up: Vec<SubscriptionFrame> = Vec::new();

        if !sub.is_pattern() {
            let Some(topic) = self
                .topics
                .get(&(sub.service_id.clone(), sub.topic.clone()))
            else {
                // Not yet published: accepted with no stream, which the
                // consumer reads as nothing to hold yet. The first publish
                // creates the topic and its first update names the stream.
                out.frames.push(SubscriptionFrame {
                    recipient: sub.subscriber.clone(),
                    content: reply_content(
                        sub_id,
                        SubscribeStatus::Accepted,
                        None,
                        lease_ms,
                        "",
                        0,
                        false,
                    ),
                    delivery: SubscriptionDelivery::Superseding(reply_key(sub_id)),
                });
                return;
            };
            // A renewal with no stream was answered before the topic existed
            // and has heard nothing of it: it holds nothing of this stream,
            // which is position zero on it, as for a pattern's unnamed topic.
            let unheard = resume
                .filter(|position| position.stream.is_empty())
                .map(|_| ResumePosition {
                    stream: topic.stream.clone(),
                    seq: 0,
                    digest: digest(std::iter::empty()),
                    topics: Vec::new(),
                });
            let same_stream = unheard
                .as_ref()
                .or_else(|| resume.filter(|position| position.stream == topic.stream));
            let name = sub.topic.clone();
            let caught = catch_up_topic(
                sub_id,
                sub,
                &name,
                topic,
                same_stream,
                resync,
                fresh,
                now,
                &mut catch_up,
            );
            out.frames.push(SubscriptionFrame {
                recipient: sub.subscriber.clone(),
                content: reply_content(
                    sub_id,
                    SubscribeStatus::Accepted,
                    None,
                    lease_ms,
                    &topic.stream,
                    caught.baseline,
                    caught.history_lost,
                ),
                delivery: SubscriptionDelivery::Superseding(reply_key(sub_id)),
            });
            out.frames.extend(catch_up);
            return;
        }

        // A pattern: each topic it follows is caught up as an exact
        // subscription to it would be. A topic missing from a renewal is one
        // the subscriber never heard of on this stream, so it holds nothing
        // of it: position zero, which replays the buffer or the checkpoint.
        // On a first subscribe there is no position, and the checkpoint is
        // the state.
        sub.follow_existing(&self.topics);
        let mut replies = Vec::new();
        for name in sub.followed.clone() {
            let Some(topic) = self.topics.get(&(sub.service_id.clone(), name.clone())) else {
                continue;
            };
            let held =
                resume.and_then(|position| position.topics.iter().find(|held| held.topic == name));
            let position = match held {
                Some(held) if held.stream == topic.stream => Some(ResumePosition {
                    stream: held.stream.clone(),
                    seq: held.seq,
                    digest: held.digest,
                    topics: Vec::new(),
                }),
                Some(_) => None,
                None if resume.is_some() && !fresh => Some(ResumePosition {
                    stream: topic.stream.clone(),
                    seq: 0,
                    digest: digest(std::iter::empty()),
                    topics: Vec::new(),
                }),
                None => None,
            };
            let caught = catch_up_topic(
                sub_id,
                sub,
                &name,
                topic,
                position.as_ref(),
                resync,
                fresh,
                now,
                &mut catch_up,
            );
            replies.push(TopicReply {
                topic: name,
                stream: topic.stream.clone(),
                seq: caught.baseline,
                history_lost: caught.history_lost,
            });
        }
        out.frames.push(SubscriptionFrame {
            recipient: sub.subscriber.clone(),
            content: pattern_reply_content(sub_id, lease_ms, replies),
            delivery: SubscriptionDelivery::Superseding(reply_key(sub_id)),
        });
        out.frames.extend(catch_up);
        self.report_pattern_full(out);
    }

    /// Tells the application, once per subscription, that a pattern
    /// subscription has passed over a matching topic because it already
    /// follows [`MAX_PATTERN_TOPICS`]. Without it the subscriber is silently
    /// sent only part of what its pattern names.
    fn report_pattern_full(&mut self, out: &mut SubscriptionOutput) {
        for (sub_id, sub) in self.subs.iter_mut() {
            if sub.pattern_full && !sub.pattern_full_reported {
                sub.pattern_full_reported = true;
                out.events.push(provider_changed(
                    sub_id,
                    sub,
                    SubscriptionState::Active,
                    "pattern_full",
                ));
            }
        }
    }
}

/// Where one topic's catch-up left a subscriber.
struct CaughtUp {
    /// The position the subscriber may take itself to be at before the
    /// frames that follow.
    baseline: u64,
    /// Neither the buffer nor the checkpoint covers its position.
    history_lost: bool,
}

/// Catches `sub` up on one topic from `same_stream`, its position when it is
/// on the topic's current stream.
///
/// `latest`: a full snapshot of the keys its filter passes when the
/// subscriber is new, asked for one, names another stream, or holds a digest
/// that differs from ours. Otherwise nothing: it holds what we hold.
///
/// `log`: what follows the subscriber's position, from the buffer; the
/// checkpoint and what follows it when the buffer no longer reaches back
/// that far; and `history_lost` when neither covers it. A new subscriber
/// with no position is sent the checkpoint, if there is one, and nothing
/// else: the checkpoint is the topic's state, the buffer its recent news. A
/// resync is sent the checkpoint and what follows it when there is one, and
/// otherwise what follows its position.
#[allow(clippy::too_many_arguments)]
fn catch_up_topic(
    sub_id: &str,
    sub: &mut Sub,
    name: &str,
    topic: &Topic,
    same_stream: Option<&ResumePosition>,
    resync: bool,
    fresh: bool,
    now: Instant,
    catch_up: &mut Vec<SubscriptionFrame>,
) -> CaughtUp {
    let label = sub.label(name);
    let mut baseline = topic.last_seq();
    let mut history_lost = false;
    match topic.mode {
        SubscriptionMode::Latest => {
            let full = fresh
                || resync
                || same_stream.is_none_or(|position| position.digest != topic.digest(&sub.keys));
            if full {
                let mut sent = false;
                for (key, (seq, body)) in topic
                    .latest
                    .iter()
                    .filter(|(key, _)| key_matches(&sub.keys, key))
                {
                    sent = true;
                    catch_up.push(update_frame(
                        &sub.subscriber,
                        sub_id,
                        label,
                        &topic.stream,
                        *seq,
                        Some(key),
                        body,
                        true,
                    ));
                }
                sub.dirty.retain(|(dirty_topic, _)| dirty_topic != name);
                // The interval runs from a value sent, not from an empty
                // snapshot, or the first publish to a new subscriber of an
                // empty topic would wait for it.
                if sent {
                    sub.last_sent = Some(now);
                }
            }
        }
        SubscriptionMode::Log => {
            let replay_after = |after: u64, catch_up: &mut Vec<SubscriptionFrame>| {
                for (seq, body) in topic.log.iter().filter(|(seq, _)| *seq > after) {
                    catch_up.push(update_frame(
                        &sub.subscriber,
                        sub_id,
                        label,
                        &topic.stream,
                        *seq,
                        None,
                        body,
                        false,
                    ));
                }
            };
            let from_checkpoint = |catch_up: &mut Vec<SubscriptionFrame>| {
                let (seq, body) = topic.checkpoint.as_ref()?;
                if !topic.buffer_covers_after(*seq) {
                    return None;
                }
                catch_up.push(update_frame(
                    &sub.subscriber,
                    sub_id,
                    label,
                    &topic.stream,
                    *seq,
                    None,
                    body,
                    true,
                ));
                Some(*seq)
            };
            // A resync asks for the state, so the checkpoint comes first
            // when there is one; the consumer has forgotten its position.
            let resynced = if resync {
                from_checkpoint(catch_up)
            } else {
                None
            };
            match (resynced, same_stream) {
                (Some(seq), _) => {
                    baseline = seq;
                    replay_after(seq, catch_up);
                }
                (None, Some(position)) if topic.buffer_covers_after(position.seq) => {
                    baseline = position.seq.min(topic.last_seq());
                    replay_after(baseline, catch_up);
                }
                (None, Some(_)) => match from_checkpoint(catch_up) {
                    Some(seq) => {
                        baseline = seq;
                        replay_after(seq, catch_up);
                    }
                    None => history_lost = true,
                },
                // New, or a previous life of this topic: the checkpoint is
                // the state, if there is one. The consumer reports a stream
                // change itself, so it is not also history lost.
                (None, None) => {
                    if let Some(seq) = from_checkpoint(catch_up) {
                        baseline = seq;
                        replay_after(seq, catch_up);
                    }
                }
            }
        }
    }
    CaughtUp {
        baseline,
        history_lost,
    }
}

fn clamp_lease(lease_ms: u64) -> Duration {
    Duration::from_millis(lease_ms.clamp(MIN_SUBSCRIPTION_LEASE_MS, MAX_SUBSCRIPTION_LEASE_MS))
}

/// The key a reply supersedes under: one answer per subscription waits.
fn reply_key(sub_id: &str) -> String {
    format!("reply:{sub_id}")
}

fn provider_changed(
    sub_id: &str,
    sub: &Sub,
    state: SubscriptionState,
    reason: &str,
) -> ServiceEvent {
    changed(
        sub_id,
        SubscriptionRole::Provider,
        &sub.service_id,
        &sub.topic,
        &sub.subscriber,
        state,
        reason,
    )
}

fn reply_content(
    sub_id: &str,
    status: SubscribeStatus,
    reason: Option<&str>,
    lease_ms: u64,
    stream: &str,
    seq: u64,
    history_lost: bool,
) -> String {
    let payload = SubscribeReplyPayload {
        v: SUBSCRIPTION_WIRE_VERSION,
        sub_id: sub_id.to_string(),
        status,
        reason: reason.map(str::to_string),
        lease_ms,
        stream: stream.to_string(),
        seq,
        history_lost,
        topics: Vec::new(),
    };
    // Serialising a struct of strings and integers cannot fail.
    encode(SVC_SUBSCRIBE_REPLY, &payload).unwrap_or_default()
}

/// An acceptance of a pattern subscription, with a position per topic.
fn pattern_reply_content(sub_id: &str, lease_ms: u64, topics: Vec<TopicReply>) -> String {
    let payload = SubscribeReplyPayload {
        v: SUBSCRIPTION_WIRE_VERSION,
        sub_id: sub_id.to_string(),
        status: SubscribeStatus::Accepted,
        reason: None,
        lease_ms,
        stream: String::new(),
        seq: 0,
        history_lost: false,
        topics,
    };
    encode(SVC_SUBSCRIBE_REPLY, &payload).unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn update_frame(
    recipient: &str,
    sub_id: &str,
    topic: Option<&str>,
    stream: &str,
    seq: u64,
    key: Option<&str>,
    body: &str,
    snapshot: bool,
) -> SubscriptionFrame {
    let payload = UpdatePayload {
        v: SUBSCRIPTION_WIRE_VERSION,
        sub_id: sub_id.to_string(),
        topic: topic.map(str::to_string),
        stream: stream.to_string(),
        seq,
        key: key.map(str::to_string),
        body: body.to_string(),
        snapshot,
    };
    let delivery = match key {
        // At most one value per key waits for an absent subscriber. A topic
        // has no `:`, so the topic and key cannot run into each other.
        Some(key) => SubscriptionDelivery::LiveOrSuperseding(match topic {
            Some(topic) => format!("upd:{sub_id}:{topic}:{key}"),
            None => format!("upd:{sub_id}:{key}"),
        }),
        // Every log update matters, and a checkpoint is followed by them.
        None => SubscriptionDelivery::Reliable,
    };
    SubscriptionFrame {
        recipient: recipient.to_string(),
        content: encode(SVC_UPDATE, &payload).unwrap_or_default(),
        delivery,
    }
}

pub(crate) fn unsubscribe_frame(recipient: &str, sub_id: &str, reason: &str) -> SubscriptionFrame {
    let payload = UnsubscribePayload {
        v: SUBSCRIPTION_WIRE_VERSION,
        sub_id: sub_id.to_string(),
        reason: reason.to_string(),
    };
    SubscriptionFrame {
        recipient: recipient.to_string(),
        content: encode(SVC_UNSUBSCRIBE, &payload).unwrap_or_default(),
        delivery: SubscriptionDelivery::Reliable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: &str = "alice";
    const MALLORY: &str = "mallory";
    const SVC: &str = "fleet.v1";

    fn provider() -> Provider {
        Provider::new(Duration::from_millis(50), Duration::from_secs(60))
    }

    fn sub_frame(sub_id: &str, topic: &str, mode: SubscriptionMode) -> String {
        encode(
            SVC_SUBSCRIBE,
            &SubscribePayload {
                v: 1,
                sub_id: sub_id.to_string(),
                service_id: SVC.to_string(),
                topic: topic.to_string(),
                mode,
                params: String::new(),
                keys: Vec::new(),
                lease_ms: 60_000,
                resume: None,
                resync: false,
            },
        )
        .unwrap()
    }

    fn renew_frame(
        sub_id: &str,
        topic: &str,
        mode: SubscriptionMode,
        resume: ResumePosition,
    ) -> String {
        encode(
            SVC_SUBSCRIBE,
            &SubscribePayload {
                v: 1,
                sub_id: sub_id.to_string(),
                service_id: SVC.to_string(),
                topic: topic.to_string(),
                mode,
                params: String::new(),
                keys: Vec::new(),
                lease_ms: 60_000,
                resume: Some(resume),
                resync: false,
            },
        )
        .unwrap()
    }

    fn subscribe(
        p: &mut Provider,
        sender: &str,
        frame: &str,
        admission: Admission,
        now: Instant,
    ) -> SubscriptionOutput {
        p.handle_subscribe(frame, sender, now, &|id| id == SVC, &|_| admission)
    }

    fn reply(out: &SubscriptionOutput) -> SubscribeReplyPayload {
        let frame = out
            .frames
            .iter()
            .find(|f| f.content.starts_with(SVC_SUBSCRIBE_REPLY))
            .expect("a reply");
        decode(&frame.content, SVC_SUBSCRIBE_REPLY).unwrap()
    }

    fn stream_of(p: &Provider, topic: &str) -> String {
        p.topics[&(SVC.to_string(), topic.to_string())]
            .stream
            .clone()
    }

    fn updates(out: &SubscriptionOutput) -> Vec<UpdatePayload> {
        out.frames
            .iter()
            .filter_map(|f| decode(&f.content, SVC_UPDATE))
            .collect()
    }

    #[test]
    fn accept_all_answers_and_snapshots_every_key() {
        let mut p = provider();
        let now = Instant::now();
        p.publish(SVC, "battery", Some("pct"), "81", now).unwrap();
        p.publish(SVC, "battery", Some("charging"), "yes", now)
            .unwrap();
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "battery", SubscriptionMode::Latest),
            Admission::Accept,
            now,
        );
        let answer = reply(&out);
        assert_eq!(answer.status, SubscribeStatus::Accepted);
        let snapshot = updates(&out);
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot
            .iter()
            .all(|u| u.snapshot && u.stream == answer.stream));
        assert!(out.frames.iter().all(|f| f.recipient == ALICE));
    }

    #[test]
    fn a_subscriber_never_creates_a_topic_or_fixes_its_mode() {
        let mut p = provider();
        let now = Instant::now();
        // Asked about, never answered: the approval times out.
        subscribe(
            &mut p,
            MALLORY,
            &sub_frame("s1", "status", SubscriptionMode::Log),
            Admission::Ask,
            now,
        );
        p.tick(now + Duration::from_secs(61));
        // Asked about and rejected, many times over.
        for i in 0..100 {
            let id = format!("r{i}");
            subscribe(
                &mut p,
                MALLORY,
                &sub_frame(&id, &format!("junk{i}"), SubscriptionMode::Log),
                Admission::Ask,
                now,
            );
            p.reject(&id, "").unwrap();
        }
        assert!(p.topics.is_empty(), "a subscriber left topics behind");
        p.publish(SVC, "status", Some("pct"), "81", now)
            .expect("the application decides the mode by publishing");
    }

    #[test]
    fn a_subscription_waits_for_a_topic_and_gets_its_first_update() {
        let mut p = provider();
        let now = Instant::now();
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "battery", SubscriptionMode::Latest),
            Admission::Accept,
            now,
        );
        let answer = reply(&out);
        assert_eq!(answer.status, SubscribeStatus::Accepted);
        assert!(
            answer.stream.is_empty(),
            "no stream before the first publish"
        );
        let (out, outcome) = p.publish(SVC, "battery", Some("pct"), "81", now).unwrap();
        assert_eq!(outcome.subscribers, 1);
        assert_eq!(updates(&out).len(), 1);
    }

    #[test]
    fn a_waiting_subscription_in_the_other_mode_ends_when_the_topic_is_made() {
        let mut p = provider();
        let now = Instant::now();
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "battery", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        let (out, outcome) = p.publish(SVC, "battery", Some("pct"), "81", now).unwrap();
        assert_eq!(outcome.subscribers, 0);
        assert_eq!(reply(&out).status, SubscribeStatus::ModeMismatch);
        assert!(!p.holds("s1"));

        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s2", "events", SubscriptionMode::Latest),
            Admission::Accept,
            now,
        );
        let out = p.set_checkpoint(SVC, "events", "cp").unwrap();
        assert_eq!(reply(&out).status, SubscribeStatus::ModeMismatch);
        assert!(!p.holds("s2"));
    }

    #[test]
    fn a_renewal_the_policy_now_refuses_ends_the_subscription() {
        let mut p = provider();
        let now = Instant::now();
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "battery", SubscriptionMode::Latest),
            Admission::Accept,
            now,
        );
        let renewal = renew_frame(
            "s1",
            "battery",
            SubscriptionMode::Latest,
            ResumePosition {
                stream: String::new(),
                seq: 0,
                digest: digest(std::iter::empty()),
                topics: Vec::new(),
            },
        );
        // Still admitted: renewed as before.
        let out = subscribe(&mut p, ALICE, &renewal, Admission::Accept, now);
        assert_eq!(reply(&out).status, SubscribeStatus::Accepted);
        // Removed from the allowed group: the next renewal ends it.
        let out = subscribe(&mut p, ALICE, &renewal, Admission::Reject, now);
        assert!(!p.holds("s1"));
        let ended: UnsubscribePayload = decode(&out.frames[0].content, SVC_UNSUBSCRIBE).unwrap();
        assert_eq!(ended.reason, "policy");
        let (out, outcome) = p.publish(SVC, "battery", Some("pct"), "81", now).unwrap();
        assert_eq!(outcome.subscribers, 0);
        assert!(out.frames.is_empty());

        // One still with the application is refused with a reply instead.
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s2", "battery", SubscriptionMode::Latest),
            Admission::Ask,
            now,
        );
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s2", "battery", SubscriptionMode::Latest),
            Admission::Reject,
            now,
        );
        assert_eq!(reply(&out).status, SubscribeStatus::Rejected);
        assert!(!p.holds("s2"));
    }

    #[test]
    fn a_full_pattern_tells_the_application_once() {
        let mut p = provider();
        let now = Instant::now();
        for i in 0..MAX_PATTERN_TOPICS {
            p.publish(SVC, &format!("alerts/{i:03}"), None, "x", now)
                .unwrap();
        }
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "alerts/*", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        let full = |out: &SubscriptionOutput| {
            out.events
                .iter()
                .filter(|e| {
                    matches!(e, ServiceEvent::ServiceSubscriptionChanged { reason, .. } if reason == "pattern_full")
                })
                .count()
        };
        assert_eq!(full(&out), 0, "exactly at the cap is not past it");
        let (out, outcome) = p.publish(SVC, "alerts/new", None, "x", now).unwrap();
        assert_eq!(outcome.subscribers, 0);
        assert_eq!(full(&out), 1);
        let (out, _) = p.publish(SVC, "alerts/newer", None, "x", now).unwrap();
        assert_eq!(full(&out), 0, "told once");
    }

    #[test]
    fn restricted_asks_and_answers_only_on_accept() {
        let mut p = provider();
        let now = Instant::now();
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "battery", SubscriptionMode::Latest),
            Admission::Ask,
            now,
        );
        assert!(out.frames.is_empty());
        assert!(matches!(
            out.events.as_slice(),
            [ServiceEvent::ServiceSubscribeReceived { renewal: false, .. }]
        ));
        let accepted = p.accept("s1", now).unwrap();
        assert_eq!(reply(&accepted).status, SubscribeStatus::Accepted);
        assert!(p.accept("s1", now).is_err(), "accepted twice");
    }

    #[test]
    fn an_unanswered_approval_is_rejected_on_the_tick() {
        let mut p = provider();
        let now = Instant::now();
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "t", SubscriptionMode::Log),
            Admission::Ask,
            now,
        );
        assert!(p.tick(now + Duration::from_secs(59)).frames.is_empty());
        let out = p.tick(now + Duration::from_secs(61));
        let answer = reply(&out);
        assert_eq!(answer.status, SubscribeStatus::Rejected);
        assert_eq!(answer.reason.as_deref(), Some("approval_timeout"));
        assert!(!p.holds("s1"));
    }

    #[test]
    fn a_policy_rejection_and_an_unknown_service_are_answered() {
        let mut p = provider();
        let now = Instant::now();
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "t", SubscriptionMode::Log),
            Admission::Reject,
            now,
        );
        assert_eq!(reply(&out).status, SubscribeStatus::Rejected);
        let frame = sub_frame("s2", "t", SubscriptionMode::Log).replace(SVC, "other.v1");
        let out = subscribe(&mut p, ALICE, &frame, Admission::Accept, now);
        assert_eq!(reply(&out).status, SubscribeStatus::NotFound);
    }

    #[test]
    fn a_mode_mismatch_is_refused() {
        let mut p = provider();
        let now = Instant::now();
        p.publish(SVC, "t", Some("k"), "v", now).unwrap();
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "t", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        assert_eq!(reply(&out).status, SubscribeStatus::ModeMismatch);
        assert!(p.publish(SVC, "t", None, "v", now).is_err());
    }

    /// Invariant 6: at capacity the newcomer is refused and nobody is evicted.
    #[test]
    fn a_full_provider_refuses_rather_than_evicts() {
        let mut p = provider();
        let now = Instant::now();
        for i in 0..MAX_SUBSCRIPTIONS_PER_PEER {
            let out = subscribe(
                &mut p,
                ALICE,
                &sub_frame(&format!("s{i}"), &format!("t{i}"), SubscriptionMode::Log),
                Admission::Accept,
                now,
            );
            assert_eq!(reply(&out).status, SubscribeStatus::Accepted);
        }
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("extra", "tx", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        assert_eq!(reply(&out).status, SubscribeStatus::Full);
        assert!(p.holds("s0"), "an existing subscription was evicted");
        // Another peer still has room of its own.
        let out = subscribe(
            &mut p,
            MALLORY,
            &sub_frame("m", "t0", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        assert_eq!(reply(&out).status, SubscribeStatus::Accepted);
    }

    /// Invariant 7: one subscription per (subscriber, service, topic).
    #[test]
    fn a_second_subscription_to_one_topic_replaces_the_first() {
        let mut p = provider();
        let now = Instant::now();
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("old", "t", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("new", "t", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        assert!(!p.holds("old") && p.holds("new"));
        assert!(out.events.iter().any(|e| matches!(e, ServiceEvent::ServiceSubscriptionChanged { reason, .. } if reason == "replaced")));
    }

    #[test]
    fn another_peer_cannot_renew_or_end_a_subscription() {
        let mut p = provider();
        let now = Instant::now();
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "t", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        let out = subscribe(
            &mut p,
            MALLORY,
            &sub_frame("s1", "t", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        assert!(out.frames.is_empty() && out.events.is_empty());
        let unsub = encode(
            SVC_UNSUBSCRIBE,
            &UnsubscribePayload {
                v: 1,
                sub_id: "s1".into(),
                reason: String::new(),
            },
        )
        .unwrap();
        assert!(p.handle_unsubscribe(&unsub, MALLORY).is_none());
        assert!(p.holds("s1"));
        assert!(p.handle_unsubscribe(&unsub, ALICE).is_some());
        assert!(!p.holds("s1"));
    }

    #[test]
    fn a_lease_that_is_not_renewed_ends_without_a_frame() {
        let mut p = provider();
        let now = Instant::now();
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "t", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        let out = p.tick(now + Duration::from_secs(61));
        assert!(out.frames.is_empty());
        assert!(!p.holds("s1"));
        assert!(
            matches!(out.events.as_slice(), [ServiceEvent::ServiceSubscriptionChanged { reason, .. }] if reason == "expired")
        );
    }

    #[test]
    fn latest_updates_faster_than_the_interval_coalesce_to_the_newest() {
        let mut p = provider();
        let now = Instant::now();
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "pos", SubscriptionMode::Latest),
            Admission::Accept,
            now,
        );
        let later = now + Duration::from_millis(100);
        let (out, outcome) = p.publish(SVC, "pos", Some("xy"), "1,1", later).unwrap();
        assert_eq!(
            (outcome.subscribers, outcome.coalesced, out.frames.len()),
            (1, 0, 1)
        );
        for body in ["2,2", "3,3", "4,4"] {
            let (out, outcome) = p
                .publish(
                    SVC,
                    "pos",
                    Some("xy"),
                    body,
                    later + Duration::from_millis(10),
                )
                .unwrap();
            assert!(out.frames.is_empty());
            assert_eq!(outcome.coalesced, 1);
        }
        assert!(p.tick(later + Duration::from_millis(20)).frames.is_empty());
        let sent = updates(&p.tick(later + Duration::from_millis(60)));
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].body, "4,4");
        assert_eq!(sent[0].seq, 4);
    }

    #[test]
    fn latest_updates_supersede_per_key_and_log_updates_do_not() {
        let mut p = provider();
        let now = Instant::now();
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "pos", SubscriptionMode::Latest),
            Admission::Accept,
            now,
        );
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s2", "events", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        let later = now + Duration::from_secs(1);
        let (out, _) = p.publish(SVC, "pos", Some("xy"), "1", later).unwrap();
        assert_eq!(
            out.frames[0].delivery,
            SubscriptionDelivery::LiveOrSuperseding("upd:s1:xy".into())
        );
        let (out, _) = p
            .publish(SVC, "events", None, "door opened", later)
            .unwrap();
        assert_eq!(out.frames[0].delivery, SubscriptionDelivery::Reliable);
    }

    #[test]
    fn a_latest_renewal_with_a_matching_digest_sends_nothing_more() {
        let mut p = provider();
        let now = Instant::now();
        p.publish(SVC, "t", Some("a"), "1", now).unwrap();
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "t", SubscriptionMode::Latest),
            Admission::Accept,
            now,
        );
        let stream = reply(&out).stream;
        let held = digest([("a", 1u64)].into_iter());
        let renewal = renew_frame(
            "s1",
            "t",
            SubscriptionMode::Latest,
            ResumePosition {
                stream: stream.clone(),
                seq: 1,
                digest: held,
                topics: Vec::new(),
            },
        );
        let out = subscribe(&mut p, ALICE, &renewal, Admission::Accept, now);
        assert_eq!(reply(&out).status, SubscribeStatus::Accepted);
        assert!(updates(&out).is_empty());
        // Holding something else: the whole table again.
        let renewal = renew_frame(
            "s1",
            "t",
            SubscriptionMode::Latest,
            ResumePosition {
                stream,
                seq: 1,
                digest: held ^ 1,
                topics: Vec::new(),
            },
        );
        assert_eq!(
            updates(&subscribe(&mut p, ALICE, &renewal, Admission::Accept, now)).len(),
            1
        );
    }

    #[test]
    fn a_log_renewal_replays_from_the_buffer() {
        let mut p = provider();
        let now = Instant::now();
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "ev", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        assert!(reply(&out).stream.is_empty(), "not published yet");
        for i in 1..=5 {
            p.publish(SVC, "ev", None, &format!("e{i}"), now).unwrap();
        }
        let stream = stream_of(&p, "ev");
        let renewal = renew_frame(
            "s1",
            "ev",
            SubscriptionMode::Log,
            ResumePosition {
                stream,
                seq: 2,
                digest: 0,
                topics: Vec::new(),
            },
        );
        let out = subscribe(&mut p, ALICE, &renewal, Admission::Accept, now);
        assert_eq!(reply(&out).seq, 2);
        let seqs: Vec<u64> = updates(&out).iter().map(|u| u.seq).collect();
        assert_eq!(seqs, vec![3, 4, 5]);
    }

    /// Invariant 8: past the buffer, the checkpoint and what follows it; with
    /// no checkpoint, `history_lost` rather than a silent gap.
    #[test]
    fn a_log_renewal_past_the_buffer_gets_the_checkpoint_or_history_lost() {
        let mut p = provider();
        let now = Instant::now();
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "ev", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        assert!(reply(&out).stream.is_empty(), "not published yet");
        for i in 0..(MAX_LOG_ENTRIES + 10) {
            p.publish(SVC, "ev", None, &format!("e{i}"), now).unwrap();
        }
        let stream = stream_of(&p, "ev");
        let renewal = renew_frame(
            "s1",
            "ev",
            SubscriptionMode::Log,
            ResumePosition {
                stream: stream.clone(),
                seq: 1,
                digest: 0,
                topics: Vec::new(),
            },
        );
        let out = subscribe(&mut p, ALICE, &renewal, Admission::Accept, now);
        assert!(reply(&out).history_lost);
        assert!(updates(&out).is_empty());

        p.set_checkpoint(SVC, "ev", "state at 266").unwrap();
        p.publish(SVC, "ev", None, "after", now).unwrap();
        let out = subscribe(&mut p, ALICE, &renewal, Admission::Accept, now);
        let answer = reply(&out);
        assert!(!answer.history_lost);
        let sent = updates(&out);
        assert!(sent[0].snapshot && sent[0].body == "state at 266" && sent[0].seq == answer.seq);
        assert_eq!(sent[1].body, "after");
    }

    #[test]
    fn a_new_log_subscriber_gets_the_checkpoint_only() {
        let mut p = provider();
        let now = Instant::now();
        p.publish(SVC, "ev", None, "e1", now).unwrap();
        p.set_checkpoint(SVC, "ev", "cp").unwrap();
        p.publish(SVC, "ev", None, "e2", now).unwrap();
        let out = subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "ev", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        let bodies: Vec<String> = updates(&out).into_iter().map(|u| u.body).collect();
        assert_eq!(bodies, vec!["cp", "e2"]);
    }

    #[test]
    fn clearing_a_topic_tells_every_subscriber_the_new_stream() {
        let mut p = provider();
        let now = Instant::now();
        p.publish(SVC, "t", Some("a"), "1", now).unwrap();
        let first = reply(&subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "t", SubscriptionMode::Latest),
            Admission::Accept,
            now,
        ))
        .stream;
        let out = p.clear_topic(SVC, "t").unwrap();
        let answer = reply(&out);
        assert_ne!(answer.stream, first);
        assert_eq!(answer.status, SubscribeStatus::Accepted);
    }

    #[test]
    fn latest_tables_are_bounded() {
        let mut p = provider();
        let now = Instant::now();
        for i in 0..MAX_LATEST_KEYS {
            p.publish(SVC, "t", Some(&format!("k{i}")), "v", now)
                .unwrap();
        }
        assert!(p.publish(SVC, "t", Some("one-more"), "v", now).is_err());
        assert!(p.publish(SVC, "t", Some("k0"), "replaced", now).is_ok());
        assert!(p
            .publish(
                SVC,
                "big",
                Some("k"),
                &"x".repeat(MAX_SERVICE_BODY_SIZE + 1),
                now
            )
            .is_err());
    }

    #[test]
    fn unregistering_a_service_ends_its_subscriptions() {
        let mut p = provider();
        let now = Instant::now();
        subscribe(
            &mut p,
            ALICE,
            &sub_frame("s1", "t", SubscriptionMode::Log),
            Admission::Accept,
            now,
        );
        let out = p.service_unregistered(SVC);
        assert!(out.frames[0].content.starts_with(SVC_UNSUBSCRIBE));
        assert!(!p.holds("s1"));
    }
}
