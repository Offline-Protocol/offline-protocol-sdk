//! A consumer and a provider exchanging frames directly, with the engine's
//! sealing and transport left out: what each side does with what the other
//! sends.

use offline_protocol_core::time::{Duration, Instant};

use super::*;
use crate::error::ServiceError;
use crate::payloads::{
    is_subscription_frame, SubscribeStatus, SubscriptionMode, MAX_PATTERN_TOPICS, SVC_SUBSCRIBE,
    SVC_SUBSCRIBE_REPLY, SVC_UNSUBSCRIBE, SVC_UPDATE,
};

const CONSUMER: &str = "alice";
const PROVIDER: &str = "bob";
const SVC: &str = "fleet.v1";

struct Link {
    consumer: Consumer,
    provider: Provider,
    admission: Admission,
    now: Instant,
    events: Vec<ServiceEvent>,
    /// When set, frames from the provider are dropped instead of delivered.
    drop_provider_frames: bool,
}

impl Link {
    fn new(admission: Admission) -> Self {
        Self {
            consumer: Consumer::new(
                Duration::from_secs(60),
                Duration::from_secs(120),
                Duration::from_secs(600),
            ),
            provider: Provider::new(Duration::from_millis(50), Duration::from_secs(60)),
            admission,
            now: Instant::now(),
            events: Vec::new(),
            drop_provider_frames: false,
        }
    }

    /// Delivers `out`'s frames to whichever side they are for, and whatever
    /// those produce, until nothing is left in flight.
    fn pump(&mut self, out: SubscriptionOutput) {
        let mut queue: Vec<SubscriptionFrame> = out.frames;
        self.events.extend(out.events);
        while !queue.is_empty() {
            let frame = queue.remove(0);
            assert!(is_subscription_frame(&frame.content));
            let produced = if frame.recipient == PROVIDER {
                self.to_provider(&frame.content)
            } else {
                if self.drop_provider_frames {
                    continue;
                }
                self.to_consumer(&frame.content)
            };
            self.events.extend(produced.events);
            queue.extend(produced.frames);
        }
    }

    fn to_provider(&mut self, content: &str) -> SubscriptionOutput {
        let admission = self.admission;
        if content.starts_with(SVC_SUBSCRIBE) {
            self.provider
                .handle_subscribe(content, CONSUMER, self.now, &|id| id == SVC, &|_| admission)
        } else if content.starts_with(SVC_UNSUBSCRIBE) {
            self.provider
                .handle_unsubscribe(content, CONSUMER)
                .unwrap_or_default()
        } else {
            panic!("unexpected frame for the provider: {content}")
        }
    }

    fn to_consumer(&mut self, content: &str) -> SubscriptionOutput {
        if content.starts_with(SVC_SUBSCRIBE_REPLY) {
            self.consumer.handle_reply(content, PROVIDER, self.now)
        } else if content.starts_with(SVC_UPDATE) {
            self.consumer.handle_update(content, PROVIDER)
        } else if content.starts_with(SVC_UNSUBSCRIBE) {
            self.consumer
                .handle_unsubscribe(content, PROVIDER)
                .unwrap_or_default()
        } else {
            panic!("unexpected frame for the consumer: {content}")
        }
    }

    fn subscribe(&mut self, topic: &str, mode: SubscriptionMode) -> String {
        self.subscribe_with(topic, SubscribeOptions::new(mode))
    }

    fn subscribe_with(&mut self, topic: &str, options: SubscribeOptions) -> String {
        let (sub_id, frame) = self
            .consumer
            .subscribe(PROVIDER, SVC, topic, &options, self.now)
            .unwrap();
        self.consumer.mark_sent(&sub_id);
        self.pump(SubscriptionOutput {
            frames: vec![frame],
            events: Vec::new(),
        });
        sub_id
    }

    fn publish(&mut self, topic: &str, key: Option<&str>, body: &str) {
        let (out, _) = self
            .provider
            .publish(SVC, topic, key, body, self.now)
            .unwrap();
        self.pump(out);
    }

    fn advance(&mut self, by: Duration) {
        self.now += by;
        let out = self.consumer.tick(self.now);
        self.pump(out);
        let out = self.provider.tick(self.now);
        self.pump(out);
    }

    /// `(key, body, snapshot)` of every update delivered, in order, then
    /// forgotten.
    fn take_updates(&mut self) -> Vec<(Option<String>, String, bool)> {
        let mut updates = Vec::new();
        self.events.retain(|event| match event {
            ServiceEvent::ServiceUpdateReceived {
                key,
                body,
                snapshot,
                ..
            } => {
                updates.push((key.clone(), body.clone(), *snapshot));
                false
            }
            _ => true,
        });
        updates
    }

    /// `(topic, key, body)` of every update delivered, in order, then
    /// forgotten.
    fn take_topic_updates(&mut self) -> Vec<(String, Option<String>, String)> {
        let mut updates = Vec::new();
        self.events.retain(|event| match event {
            ServiceEvent::ServiceUpdateReceived {
                topic, key, body, ..
            } => {
                updates.push((topic.clone(), key.clone(), body.clone()));
                false
            }
            _ => true,
        });
        updates
    }

