//! A consumer and a provider exchanging frames directly, with the engine's
//! sealing and transport left out: what each side does with what the other
//! sends.

use offline_protocol_core::time::{Duration, Instant};

use super::*;
use crate::payloads::{
    is_subscription_frame, SubscriptionMode, SVC_SUBSCRIBE, SVC_SUBSCRIBE_REPLY, SVC_UNSUBSCRIBE,
    SVC_UPDATE,
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
        let (sub_id, frame) = self
            .consumer
            .subscribe(PROVIDER, SVC, topic, mode, "", self.now)
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
        .subscribe(PROVIDER, SVC, "a", SubscriptionMode::Log, "", now)
        .unwrap();
    let (unanswered, _) = link
        .consumer
        .subscribe(PROVIDER, SVC, "b", SubscriptionMode::Log, "", now)
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
        .subscribe(PROVIDER, SVC, "pos", SubscriptionMode::Latest, "", link.now)
        .is_err());
}
