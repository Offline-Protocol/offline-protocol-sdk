use std::collections::{BTreeMap, BTreeSet, HashMap};

use offline_protocol_core::time::{Duration, Instant};
use tracing::{debug, warn};

use super::provider::unsubscribe_frame;
use super::{
    changed, clamp_reason, decode, digest, encode, validate_topic, SubscriptionDelivery,
    SubscriptionFrame, SubscriptionOutput, SubscriptionRole, SubscriptionState,
};
use crate::error::ServiceError;
use crate::events::ServiceEvent;
use crate::payloads::{
    ResumePosition, SubscribePayload, SubscribeReplyPayload, SubscribeStatus, SubscriptionMode,
    UnsubscribePayload, UpdatePayload, MAX_CONSUMER_SUBSCRIPTIONS, MAX_CONSUMER_TRACKED_KEYS,
    MAX_LOG_SEEN_AHEAD, MAX_SUBSCRIPTION_LEASE_MS, MAX_SUBSCRIPTION_PARAMS_SIZE,
    MAX_UNANSWERED_RENEWALS, MIN_SUBSCRIPTION_LEASE_MS, SUBSCRIPTION_WIRE_VERSION, SVC_SUBSCRIBE,
    SVC_SUBSCRIBE_REPLY, SVC_UNSUBSCRIBE, SVC_UPDATE,
};

/// One subscription this device holds, for the application to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionInfo {
    /// The subscription's identifier.
    pub sub_id: String,
    /// The provider.
    pub provider: String,
    /// The subscribed service.
    pub service_id: String,
    /// The subscribed topic.
    pub topic: String,
    /// The mode asked for.
    pub mode: SubscriptionMode,
    /// Where it stands.
    pub state: SubscriptionState,
}

/// One subscription this device holds on another.
struct Sub {
    provider: String,
    service_id: String,
    topic: String,
    mode: SubscriptionMode,
    params: String,
    state: SubscriptionState,
    /// Whether the subscribe frame has left. Until it has, the subscription
    /// waits for the session with the provider to confirm.
    sent: bool,
    created_at: Instant,
    /// The stream the provider's frames carry. `None` until the first one.
    stream: Option<String>,
    /// `latest`: the seq of the value held for each key.
    keys: BTreeMap<String, u64>,
    /// `log`: every seq up to here has been delivered. `None` until the
    /// provider says where to start.
    contiguous: Option<u64>,
    /// `log`: seqs delivered above `contiguous`, waiting for the gap below
    /// them to fill.
    seen_ahead: BTreeSet<u64>,
    lease: Duration,
    next_renewal: Option<Instant>,
    renewal_outstanding: bool,
    unanswered: u32,
    paused_since: Option<Instant>,
}

impl Sub {
    /// Where this subscription stands, for a renewal or a resync.
    fn resume(&self) -> Option<ResumePosition> {
        let stream = self.stream.clone()?;
        Some(match self.mode {
            SubscriptionMode::Latest => ResumePosition {
                stream,
                seq: self.keys.values().copied().max().unwrap_or(0),
                digest: digest(self.keys.iter().map(|(key, seq)| (key.as_str(), *seq))),
            },
            SubscriptionMode::Log => ResumePosition {
                stream,
                seq: self.contiguous.unwrap_or(0),
                digest: 0,
            },
        })
    }

    /// Forgets everything held for the provider's previous stream.
    fn reset_stream(&mut self, stream: &str) {
        self.stream = Some(stream.to_string());
        self.keys.clear();
        self.contiguous = None;
        self.seen_ahead.clear();
    }

    /// Moves `contiguous` up through every seq already seen above it.
    fn advance(&mut self) {
        let Some(mut position) = self.contiguous else {
            return;
        };
        while self.seen_ahead.remove(&(position + 1)) {
            position += 1;
        }
        self.contiguous = Some(position);
        self.seen_ahead.retain(|seq| *seq > position);
    }
}

/// The consumer half of subscriptions: what this device has subscribed to,
/// how far it has got in each, and when each must be renewed.
pub struct Consumer {
    subs: HashMap<String, Sub>,
    lease: Duration,
    pending_timeout: Duration,
    max_pause: Duration,
}

