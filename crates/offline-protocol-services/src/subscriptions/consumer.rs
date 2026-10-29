use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use offline_protocol_core::time::{Duration, Instant};
use tracing::{debug, warn};

use super::provider::unsubscribe_frame;
use super::{
    changed, clamp_reason, decode, digest, encode, is_topic_pattern, key_matches, topic_matches,
    validate_key, validate_key_filter, validate_subscription_topic, SubscribeOptions,
    SubscriptionDelivery, SubscriptionFrame, SubscriptionOutput, SubscriptionRole,
    SubscriptionState,
};
use crate::error::ServiceError;
use crate::events::ServiceEvent;
use crate::payloads::{
    ResumePosition, SubscribePayload, SubscribeReplyPayload, SubscribeStatus, SubscriptionMode,
    TopicPosition, UnsubscribePayload, UpdatePayload, MAX_CONSUMER_SUBSCRIPTIONS,
    MAX_CONSUMER_TRACKED_KEYS, MAX_LOG_SEEN_AHEAD, MAX_PATTERN_TOPICS, MAX_RETIRED_PER_TOPIC,
    MAX_SUBSCRIPTION_LEASE_MS, MAX_SUBSCRIPTION_PARAMS_SIZE, MAX_UNANSWERED_RENEWALS,
    MIN_SUBSCRIPTION_LEASE_MS, SUBSCRIPTION_WIRE_VERSION, SVC_SUBSCRIBE, SVC_SUBSCRIBE_REPLY,
    SVC_UNSUBSCRIBE, SVC_UPDATE,
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
    /// The subscribed topic, or a pattern (`prefix*`).
    pub topic: String,
    /// The mode asked for.
    pub mode: SubscriptionMode,
    /// Where it stands.
    pub state: SubscriptionState,
    /// The `latest` keys asked for; empty for every key.
    pub keys: Vec<String>,
    /// The topics it has received a position or an update for: the topic
    /// itself for an exact subscription, each matching topic for a pattern.
    pub topics: Vec<String>,
}

/// How far a subscription has got in one topic: the stream it is on and
/// what it holds of it. An exact subscription has one; a pattern
/// subscription one per topic it has heard of.
#[derive(Default)]
struct Track {
    /// The stream the provider's frames carry. `None` until the first one.
    stream: Option<String>,
    /// `latest`: the seq of the value held for each key.
    keys: BTreeMap<String, u64>,
    /// `log`: every seq up to here has been delivered or summarised. Starts
    /// at 0, "nothing of this stream yet" (seqs start at 1), and is raised by
    /// a reply's baseline, a checkpoint, or the next seq in turn; never set
    /// from an arbitrary update. The reliable ladder keeps no order between
    /// entries, so a resent reply can land after the catch-up frames that
    /// followed it: were the first update to set the position, everything
    /// between the baseline and it would be dropped as already seen.
    contiguous: u64,
    /// `log`: seqs delivered above `contiguous`, waiting for the gap below
    /// them to fill.
    seen_ahead: BTreeSet<u64>,
    /// Streams this topic has moved away from, newest last, at most
    /// [`MAX_RETIRED_PER_TOPIC`]. A frame naming one is from before a
    /// provider restart, still in a relay mailbox or with a custodian; taken,
    /// it would reset the consumer to the dead stream, and the live stream's
    /// next frame would reset it back.
    retired: VecDeque<String>,
}

impl Track {
    /// Where this track stands, for a renewal or a resync.
    fn position(&self, mode: SubscriptionMode) -> Option<(String, u64, u64)> {
        let stream = self.stream.clone()?;
        Some(match mode {
            SubscriptionMode::Latest => (
                stream,
                self.keys.values().copied().max().unwrap_or(0),
                digest(self.keys.iter().map(|(key, seq)| (key.as_str(), *seq))),
            ),
            SubscriptionMode::Log => (stream, self.contiguous, 0),
        })
    }

    /// Forgets everything held for the provider's previous stream, and
    /// retires that stream.
    fn reset_stream(&mut self, stream: &str) {
        if let Some(previous) = self.stream.replace(stream.to_string()) {
            self.retired.retain(|retired| *retired != previous);
            self.retired.push_back(previous);
            if self.retired.len() > MAX_RETIRED_PER_TOPIC {
                self.retired.pop_front();
            }
        }
        self.forget();
    }