    /// `(role, state, reason)` of every state change, in order, then
    /// forgotten.
    fn take_changes(&mut self) -> Vec<(String, String, String)> {
        let mut changes = Vec::new();
        self.events.retain(|event| match event {
            ServiceEvent::ServiceSubscriptionChanged {
                role,
                state,
                reason,
                ..
            } => {
                changes.push((role.clone(), state.clone(), reason.clone()));
                false
            }
            _ => true,
        });
        changes
    }

    fn consumer_state(&self, sub_id: &str) -> Option<SubscriptionState> {
        self.consumer
            .subscriptions()
            .into_iter()
            .find(|info| info.sub_id == sub_id)
            .map(|info| info.state)
    }
}

fn update(sub_id: &str, stream: &str, seq: u64, key: Option<&str>, body: &str) -> String {
    super::encode(
        SVC_UPDATE,
        &crate::payloads::UpdatePayload {
            v: 1,
            sub_id: sub_id.to_string(),
            topic: None,
            stream: stream.to_string(),
            seq,
            key: key.map(str::to_string),
            body: body.to_string(),
            snapshot: false,
        },
    )
    .unwrap()
}

#[test]
fn latest_end_to_end_with_snapshot_then_live_updates() {
    let mut link = Link::new(Admission::Accept);
    link.publish("battery", Some("pct"), "81");
    let sub_id = link.subscribe("battery", SubscriptionMode::Latest);
    assert_eq!(
        link.consumer_state(&sub_id),
        Some(SubscriptionState::Active)
    );
    assert_eq!(
        link.take_updates(),
        vec![(Some("pct".to_string()), "81".to_string(), true)]
    );
    link.advance(Duration::from_secs(1));
    link.publish("battery", Some("pct"), "80");
    assert_eq!(
        link.take_updates(),
        vec![(Some("pct".to_string()), "80".to_string(), false)]
    );
}

#[test]
fn restricted_waits_for_the_application() {
    let mut link = Link::new(Admission::Ask);
    let sub_id = link.subscribe("battery", SubscriptionMode::Latest);
    assert_eq!(
        link.consumer_state(&sub_id),
        Some(SubscriptionState::Pending)
    );
    let out = link.provider.accept(&sub_id, link.now).unwrap();
    link.pump(out);
    assert_eq!(
        link.consumer_state(&sub_id),
        Some(SubscriptionState::Active)
    );
}

#[test]
fn a_rejection_ends_the_subscription_with_its_reason() {
    let mut link = Link::new(Admission::Ask);
    let sub_id = link.subscribe("battery", SubscriptionMode::Latest);
    let out = link.provider.reject(&sub_id, "not you").unwrap();
    link.pump(out);
    assert!(!link.consumer.holds(&sub_id));
    assert!(link
        .take_changes()
        .contains(&("consumer".into(), "ended".into(), "not you".into())));
}

/// Invariant 4: an older value that arrives after a newer one is dropped.
#[test]
fn an_older_value_never_overtakes_a_newer_one() {
    let mut link = Link::new(Admission::Accept);
    link.publish("pos", Some("xy"), "0,0");
    let sub_id = link.subscribe("pos", SubscriptionMode::Latest);
    link.take_updates();
    link.advance(Duration::from_secs(1));
    let (out, _) = link
        .provider
        .publish(SVC, "pos", Some("xy"), "2,2", link.now)
        .unwrap();
    // The newer frame, held back so that an older one can be sent after it.
    let newer = out.frames[0].content.clone();
    let sent: crate::payloads::UpdatePayload = super::decode(&newer, SVC_UPDATE).unwrap();
    let older = update(&sub_id, &sent.stream, sent.seq - 1, Some("xy"), "1,1");
    link.pump(SubscriptionOutput {
        frames: vec![
            SubscriptionFrame {
                recipient: CONSUMER.into(),
                content: newer,
                delivery: SubscriptionDelivery::Reliable,
            },
            SubscriptionFrame {
                recipient: CONSUMER.into(),
                content: older,
                delivery: SubscriptionDelivery::Reliable,
            },
        ],
        events: Vec::new(),
    });
    assert_eq!(
        link.take_updates(),
        vec![(Some("xy".to_string()), "2,2".to_string(), false)]
    );
}

/// Invariant 2: an update from anyone but the subscription's provider is
/// dropped, whatever `sub_id` it names.
#[test]
fn an_update_from_another_peer_is_dropped() {
    let mut link = Link::new(Admission::Accept);
    let sub_id = link.subscribe("pos", SubscriptionMode::Latest);
    let forged = update(&sub_id, "any", 99, Some("xy"), "forged");
    let out = link.consumer.handle_update(&forged, "mallory");
    assert!(out.events.is_empty());
}