impl Consumer {
    /// A consumer that asks for `lease`, gives up on an unanswered
    /// subscription after `pending_timeout`, and ends one paused for longer
    /// than `max_pause`.
    pub fn new(lease: Duration, pending_timeout: Duration, max_pause: Duration) -> Self {
        Self {
            subs: HashMap::new(),
            lease,
            pending_timeout,
            max_pause,
        }
    }

    /// Changes the timing for what follows.
    pub fn set_timing(&mut self, lease: Duration, pending_timeout: Duration, max_pause: Duration) {
        self.lease = lease;
        self.pending_timeout = pending_timeout;
        self.max_pause = max_pause;
    }

    /// Whether `sub_id` is a subscription this device holds.
    pub fn holds(&self, sub_id: &str) -> bool {
        self.subs.contains_key(sub_id)
    }

    /// The provider of `sub_id`.
    pub fn provider_of(&self, sub_id: &str) -> Option<&str> {
        self.subs.get(sub_id).map(|sub| sub.provider.as_str())
    }

    /// Every subscription this device holds.
    pub fn subscriptions(&self) -> Vec<SubscriptionInfo> {
        let mut list: Vec<SubscriptionInfo> = self
            .subs
            .iter()
            .map(|(sub_id, sub)| SubscriptionInfo {
                sub_id: sub_id.clone(),
                provider: sub.provider.clone(),
                service_id: sub.service_id.clone(),
                topic: sub.topic.clone(),
                mode: sub.mode,
                state: sub.state,
            })
            .collect();
        list.sort_by(|a, b| a.sub_id.cmp(&b.sub_id));
        list
    }

    // ------------------------------------------------------------------
    // Calls from the application
    // ------------------------------------------------------------------

    /// Opens a subscription and returns its id and the frame that asks for
    /// it. The subscription is `pending` until the provider answers; the
    /// engine calls [`Self::mark_sent`] once the frame has left, and until
    /// then offers it again from [`Self::frames_awaiting_session`].
    pub fn subscribe(
        &mut self,
        provider: &str,
        service_id: &str,
        topic: &str,
        mode: SubscriptionMode,
        params: &str,
        now: Instant,
    ) -> Result<(String, SubscriptionFrame), ServiceError> {
        validate_topic(topic)?;
        if params.len() > MAX_SUBSCRIPTION_PARAMS_SIZE {
            return Err(ServiceError::PayloadTooLarge(format!(
                "params size {} exceeds maximum {MAX_SUBSCRIPTION_PARAMS_SIZE}",
                params.len()
            )));
        }
        if let Some((existing, _)) = self.subs.iter().find(|(_, sub)| {
            sub.provider == provider && sub.service_id == service_id && sub.topic == topic
        }) {
            return Err(ServiceError::InvalidArgument(format!(
                "already subscribed to this topic as {existing}"
            )));
        }
        if self.subs.len() >= MAX_CONSUMER_SUBSCRIPTIONS {
            return Err(ServiceError::InvalidArgument(format!(
                "a device holds at most {MAX_CONSUMER_SUBSCRIPTIONS} subscriptions"
            )));
        }
        let sub_id = uuid::Uuid::new_v4().to_string();
        let sub = Sub {
            provider: provider.to_string(),
            service_id: service_id.to_string(),
            topic: topic.to_string(),
            mode,
            params: params.to_string(),
            state: SubscriptionState::Pending,
            sent: false,
            created_at: now,
            stream: None,
            keys: BTreeMap::new(),
            contiguous: None,
            seen_ahead: BTreeSet::new(),
            lease: self.lease,
            next_renewal: None,
            renewal_outstanding: false,
            unanswered: 0,
            paused_since: None,
        };
        let frame = subscribe_frame(&sub_id, &sub, None, false)?;
        self.subs.insert(sub_id.clone(), sub);
        Ok((sub_id, frame))
    }

    /// Records that the subscribe frame for `sub_id` has left.
    pub fn mark_sent(&mut self, sub_id: &str) {
        if let Some(sub) = self.subs.get_mut(sub_id) {
            sub.sent = true;
        }
    }

    /// The subscribe frames still waiting for the session with `provider`,
    /// paired with their subscription ids.
    pub fn frames_awaiting_session(&self, provider: &str) -> Vec<(String, SubscriptionFrame)> {
        self.subs
            .iter()
            .filter(|(_, sub)| {
                sub.provider == provider && !sub.sent && sub.state == SubscriptionState::Pending
            })
            .filter_map(|(sub_id, sub)| {
                subscribe_frame(sub_id, sub, None, false)
                    .ok()
                    .map(|frame| (sub_id.clone(), frame))
            })
            .collect()
    }