    /// Whether `stream` is one this topic has moved away from.
    fn is_retired(&self, stream: &str) -> bool {
        self.retired.iter().any(|retired| retired == stream)
    }

    /// Forgets what is held, keeping the stream.
    fn forget(&mut self) {
        self.keys.clear();
        self.contiguous = 0;
        self.seen_ahead.clear();
    }

    /// Moves `contiguous` up through every seq already seen above it.
    fn advance(&mut self) {
        while self.seen_ahead.remove(&(self.contiguous + 1)) {
            self.contiguous += 1;
        }
        let position = self.contiguous;
        self.seen_ahead.retain(|seq| *seq > position);
    }
}

/// One subscription this device holds on another.
struct Sub {
    provider: String,
    service_id: String,
    topic: String,
    mode: SubscriptionMode,
    params: String,
    /// `latest` keys asked for; empty for every key.
    filter: Vec<String>,
    state: SubscriptionState,
    /// Whether the subscribe frame has left. Until it has, the subscription
    /// waits for the session with the provider to confirm.
    sent: bool,
    created_at: Instant,
    /// What is held of each topic, by topic name. At most
    /// [`MAX_PATTERN_TOPICS`] entries.
    tracks: BTreeMap<String, Track>,
    lease: Duration,
    next_renewal: Option<Instant>,
    renewal_outstanding: bool,
    unanswered: u32,
    paused_since: Option<Instant>,
    /// A resync the provider has not answered yet. Every frame for the
    /// subscription carries `resync` until a reply arrives, so a renewal that
    /// supersedes the queued resync does not withdraw the application's
    /// request with it.
    resync_pending: bool,
    /// A pattern subscription was told it follows as many topics as it may,
    /// and the application has been told; once per subscription.
    pattern_full_reported: bool,
}

impl Sub {
    fn is_pattern(&self) -> bool {
        is_topic_pattern(&self.topic)
    }

    /// Where this subscription stands, for a renewal or a resync. A pattern
    /// subscription that has been answered always has one, even with no
    /// topic yet, so that the provider treats every topic missing from it as
    /// one the subscriber holds nothing of.
    fn resume(&self) -> Option<ResumePosition> {
        if !self.is_pattern() {
            let held = self
                .tracks
                .get(&self.topic)
                .and_then(|track| track.position(self.mode));
            // Answered before the topic was published, and nothing heard of
            // it since: no stream, position zero, so that the provider
            // replays what this device missed rather than treat it as new.
            let (stream, seq, digest) = match held {
                Some(held) => held,
                None if self.state == SubscriptionState::Pending => return None,
                None => (String::new(), 0, digest(std::iter::empty())),
            };
            return Some(ResumePosition {
                stream,
                seq,
                digest,
                topics: Vec::new(),
            });
        }
        if self.state == SubscriptionState::Pending {
            return None;
        }
        let topics = self
            .tracks
            .iter()
            .filter_map(|(topic, track)| {
                let (stream, seq, digest) = track.position(self.mode)?;
                Some(TopicPosition {
                    topic: topic.clone(),
                    stream,
                    seq,
                    digest,
                })
            })
            .collect();
        Some(ResumePosition {
            stream: String::new(),
            seq: 0,
            digest: 0,
            topics,
        })
    }