#[test]
fn renewals_keep_a_subscription_alive_and_unanswered_ones_pause_it() {
    let mut link = Link::new(Admission::Accept);
    let sub_id = link.subscribe("pos", SubscriptionMode::Latest);
    link.take_changes();
    // Renewed at half the 60 s lease, answered: the provider's lease is
    // extended and nothing changes on either side.
    for _ in 0..4 {
        link.advance(Duration::from_secs(31));
    }
    assert!(link.provider.holds(&sub_id));
    assert_eq!(
        link.consumer_state(&sub_id),
        Some(SubscriptionState::Active)
    );
    assert!(link.take_changes().is_empty());

    // The provider goes quiet.
    link.drop_provider_frames = true;
    for _ in 0..4 {
        link.advance(Duration::from_secs(31));
    }
    assert_eq!(
        link.consumer_state(&sub_id),
        Some(SubscriptionState::Paused)
    );
    assert!(link.take_changes().contains(&(
        "consumer".into(),
        "paused".into(),
        "renewal_unanswered".into()
    )));

    // Seen again: renewed at once, and resumed.
    link.drop_provider_frames = false;
    let out = link.consumer.on_peer_seen(PROVIDER);
    link.pump(out);
    assert_eq!(
        link.consumer_state(&sub_id),
        Some(SubscriptionState::Active)
    );
    assert!(link
        .take_changes()
        .contains(&("consumer".into(), "active".into(), "resumed".into())));
}

#[test]
fn a_subscription_paused_too_long_ends() {
    let mut link = Link::new(Admission::Accept);
    let sub_id = link.subscribe("pos", SubscriptionMode::Latest);
    link.drop_provider_frames = true;
    for _ in 0..30 {
        link.advance(Duration::from_secs(31));
    }
    assert!(!link.consumer.holds(&sub_id));
    assert!(link.take_changes().contains(&(
        "consumer".into(),
        "ended".into(),
        "provider_unreachable".into()
    )));
}