    /// Ends `sub_id`. Returns the frame that tells the provider, when one is
    /// owed (the subscribe frame left), and the event; `None` when there is
    /// no such subscription.
    pub fn unsubscribe(&mut self, sub_id: &str) -> Option<SubscriptionOutput> {
        let sub = self.subs.remove(sub_id)?;
        let mut out = SubscriptionOutput::default();
        if sub.sent {
            out.frames
                .push(unsubscribe_frame(&sub.provider, sub_id, "unsubscribed"));
        }
        out.events.push(consumer_changed(
            sub_id,
            &sub,
            SubscriptionState::Ended,
            "unsubscribed",
        ));
        Some(out)
    }

    /// Asks the provider for the state again now: a full snapshot of a
    /// `latest` topic, or a `log` topic's checkpoint and what follows it
    /// (what follows this device's position when there is no checkpoint).
    ///
    /// The position is forgotten first, so that the values that come back
    /// are delivered even where they equal what was delivered before: the
    /// application asked for them. Ordering still holds from here on, since a
    /// value older than the snapshot that arrives after it is dropped.
    pub fn resync(&mut self, sub_id: &str) -> Result<SubscriptionFrame, ServiceError> {
        let sub = self
            .subs
            .get_mut(sub_id)
            .filter(|sub| sub.sent && sub.state != SubscriptionState::Ended)
            .ok_or_else(|| ServiceError::UnknownSubscription(sub_id.to_string()))?;
        sub.renewal_outstanding = true;
        let resume = sub.resume();
        sub.keys.clear();
        sub.contiguous = None;
        sub.seen_ahead.clear();
        subscribe_frame(sub_id, sub, resume, true)
    }

    // ------------------------------------------------------------------
    // Frames from providers
    // ------------------------------------------------------------------

    /// Handles a `__SVC_SUB_R__` from `sender`.
    pub fn handle_reply(
        &mut self,
        content: &str,
        sender: &str,
        now: Instant,
    ) -> SubscriptionOutput {
        let mut out = SubscriptionOutput::default();
        let Some(payload) = decode::<SubscribeReplyPayload>(content, SVC_SUBSCRIBE_REPLY) else {
            return out;
        };
        let Some(sub) = self
            .subs
            .get_mut(&payload.sub_id)
            .filter(|sub| sub.provider == sender)
        else {
            debug!(sender = %sender, "Dropping a subscription reply for no subscription of this provider");
            return out;
        };

        if payload.status != SubscribeStatus::Accepted {
            let reason = payload
                .reason
                .as_deref()
                .map(clamp_reason)
                .unwrap_or_else(|| payload.status.as_str().to_string());
            if let Some(sub) = self.subs.remove(&payload.sub_id) {
                out.events.push(consumer_changed(
                    &payload.sub_id,
                    &sub,
                    SubscriptionState::Ended,
                    &reason,
                ));
            }
            return out;
        }

        let reset = sub
            .stream
            .as_deref()
            .is_some_and(|stream| stream != payload.stream);
        if reset || sub.stream.is_none() {
            sub.reset_stream(&payload.stream);
        }
        if sub.mode == SubscriptionMode::Log {
            sub.contiguous = Some(sub.contiguous.map_or(payload.seq, |c| c.max(payload.seq)));
            sub.advance();
        }
        sub.lease = Duration::from_millis(
            payload
                .lease_ms
                .clamp(MIN_SUBSCRIPTION_LEASE_MS, MAX_SUBSCRIPTION_LEASE_MS),
        );
        sub.next_renewal = Some(now + sub.lease / 2);
        sub.renewal_outstanding = false;
        sub.unanswered = 0;
        sub.sent = true;

        let previous = sub.state;
        sub.state = SubscriptionState::Active;
        sub.paused_since = None;
        match previous {
            SubscriptionState::Pending => out.events.push(consumer_changed(
                &payload.sub_id,
                sub,
                SubscriptionState::Active,
                "accepted",
            )),
            SubscriptionState::Paused => out.events.push(consumer_changed(
                &payload.sub_id,
                sub,
                SubscriptionState::Active,
                "resumed",
            )),
            _ => {}
        }
        if reset {
            out.events.push(consumer_changed(
                &payload.sub_id,
                sub,
                SubscriptionState::Active,
                "stream_reset",
            ));
        }
        if payload.history_lost {
            out.events.push(consumer_changed(
                &payload.sub_id,
                sub,
                SubscriptionState::Active,
                "history_lost",
            ));
        }
        out
    }