    /// The track for `topic`, made when there is room. `None` for a topic
    /// this subscription does not cover, or one past the cap.
    fn track(&mut self, topic: &str) -> Option<&mut Track> {
        if !topic_matches(&self.topic, topic) || is_topic_pattern(topic) {
            return None;
        }
        if !self.tracks.contains_key(topic) && self.tracks.len() >= MAX_PATTERN_TOPICS {
            warn!("Dropping a frame for a topic beyond what one subscription tracks");
            return None;
        }
        Some(self.tracks.entry(topic.to_string()).or_default())
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
                keys: sub.filter.clone(),
                topics: sub
                    .tracks
                    .iter()
                    .filter(|(_, track)| track.stream.is_some())
                    .map(|(topic, _)| topic.clone())
                    .collect(),
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
    ///
    /// `topic` may be a pattern, `prefix*`: the provider then sends the
    /// updates of every topic in `mode` whose name starts with `prefix`, up
    /// to [`MAX_PATTERN_TOPICS`], each tracked and recovered on its own.
    pub fn subscribe(
        &mut self,
        provider: &str,
        service_id: &str,
        topic: &str,
        options: &SubscribeOptions,
        now: Instant,
    ) -> Result<(String, SubscriptionFrame), ServiceError> {
        validate_subscription_topic(topic)?;
        validate_key_filter(&options.keys, options.mode)?;
        let params = options.params.as_str();
        let mode = options.mode;
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
            filter: options.keys.clone(),
            state: SubscriptionState::Pending,
            sent: false,
            created_at: now,
            tracks: BTreeMap::new(),
            lease: self.lease,
            next_renewal: None,
            renewal_outstanding: false,
            unanswered: 0,
            paused_since: None,
            resync_pending: false,
            pattern_full_reported: false,
        };
        let frame = subscribe_frame(&sub_id, &sub, None)?;
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
                subscribe_frame(sub_id, sub, None)
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
        sub.resync_pending = true;
        let resume = sub.resume();
        for track in sub.tracks.values_mut() {
            track.forget();
        }
        subscribe_frame(sub_id, sub, resume)
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

        // Each topic the reply positions: the subscription's own, or for a
        // pattern each topic it follows.
        let positions: Vec<(String, String, u64, bool)> = if sub.is_pattern() {
            payload
                .topics
                .iter()
                .map(|t| (t.topic.clone(), t.stream.clone(), t.seq, t.history_lost))
                .collect()
        } else {
            vec![(
                sub.topic.clone(),
                payload.stream.clone(),
                payload.seq,
                payload.history_lost,
            )]
        };
        // A reply naming a stream this device has left is from before the
        // provider restarted: none of it describes the live stream.
        let stale = positions.iter().any(|(topic, stream, _, _)| {
            sub.tracks
                .get(topic)
                .is_some_and(|track| track.is_retired(stream))
        });
        if stale {
            debug!(sender = %sender, "Dropping a subscription reply naming a retired stream");
            return out;
        }
        let mode = sub.mode;
        let mut resets = Vec::new();
        let mut lost = Vec::new();
        for (topic, stream, seq, history_lost) in positions {
            // No stream: a topic the provider has not published yet. Nothing
            // is held, and its first update names the stream.
            if stream.is_empty() {
                continue;
            }
            let Some(track) = sub.track(&topic) else {
                continue;
            };
            let reset = track.stream.as_deref().is_some_and(|held| held != stream);
            if reset || track.stream.is_none() {
                track.reset_stream(&stream);
            }
            if mode == SubscriptionMode::Log {
                // A baseline only ever raises the position.
                track.contiguous = track.contiguous.max(seq);
                track.advance();
            }
            if reset {
                resets.push(topic.clone());
            }
            if history_lost {
                lost.push(topic);
            }
        }
        sub.lease = Duration::from_millis(
            payload
                .lease_ms
                .clamp(MIN_SUBSCRIPTION_LEASE_MS, MAX_SUBSCRIPTION_LEASE_MS),
        );
        sub.next_renewal = Some(now + sub.lease / 2);
        sub.renewal_outstanding = false;
        sub.resync_pending = false;
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
        if payload.pattern_full && sub.is_pattern() && !sub.pattern_full_reported {
            sub.pattern_full_reported = true;
            out.events.push(consumer_changed(
                &payload.sub_id,
                sub,
                SubscriptionState::Active,
                "pattern_full",
            ));
        }
        for (topics, reason) in [(resets, "stream_reset"), (lost, "history_lost")] {
            for topic in topics {
                out.events.push(changed(
                    &payload.sub_id,
                    SubscriptionRole::Consumer,
                    &sub.service_id,
                    &topic,
                    &sub.provider,
                    SubscriptionState::Active,
                    reason,
                ));
            }
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

        // An exact subscription's updates are for its topic; a pattern's name
        // theirs, which must be one the pattern covers.
        let topic = if sub.is_pattern() {
            match payload.topic.as_deref() {
                Some(topic) => topic.to_string(),
                None => {
                    warn!("Dropping a pattern subscription update that names no topic");
                    return out;
                }
            }
        } else {
            sub.topic.clone()
        };
        if let Some(key) = payload.key.as_deref() {
            if validate_key(key).is_err() {
                warn!("Dropping an update whose key no provider may publish");
                return out;
            }
            // The provider applies the filter. One that does not is not
            // believed: the application asked not to be sent this.
            if !key_matches(&sub.filter, key) {
                debug!("Dropping an update for a key outside the subscription's filter");
                return out;
            }
        }
        let mode = sub.mode;
        let state = sub.state;
        let Some(track) = sub.track(&topic) else {
            debug!("Dropping an update for a topic the subscription does not cover");
            return out;
        };
        if track.is_retired(&payload.stream) {
            debug!("Dropping an update for a stream this subscription has left");
            return out;
        }
        let mut reset = false;
        let mut gap_given_up = false;
        match track.stream.as_deref() {
            None => track.reset_stream(&payload.stream),
            Some(stream) if stream != payload.stream => {
                track.reset_stream(&payload.stream);
                reset = true;
            }
            Some(_) => {}
        }

        let fresh = match mode {
            SubscriptionMode::Latest => {
                let Some(key) = payload.key.as_deref() else {
                    warn!("Dropping a `latest` update with no key");
                    return out;
                };
                match track.keys.get(key) {
                    Some(held) if *held >= payload.seq => false,
                    None if track.keys.len() >= MAX_CONSUMER_TRACKED_KEYS => {
                        warn!("Dropping an update for a key beyond what one subscription tracks");
                        false
                    }
                    _ => {
                        track.keys.insert(key.to_string(), payload.seq);
                        true
                    }
                }
            }
            SubscriptionMode::Log => {
                if payload.snapshot {
                    // A checkpoint: everything up to its seq is summarised.
                    // Stale only once this device is past it.
                    if track.contiguous > payload.seq {
                        false
                    } else {
                        track.contiguous = payload.seq;
                        track.advance();
                        true
                    }
                } else if track.contiguous >= payload.seq || track.seen_ahead.contains(&payload.seq)
                {
                    false
                } else {
                    // Held above the position until the gap below fills, by
                    // the next seq in turn or by a reply's baseline that
                    // overtook it. Delivered now, so a `log` topic arrives in
                    // arrival order, not seq order.
                    track.seen_ahead.insert(payload.seq);
                    if track.seen_ahead.len() > MAX_LOG_SEEN_AHEAD {
                        // The gap below will not fill in time. Give it up
                        // rather than remember without bound. The position
                        // moves past it, so no renewal asks for it again: the
                        // application is told that history was lost, never
                        // left with a silent hole in a log that promises
                        // every event.
                        if let Some(lowest) = track.seen_ahead.pop_first() {
                            track.contiguous = lowest;
                            gap_given_up = true;
                        }
                    }
                    track.advance();
                    true
                }
            }
        };
        if reset {
            out.events.push(changed(
                &payload.sub_id,
                SubscriptionRole::Consumer,
                &sub.service_id,
                &topic,
                &sub.provider,
                state,
                "stream_reset",
            ));
        }
        if gap_given_up {
            out.events.push(changed(
                &payload.sub_id,
                SubscriptionRole::Consumer,
                &sub.service_id,
                &topic,
                &sub.provider,
                state,
                "history_lost",
            ));
        }
        if !fresh {
            return out;
        }
        out.events.push(ServiceEvent::ServiceUpdateReceived {
            sub_id: payload.sub_id,
            service_id: sub.service_id.clone(),
            topic,
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
                    if let Ok(frame) = subscribe_frame(sub_id, sub, sub.resume()) {
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
            if let Ok(frame) = subscribe_frame(sub_id, sub, sub.resume()) {
                out.frames.push(frame);
            }
        }
        out
    }
}

/// `__SVC_SUB__` for `sub`: a subscribe without `resume`, a renewal with it,
/// and `resync` while one is unanswered. One waits per subscription.
fn subscribe_frame(
    sub_id: &str,
    sub: &Sub,
    resume: Option<ResumePosition>,
) -> Result<SubscriptionFrame, ServiceError> {
    let payload = SubscribePayload {
        v: SUBSCRIPTION_WIRE_VERSION,
        sub_id: sub_id.to_string(),
        service_id: sub.service_id.clone(),
        topic: sub.topic.clone(),
        mode: sub.mode,
        params: sub.params.clone(),
        keys: sub.filter.clone(),
        lease_ms: sub.lease.as_millis() as u64,
        resume,
        resync: sub.resync_pending,
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