#[test]
fn a_pending_subscription_times_out_with_the_reason_it_waited_for() {
    let mut link = Link::new(Admission::Accept);
    let now = link.now;
    let (never_sent, _) = link
        .consumer
        .subscribe(
            PROVIDER,
            SVC,
            "a",
            &SubscribeOptions::new(SubscriptionMode::Log),
            now,
        )
        .unwrap();
    let (unanswered, _) = link
        .consumer
        .subscribe(
            PROVIDER,
            SVC,
            "b",
            &SubscribeOptions::new(SubscriptionMode::Log),
            now,
        )
        .unwrap();
    link.consumer.mark_sent(&unanswered);
    assert_eq!(link.consumer.frames_awaiting_session(PROVIDER).len(), 1);
    let out = link.consumer.tick(now + Duration::from_secs(121));
    let reasons: Vec<String> = out
        .events
        .iter()
        .filter_map(|event| match event {
            ServiceEvent::ServiceSubscriptionChanged { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert!(reasons.contains(&"session_timeout".to_string()));
    assert!(reasons.contains(&"no_reply".to_string()));
    assert!(!link.consumer.holds(&never_sent) && !link.consumer.holds(&unanswered));
}

/// Invariant 8, first path: a `latest` value lost on the way is recovered by
/// the next renewal, whose digest no longer matches the provider's.
#[test]
fn a_lost_latest_value_comes_back_at_the_next_renewal() {
    let mut link = Link::new(Admission::Accept);
    link.subscribe("pos", SubscriptionMode::Latest);
    link.take_updates();
    link.advance(Duration::from_secs(1));
    link.drop_provider_frames = true;
    link.publish("pos", Some("xy"), "lost");
    link.drop_provider_frames = false;
    assert!(link.take_updates().is_empty());
    link.advance(Duration::from_secs(30));
    assert_eq!(
        link.take_updates(),
        vec![(Some("xy".to_string()), "lost".to_string(), true)]
    );
}

/// Invariant 8, second path: a provider that starts over is noticed, and the
/// consumer is told before it takes the new state.
#[test]
fn a_cleared_topic_resets_the_consumer() {
    let mut link = Link::new(Admission::Accept);
    link.publish("pos", Some("xy"), "old");
    link.subscribe("pos", SubscriptionMode::Latest);
    link.take_updates();
    link.take_changes();
    let out = link.provider.clear_topic(SVC, "pos").unwrap();
    link.pump(out);
    assert!(link.take_changes().contains(&(
        "consumer".into(),
        "active".into(),
        "stream_reset".into()
    )));
    // Seq 1 again under the new stream is accepted, though seq 1 was held.
    link.advance(Duration::from_secs(1));
    link.publish("pos", Some("xy"), "new");
    assert_eq!(
        link.take_updates(),
        vec![(Some("xy".to_string()), "new".to_string(), false)]
    );
}

/// Invariant 8, third path: asked for, the state comes back in full, values
/// already delivered included.
#[test]
fn a_resync_delivers_the_whole_state_again() {
    let mut link = Link::new(Admission::Accept);
    link.publish("pos", Some("a"), "1");
    link.publish("pos", Some("b"), "2");
    let sub_id = link.subscribe("pos", SubscriptionMode::Latest);
    assert_eq!(link.take_updates().len(), 2);
    let frame = link.consumer.resync(&sub_id).unwrap();
    link.pump(SubscriptionOutput {
        frames: vec![frame],
        events: Vec::new(),
    });
    let again = link.take_updates();
    assert_eq!(again.len(), 2);
    assert!(again.iter().all(|(_, _, snapshot)| *snapshot));
}

#[test]
fn a_log_resync_starts_from_the_checkpoint() {
    let mut link = Link::new(Admission::Accept);
    let sub_id = link.subscribe("ev", SubscriptionMode::Log);
    link.publish("ev", None, "e1");
    link.provider.set_checkpoint(SVC, "ev", "cp1").unwrap();
    link.publish("ev", None, "e2");
    assert_eq!(link.take_updates().len(), 2);
    let frame = link.consumer.resync(&sub_id).unwrap();
    link.pump(SubscriptionOutput {
        frames: vec![frame],
        events: Vec::new(),
    });
    let bodies: Vec<String> = link.take_updates().into_iter().map(|(_, b, _)| b).collect();
    assert_eq!(bodies, vec!["cp1", "e2"]);
}

#[test]
fn a_log_consumer_that_was_away_is_replayed_what_it_missed() {
    let mut link = Link::new(Admission::Accept);
    link.subscribe("ev", SubscriptionMode::Log);
    link.drop_provider_frames = true;
    link.publish("ev", None, "e1");
    link.publish("ev", None, "e2");
    link.drop_provider_frames = false;
    link.advance(Duration::from_secs(31));
    let bodies: Vec<String> = link.take_updates().into_iter().map(|(_, b, _)| b).collect();
    assert_eq!(bodies, vec!["e1", "e2"]);
    // Delivered once: the next renewal replays nothing.
    link.advance(Duration::from_secs(31));
    assert!(link.take_updates().is_empty());
}

#[test]
fn a_log_consumer_past_the_buffer_is_told_history_was_lost() {
    let mut link = Link::new(Admission::Accept);
    link.subscribe("ev", SubscriptionMode::Log);
    link.drop_provider_frames = true;
    for i in 0..(crate::payloads::MAX_LOG_ENTRIES + 5) {
        link.publish("ev", None, &format!("e{i}"));
    }
    link.drop_provider_frames = false;
    link.take_changes();
    link.advance(Duration::from_secs(31));
    assert!(link.take_changes().contains(&(
        "consumer".into(),
        "active".into(),
        "history_lost".into()
    )));
}

#[test]
fn unsubscribing_ends_both_sides() {
    let mut link = Link::new(Admission::Accept);
    let sub_id = link.subscribe("pos", SubscriptionMode::Latest);
    let out = link.consumer.unsubscribe(&sub_id).unwrap();
    link.pump(out);
    assert!(!link.consumer.holds(&sub_id));
    assert!(!link.provider.holds(&sub_id));
}

#[test]
fn a_provider_revocation_ends_the_consumer_side() {
    let mut link = Link::new(Admission::Accept);
    let sub_id = link.subscribe("pos", SubscriptionMode::Latest);
    let out = link.provider.end(&sub_id, "").unwrap();
    link.pump(out);
    assert!(!link.consumer.holds(&sub_id));
    assert!(link
        .take_changes()
        .contains(&("consumer".into(), "ended".into(), "revoked".into())));
}

#[test]
fn one_subscription_per_topic_on_the_consumer() {
    let mut link = Link::new(Admission::Accept);
    link.subscribe("pos", SubscriptionMode::Latest);
    assert!(link
        .consumer
        .subscribe(
            PROVIDER,
            SVC,
            "pos",
            &SubscribeOptions::new(SubscriptionMode::Latest),
            link.now
        )
        .is_err());
}

// ---------------------------------------------------------------------------
// Key filters
// ---------------------------------------------------------------------------

fn keyed(keys: &[&str]) -> SubscribeOptions {
    SubscribeOptions {
        keys: keys.iter().map(|key| key.to_string()).collect(),
        ..SubscribeOptions::new(SubscriptionMode::Latest)
    }
}

#[test]
fn a_key_filter_holds_back_every_other_key_in_the_snapshot_and_live() {
    let mut link = Link::new(Admission::Accept);
    link.publish("car", Some("battery"), "80");
    link.publish("car", Some("temp"), "21");
    link.publish("car", Some("pos.lat"), "51.5");
    link.subscribe_with("car", keyed(&["battery", "pos.*"]));
    let mut snapshot: Vec<_> = link
        .take_updates()
        .into_iter()
        .map(|(key, body, _)| (key.unwrap(), body))
        .collect();
    snapshot.sort();
    assert_eq!(
        snapshot,
        vec![
            ("battery".to_string(), "80".to_string()),
            ("pos.lat".to_string(), "51.5".to_string())
        ]
    );

    link.now += Duration::from_secs(1);
    let (out, outcome) = link
        .provider
        .publish(SVC, "car", Some("temp"), "22", link.now)
        .unwrap();
    assert_eq!(outcome.subscribers, 0, "a filtered key reaches nobody");
    assert!(out.frames.is_empty());
    link.publish("car", Some("pos.lon"), "-0.1");
    assert_eq!(
        link.take_updates(),
        vec![(Some("pos.lon".into()), "-0.1".into(), false)]
    );
}

/// The digest a renewal carries covers the filtered keys on both sides, so
/// a caught-up filtered subscriber is not sent a snapshot at every renewal.
#[test]
fn a_filtered_subscriber_that_holds_everything_is_not_resent_it() {
    let mut link = Link::new(Admission::Accept);
    link.publish("car", Some("battery"), "80");
    link.publish("car", Some("temp"), "21");
    link.subscribe_with("car", keyed(&["battery"]));
    link.take_updates();
    // The consumer would drop a resent value it holds, so the frames are
    // what show whether one was sent.
    link.now += Duration::from_secs(31);
    let renewal = link.consumer.tick(link.now);
    assert_eq!(renewal.frames.len(), 1);
    let answer = link.to_provider(&renewal.frames[0].content);
    assert_eq!(answer.frames.len(), 1, "the reply alone");
    assert!(answer.frames[0].content.starts_with(SVC_SUBSCRIBE_REPLY));
}

#[test]
fn a_key_filter_on_a_log_subscription_is_refused_on_both_sides() {
    let mut link = Link::new(Admission::Accept);
    let options = SubscribeOptions {
        keys: vec!["x".into()],
        ..SubscribeOptions::new(SubscriptionMode::Log)
    };
    assert!(matches!(
        link.consumer
            .subscribe(PROVIDER, SVC, "ev", &options, link.now),
        Err(ServiceError::InvalidArgument(_))
    ));

    let frame = super::encode(
        SVC_SUBSCRIBE,
        &crate::payloads::SubscribePayload {
            v: 1,
            sub_id: "s1".into(),
            service_id: SVC.into(),
            topic: "ev".into(),
            mode: SubscriptionMode::Log,
            params: String::new(),
            keys: vec!["x".into()],
            lease_ms: 60_000,
            resume: None,
            resync: false,
        },
    )
    .unwrap();
    let out = link.to_provider(&frame);
    assert!(!link.provider.holds("s1"));
    assert!(out.frames[0].content.contains("invalid_subscription"));
}

#[test]
fn a_consumer_drops_a_key_its_filter_excludes_even_from_its_provider() {
    let mut link = Link::new(Admission::Accept);
    let sub_id = link.subscribe_with("car", keyed(&["battery"]));
    // Checked before anything else about the update, its stream included.
    let out = link.to_consumer(&update(&sub_id, "any", 1, Some("temp"), "21"));
    assert!(out.events.is_empty());
}

// ---------------------------------------------------------------------------
// Topic patterns
// ---------------------------------------------------------------------------

#[test]
fn a_pattern_snapshots_every_matching_topic_and_follows_new_ones() {
    let mut link = Link::new(Admission::Accept);
    link.publish("sensors/a", Some("v"), "1");
    link.publish("other/b", Some("v"), "2");
    link.publish("sensors/log", None, "an event");
    let sub_id = link.subscribe_with("sensors/*", SubscribeOptions::new(SubscriptionMode::Latest));
    assert_eq!(
        link.take_topic_updates(),
        vec![("sensors/a".into(), Some("v".into()), "1".into())]
    );

    link.now += Duration::from_secs(1);
    link.publish("sensors/c", Some("v"), "3");
    link.now += Duration::from_secs(1);
    link.publish("other/b", Some("v"), "4");
    link.publish("sensors/log", None, "another");
    assert_eq!(
        link.take_topic_updates(),
        vec![("sensors/c".into(), Some("v".into()), "3".into())]
    );
    let info = link
        .consumer
        .subscriptions()
        .into_iter()
        .find(|info| info.sub_id == sub_id)
        .unwrap();
    assert_eq!(info.topic, "sensors/*");
    assert_eq!(
        info.topics,
        vec!["sensors/a".to_string(), "sensors/c".to_string()]
    );
}

/// The first update of a topic created while the subscriber was away is lost
/// with it; the renewal names only the topics it knows, and the provider
/// sends the rest from nothing.
#[test]
fn a_pattern_recovers_a_topic_it_never_heard_of_at_the_next_renewal() {
    let mut link = Link::new(Admission::Accept);
    link.publish("sensors/a", Some("v"), "1");
    link.subscribe_with("sensors/*", SubscribeOptions::new(SubscriptionMode::Latest));
    link.take_topic_updates();

    link.drop_provider_frames = true;
    link.now += Duration::from_secs(1);
    link.publish("sensors/b", Some("v"), "2");
    link.drop_provider_frames = false;
    assert!(link.take_topic_updates().is_empty());

    link.advance(Duration::from_secs(31));
    assert_eq!(
        link.take_topic_updates(),
        vec![("sensors/b".into(), Some("v".into()), "2".into())]
    );
}

#[test]
fn a_log_pattern_replays_each_topic_from_its_own_position() {
    let mut link = Link::new(Admission::Accept);
    link.publish("doors/front", None, "f1");
    link.subscribe_with("doors/*", SubscribeOptions::new(SubscriptionMode::Log));
    assert!(
        link.take_topic_updates().is_empty(),
        "no checkpoint: nothing to catch up"
    );

    link.publish("doors/front", None, "f2");
    link.publish("doors/back", None, "b1");
    assert_eq!(link.take_topic_updates().len(), 2);

    link.drop_provider_frames = true;
    link.publish("doors/front", None, "f3");
    link.publish("doors/back", None, "b2");
    link.publish("doors/side", None, "s1");
    link.drop_provider_frames = false;

    link.advance(Duration::from_secs(31));
    let mut replayed = link.take_topic_updates();
    replayed.sort();
    assert_eq!(
        replayed,
        vec![
            ("doors/back".into(), None, "b2".into()),
            ("doors/front".into(), None, "f3".into()),
            ("doors/side".into(), None, "s1".into()),
        ]
    );
}

#[test]
fn clearing_one_topic_resets_only_that_topic_of_a_pattern() {
    let mut link = Link::new(Admission::Accept);
    link.publish("sensors/a", Some("v"), "1");
    link.publish("sensors/b", Some("v"), "1");
    link.subscribe_with("sensors/*", SubscribeOptions::new(SubscriptionMode::Latest));
    link.take_topic_updates();
    link.take_changes();

    let out = link.provider.clear_topic(SVC, "sensors/a").unwrap();
    link.pump(out);
    let resets: Vec<_> = link
        .events
        .iter()
        .filter_map(|event| match event {
            ServiceEvent::ServiceSubscriptionChanged { topic, reason, .. } => {
                Some((topic.clone(), reason.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(resets, vec![("sensors/a".into(), "stream_reset".into())]);

    // Its next value is delivered although its seq starts over.
    link.events.clear();
    link.now += Duration::from_secs(1);
    link.publish("sensors/a", Some("v"), "fresh");
    assert_eq!(
        link.take_topic_updates(),
        vec![("sensors/a".into(), Some("v".into()), "fresh".into())]
    );
}

#[test]
fn a_pattern_follows_at_most_its_cap_of_topics() {
    let mut link = Link::new(Admission::Accept);
    link.subscribe_with("t/*", SubscribeOptions::new(SubscriptionMode::Log));
    for i in 0..=MAX_PATTERN_TOPICS {
        link.publish(&format!("t/{i:03}"), None, "x");
    }
    let topics: std::collections::BTreeSet<_> = link
        .take_topic_updates()
        .into_iter()
        .map(|(topic, _, _)| topic)
        .collect();
    assert_eq!(topics.len(), MAX_PATTERN_TOPICS);
    assert!(!topics.contains(&format!("t/{MAX_PATTERN_TOPICS:03}")));
}

#[test]
fn a_pattern_update_for_a_topic_it_does_not_cover_is_dropped() {
    let mut link = Link::new(Admission::Accept);
    let sub_id = link.subscribe_with("sensors/*", SubscribeOptions::new(SubscriptionMode::Log));
    let mut frame = crate::payloads::UpdatePayload {
        v: 1,
        sub_id: sub_id.clone(),
        topic: Some("other/x".into()),
        stream: "s".into(),
        seq: 1,
        key: None,
        body: "b".into(),
        snapshot: false,
    };
    let out = link.to_consumer(&super::encode(SVC_UPDATE, &frame).unwrap());
    assert!(out.events.is_empty());
    frame.topic = None;
    let out = link.to_consumer(&super::encode(SVC_UPDATE, &frame).unwrap());
    assert!(out.events.is_empty());
}

#[test]
fn a_pattern_is_refused_where_a_star_is_not_last() {
    let link = Link::new(Admission::Accept);
    let mut consumer = link.consumer;
    for topic in ["a*b", "a/**", "*/x"] {
        assert!(
            consumer
                .subscribe(
                    PROVIDER,
                    SVC,
                    topic,
                    &SubscribeOptions::new(SubscriptionMode::Log),
                    link.now
                )
                .is_err(),
            "{topic}"
        );
    }
    for topic in ["*", "a/*", "a*"] {
        assert!(
            consumer
                .subscribe(
                    PROVIDER,
                    SVC,
                    topic,
                    &SubscribeOptions::new(SubscriptionMode::Log),
                    link.now
                )
                .is_ok(),
            "{topic}"
        );
    }
}

fn subscribe_frame(sub_id: &str, resume: Option<crate::payloads::ResumePosition>) -> String {
    super::encode(
        SVC_SUBSCRIBE,
        &crate::payloads::SubscribePayload {
            v: 1,
            sub_id: sub_id.into(),
            service_id: SVC.into(),
            topic: "car".into(),
            mode: SubscriptionMode::Latest,
            params: String::new(),
            keys: Vec::new(),
            lease_ms: 60_000,
            resume,
            resync: false,
        },
    )
    .unwrap()
}

fn held_nothing() -> Option<crate::payloads::ResumePosition> {
    Some(crate::payloads::ResumePosition {
        stream: String::new(),
        seq: 0,
        digest: 0,
        topics: Vec::new(),
    })
}

/// Invariant 7. After both sides restart, a renewal still in a mailbox
/// under the consumer's old id must not take the place of the subscription
/// it made since; and a replaced id never comes back.
#[test]
fn only_a_first_subscribe_replaces_and_a_replaced_id_never_returns() {
    let mut link = Link::new(Admission::Accept);
    link.publish("car", Some("battery"), "80");
    assert!(!link
        .to_provider(&subscribe_frame("old", None))
        .frames
        .is_empty());
    // The consumer restarted and subscribed again: the first subscribe wins.
    let out = link.to_provider(&subscribe_frame("new", None));
    assert!(link.provider.holds("new") && !link.provider.holds("old"));
    assert!(out.events.iter().any(|event| matches!(
        event,
        ServiceEvent::ServiceSubscriptionChanged { sub_id, reason, .. }
            if sub_id == "old" && reason == "replaced"
    )));

    // The old id, renewing or subscribing, is dropped without a reply.
    for frame in [
        subscribe_frame("old", held_nothing()),
        subscribe_frame("old", None),
    ] {
        let out = link.to_provider(&frame);
        assert!(out.frames.is_empty() && out.events.is_empty());
        assert!(link.provider.holds("new") && !link.provider.holds("old"));
    }
    // So is a renewal under an id the provider never saw (it restarted
    // since that one was made): the triple is held under another.
    let out = link.to_provider(&subscribe_frame("before-restart", held_nothing()));
    assert!(out.frames.is_empty() && out.events.is_empty());
    assert!(link.provider.holds("new") && !link.provider.holds("before-restart"));
}

#[test]
fn a_provider_remembers_a_bounded_number_of_replaced_ids_per_topic() {
    let mut link = Link::new(Admission::Accept);
    let ids: Vec<String> = (0..=crate::payloads::MAX_RETIRED_PER_TOPIC + 1)
        .map(|i| format!("id{i}"))
        .collect();
    for id in &ids {
        link.to_provider(&subscribe_frame(id, None));
    }
    // The newest is held, the next four are retired, the oldest forgotten.
    let out = link.to_provider(&subscribe_frame(&ids[1], None));
    assert!(out.frames.is_empty(), "a retired id is dropped");
    let out = link.to_provider(&subscribe_frame(&ids[0], None));
    assert!(
        !out.frames.is_empty(),
        "a forgotten id is a first subscribe again"
    );
    assert!(link.provider.holds(&ids[0]));
}

/// The reliable ladder keeps no order between entries: a reply resent after
/// a loss lands after the catch-up frames that followed it. The position is
/// never taken from an update, so none of them is mistaken for the baseline.
#[test]
fn catch_up_frames_that_overtake_their_reply_are_all_delivered() {
    let mut link = Link::new(Admission::Accept);
    link.publish("ev", None, "e1");
    link.publish("ev", None, "e2");
    link.provider
        .set_checkpoint(SVC, "ev", "state at 2")
        .unwrap();
    link.publish("ev", None, "e3");
    let (sub_id, frame) = link
        .consumer
        .subscribe(
            PROVIDER,
            SVC,
            "ev",
            &SubscribeOptions::new(SubscriptionMode::Log),
            link.now,
        )
        .unwrap();
    link.consumer.mark_sent(&sub_id);
    let answer = link.to_provider(&frame.content);
    // The reply, the checkpoint at 2, then e3; delivered backwards.
    assert_eq!(answer.frames.len(), 3);
    for frame in answer.frames.iter().rev() {
        let out = link.to_consumer(&frame.content);
        link.events.extend(out.events);
    }
    assert_eq!(
        link.take_updates(),
        vec![
            (None, "e3".into(), false),
            (None, "state at 2".into(), true),
        ]
    );
    link.publish("ev", None, "e4");
    assert_eq!(link.take_updates(), vec![(None, "e4".into(), false)]);
}

/// A frame from before a provider restart can still arrive from a mailbox.
/// Taken, it would reset the consumer to the dead stream, and the live
/// stream's next frame would reset it back.
#[test]
fn a_frame_naming_a_stream_the_consumer_left_is_dropped() {
    let mut link = Link::new(Admission::Accept);
    let sub_id = link.subscribe("car", SubscriptionMode::Latest);
    link.to_consumer(&update(&sub_id, "before", 1, Some("battery"), "70"));
    let out = link.to_consumer(&update(&sub_id, "after", 1, Some("battery"), "80"));
    assert_eq!(out.events.len(), 2, "a stream_reset and the value");

    let out = link.to_consumer(&update(&sub_id, "before", 9, Some("battery"), "stale"));
    assert!(out.events.is_empty());
    let reply = super::encode(
        SVC_SUBSCRIBE_REPLY,
        &crate::payloads::SubscribeReplyPayload {
            v: 1,
            sub_id: sub_id.clone(),
            status: SubscribeStatus::Accepted,
            reason: None,
            lease_ms: 60_000,
            stream: "before".into(),
            seq: 9,
            history_lost: false,
            topics: Vec::new(),
            pattern_full: false,
        },
    )
    .unwrap();
    assert!(link.to_consumer(&reply).events.is_empty());
    let out = link.to_consumer(&update(&sub_id, "after", 2, Some("battery"), "81"));
    assert_eq!(out.events.len(), 1, "still on the live stream, no reset");
}

/// A renewal that falls due while a resync is queued supersedes it. It must
/// carry `resync` itself, or the application's request is withdrawn.
#[test]
fn a_renewal_carries_an_unanswered_resync() {
    let mut link = Link::new(Admission::Accept);
    let sub_id = link.subscribe("car", SubscriptionMode::Latest);
    let resync_of = |content: &str| {
        serde_json::from_str::<crate::payloads::SubscribePayload>(
            content.strip_prefix(SVC_SUBSCRIBE).unwrap(),
        )
        .unwrap()
        .resync
    };
    assert!(resync_of(&link.consumer.resync(&sub_id).unwrap().content));
    link.now += Duration::from_secs(31);
    let renewal = link.consumer.tick(link.now);
    assert!(resync_of(&renewal.frames[0].content));

    link.pump(renewal);
    link.now += Duration::from_secs(31);
    let renewal = link.consumer.tick(link.now);
    assert!(
        !resync_of(&renewal.frames[0].content),
        "answered, so asked no more"
    );
}

#[test]
fn a_full_pattern_is_reported_to_the_consumer_on_a_reply() {
    // Full when it subscribes: the reply that answers it says so.
    let mut link = Link::new(Admission::Accept);
    for i in 0..=MAX_PATTERN_TOPICS {
        link.publish(&format!("t/{i:03}"), Some("k"), "x");
    }
    link.subscribe_with("t/*", SubscribeOptions::new(SubscriptionMode::Latest));
    let changes = link.take_changes();
    for role in ["provider", "consumer"] {
        assert_eq!(
            changes
                .iter()
                .filter(|(r, state, reason)| r == role
                    && state == "active"
                    && reason == "pattern_full")
                .count(),
            1,
            "{role}: {changes:?}"
        );
    }

    // Full later: a reply of its own when the cap is reached, and only once.
    let mut link = Link::new(Admission::Accept);
    link.subscribe_with("t/*", SubscribeOptions::new(SubscriptionMode::Log));
    for i in 0..=MAX_PATTERN_TOPICS + 1 {
        link.publish(&format!("t/{i:03}"), None, "x");
    }
    link.now += Duration::from_secs(31);
    let renewal = link.consumer.tick(link.now);
    link.pump(renewal);
    let changes: Vec<_> = link
        .take_changes()
        .into_iter()
        .filter(|(_, _, reason)| reason == "pattern_full")
        .collect();
    assert_eq!(
        changes,
        vec![
            ("provider".into(), "active".into(), "pattern_full".into()),
            ("consumer".into(), "active".into(), "pattern_full".into()),
        ]
    );
}

/// The zero byte separates a key from its seq in the digest.
#[test]
fn a_key_with_a_zero_byte_is_refused_everywhere() {
    let mut link = Link::new(Admission::Accept);
    assert!(matches!(
        link.provider
            .publish(SVC, "car", Some("a\0b"), "x", link.now),
        Err(ServiceError::InvalidArgument(_))
    ));
    assert!(link
        .consumer
        .subscribe(PROVIDER, SVC, "car", &keyed(&["a\0*"]), link.now)
        .is_err());
    let sub_id = link.subscribe("car", SubscriptionMode::Latest);
    let out = link.to_consumer(&update(&sub_id, "s", 1, Some("a\0b"), "x"));
    assert!(out.events.is_empty());
}

#[test]
fn every_frame_goes_out_as_version_1() {
    let mut link = Link::new(Admission::Accept);
    let (_, frame) = link
        .consumer
        .subscribe(
            PROVIDER,
            SVC,
            "car",
            &SubscribeOptions::new(SubscriptionMode::Latest),
            link.now,
        )
        .unwrap();
    link.provider
        .publish(SVC, "car", Some("k"), "x", link.now)
        .unwrap();
    let answer = link.to_provider(&frame.content);
    for content in std::iter::once(&frame.content).chain(answer.frames.iter().map(|f| &f.content)) {
        assert!(content.contains("\"v\":1"), "{content}");
    }
}

#[test]
fn a_caught_up_pattern_renews_with_nothing_resent() {
    let mut link = Link::new(Admission::Accept);
    link.publish("sensors/a", Some("v"), "1");
    link.publish("sensors/b", Some("v"), "2");
    link.subscribe_with("sensors/*", SubscribeOptions::new(SubscriptionMode::Latest));
    assert_eq!(link.take_topic_updates().len(), 2);
    link.now += Duration::from_secs(31);
    let renewal = link.consumer.tick(link.now);
    let answer = link.to_provider(&renewal.frames[0].content);
    assert_eq!(answer.frames.len(), 1, "the reply alone");
}

#[test]
fn a_log_gap_given_up_on_is_reported_as_history_lost() {
    let mut link = Link::new(Admission::Accept);
    link.subscribe("ev", SubscriptionMode::Log);
    link.publish("ev", None, "e1");
    // e2 is lost in transit, and more than the consumer holds above a gap
    // arrives after it.
    link.drop_provider_frames = true;
    link.publish("ev", None, "e2");
    link.drop_provider_frames = false;
    link.take_changes();
    for i in 0..=crate::payloads::MAX_LOG_SEEN_AHEAD {
        link.publish("ev", None, &format!("after{i}"));
    }
    assert!(
        link.take_changes()
            .contains(&("consumer".into(), "active".into(), "history_lost".into())),
        "a gap the consumer stops waiting for is never a silent hole"
    );
}