    /// Handles a `__SVC_UPD__` from `sender`. Delivered only from the
    /// subscription's own provider, and never older than what was delivered
    /// before it for the same key.
    pub fn handle_update(&mut self, content: &str, sender: &str) -> SubscriptionOutput {
        let mut out = SubscriptionOutput::default();
        let Some(payload) = decode::<UpdatePayload>(content, SVC_UPDATE) else {
            return out;
        };
        let Some(sub) = self
            .subs
            .get_mut(&payload.sub_id)
            .filter(|sub| sub.provider == sender && sub.state != SubscriptionState::Ended)
        else {
            debug!(sender = %sender, "Dropping an update for no subscription of this provider");
            return out;
        };

        match sub.stream.as_deref() {
            None => sub.reset_stream(&payload.stream),
            Some(stream) if stream != payload.stream => {
                sub.reset_stream(&payload.stream);
                out.events.push(consumer_changed(
                    &payload.sub_id,
                    sub,
                    sub.state,
                    "stream_reset",
                ));
            }
            Some(_) => {}
        }

        let fresh = match sub.mode {
            SubscriptionMode::Latest => {
                let Some(key) = payload.key.as_deref() else {
                    warn!("Dropping a `latest` update with no key");
                    return out;
                };
                match sub.keys.get(key) {
                    Some(held) if *held >= payload.seq => false,
                    None if sub.keys.len() >= MAX_CONSUMER_TRACKED_KEYS => {
                        warn!("Dropping an update for a key beyond what one subscription tracks");
                        false
                    }
                    _ => {
                        sub.keys.insert(key.to_string(), payload.seq);
                        true
                    }
                }
            }
            SubscriptionMode::Log => {
                if payload.snapshot {
                    // A checkpoint: everything up to its seq is summarised.
                    // Stale only once this device is past it; the reply that
                    // precedes it sets the position to exactly its seq.
                    if sub.contiguous.is_some_and(|c| c > payload.seq) {
                        false
                    } else {
                        sub.contiguous = Some(payload.seq);
                        sub.advance();
                        true
                    }
                } else {
                    match sub.contiguous {
                        Some(c) if c >= payload.seq => false,
                        _ if sub.seen_ahead.contains(&payload.seq) => false,
                        None => {
                            sub.contiguous = Some(payload.seq);
                            sub.advance();
                            true
                        }
                        Some(_) => {
                            sub.seen_ahead.insert(payload.seq);
                            if sub.seen_ahead.len() > MAX_LOG_SEEN_AHEAD {
                                // The gap below will not fill in time. Give it
                                // up rather than remember without bound; the
                                // next renewal asks the provider for it.
                                if let Some(lowest) = sub.seen_ahead.pop_first() {
                                    sub.contiguous = Some(lowest);
                                }
                            }
                            sub.advance();
                            true
                        }
                    }
                }
            }
        };
        if !fresh {
            return out;
        }
        out.events.push(ServiceEvent::ServiceUpdateReceived {
            sub_id: payload.sub_id,
            service_id: sub.service_id.clone(),
            topic: sub.topic.clone(),
            key: payload.key,
            seq: payload.seq,
            body: payload.body,
            snapshot: payload.snapshot,
            provider_peer_id: sender.to_string(),
        });
        out
    }

    /// Handles a `__SVC_UNSUB__` from `sender`. `None` when it names no
    /// subscription this device holds on `sender`.
    pub fn handle_unsubscribe(
        &mut self,
        content: &str,
        sender: &str,
    ) -> Option<SubscriptionOutput> {
        let payload = decode::<UnsubscribePayload>(content, SVC_UNSUBSCRIBE)?;
        if self.subs.get(&payload.sub_id)?.provider != sender {
            return None;
        }
        let sub = self.subs.remove(&payload.sub_id)?;
        let reason = if payload.reason.is_empty() {
            "revoked".to_string()
        } else {
            clamp_reason(&payload.reason)
        };
        let mut out = SubscriptionOutput::default();
        out.events.push(consumer_changed(
            &payload.sub_id,
            &sub,
            SubscriptionState::Ended,
            &reason,
        ));
        Some(out)
    }

    // ------------------------------------------------------------------
    // Time and presence
    // ------------------------------------------------------------------

    /// Renews what is due, pauses what has stopped answering, and ends what
    /// waited too long.
    pub fn tick(&mut self, now: Instant) -> SubscriptionOutput {
        let mut out = SubscriptionOutput::default();
        let mut ended: Vec<(String, &'static str)> = Vec::new();
        for (sub_id, sub) in self.subs.iter_mut() {
            match sub.state {
                SubscriptionState::Pending => {
                    if now.saturating_duration_since(sub.created_at) >= self.pending_timeout {
                        ended.push((
                            sub_id.clone(),
                            if sub.sent {
                                "no_reply"
                            } else {
                                "session_timeout"
                            },
                        ));
                    }
                }
                SubscriptionState::Paused
                    if sub.paused_since.is_some_and(|since| {
                        now.saturating_duration_since(since) >= self.max_pause
                    }) =>
                {
                    ended.push((sub_id.clone(), "provider_unreachable"));
                }
                SubscriptionState::Active | SubscriptionState::Paused => {
                    if sub.next_renewal.is_none_or(|at| now < at) {
                        continue;
                    }
                    if sub.renewal_outstanding {
                        sub.unanswered += 1;
                    }
                    if sub.unanswered >= MAX_UNANSWERED_RENEWALS
                        && sub.state == SubscriptionState::Active
                    {
                        sub.state = SubscriptionState::Paused;
                        sub.paused_since = Some(now);
                        out.events.push(consumer_changed(
                            sub_id,
                            sub,
                            SubscriptionState::Paused,
                            "renewal_unanswered",
                        ));
                    }
                    sub.renewal_outstanding = true;
                    sub.next_renewal = Some(now + sub.lease / 2);
                    if let Ok(frame) = subscribe_frame(sub_id, sub, sub.resume(), false) {
                        out.frames.push(frame);
                    }
                }
                SubscriptionState::Ended => {}
            }
        }
        for (sub_id, reason) in ended {
            if let Some(sub) = self.subs.remove(&sub_id) {
                out.events.push(consumer_changed(
                    &sub_id,
                    &sub,
                    SubscriptionState::Ended,
                    reason,
                ));
            }
        }
        out
    }

    /// `peer` was seen again: a paused subscription on it renews at once
    /// rather than at its next renewal time.
    pub fn on_peer_seen(&mut self, peer: &str) -> SubscriptionOutput {
        let mut out = SubscriptionOutput::default();
        for (sub_id, sub) in self
            .subs
            .iter_mut()
            .filter(|(_, sub)| sub.provider == peer && sub.state == SubscriptionState::Paused)
        {
            sub.renewal_outstanding = true;
            if let Ok(frame) = subscribe_frame(sub_id, sub, sub.resume(), false) {
                out.frames.push(frame);
            }
        }
        out
    }
}

/// `__SVC_SUB__` for `sub`: a subscribe without `resume`, a renewal with it,
/// or a resync. One waits per subscription.
fn subscribe_frame(
    sub_id: &str,
    sub: &Sub,
    resume: Option<ResumePosition>,
    resync: bool,
) -> Result<SubscriptionFrame, ServiceError> {
    let payload = SubscribePayload {
        v: SUBSCRIPTION_WIRE_VERSION,
        sub_id: sub_id.to_string(),
        service_id: sub.service_id.clone(),
        topic: sub.topic.clone(),
        mode: sub.mode,
        params: sub.params.clone(),
        lease_ms: sub.lease.as_millis() as u64,
        resume,
        resync,
    };
    Ok(SubscriptionFrame {
        recipient: sub.provider.clone(),
        content: encode(SVC_SUBSCRIBE, &payload)?,
        delivery: SubscriptionDelivery::Superseding(format!("sub:{sub_id}")),
    })
}

fn consumer_changed(
    sub_id: &str,
    sub: &Sub,
    state: SubscriptionState,
    reason: &str,
) -> ServiceEvent {
    changed(
        sub_id,
        SubscriptionRole::Consumer,
        &sub.service_id,
        &sub.topic,
        &sub.provider,
        state,
        reason,
    )
}
