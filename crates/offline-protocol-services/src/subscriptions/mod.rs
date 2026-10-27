//! Service subscriptions (ADR 0035): a consumer holds a lease on one topic of
//! one provider's service, and the provider sends it updates as they happen.
//!
//! Like the rest of this crate, nothing here performs I/O. Calls and incoming
//! frames return [`SubscriptionOutput`]: frames for the engine to seal and
//! send, each tagged with the delivery it needs, and events for the
//! application. Time is passed in, so the lease logic is tested without a
//! clock.
//!
//! The invariants the two halves keep, each with the failure it prevents, are
//! in the ADR. The ones enforced here:
//!
//! - a consumer never surfaces an older value after a newer one for the same
//!   subscription and key ([`consumer`](crate::subscriptions::consumer)),
//! - a lease that is not renewed ends the subscription on both sides,
//! - a provider at capacity refuses rather than evicts ([`provider`](crate::subscriptions::provider)),
//! - one subscription per (consumer, service, topic) on a provider,
//! - a consumer that missed updates gets the state back, or an event saying
//!   it cannot.
//!
//! The one this module cannot enforce is that every frame is sealed: that is
//! the engine's, which sends through a path with no plaintext fallback and
//! drops a subscription frame that arrives in plaintext.

/// The consumer half: subscriptions this device holds on other devices.
pub mod consumer;
/// The provider half: subscriptions other devices hold on this one.
pub mod provider;

use serde::Serialize;

use crate::error::ServiceError;
use crate::events::ServiceEvent;
use crate::payloads::{
    MAX_SERVICE_PAYLOAD_SIZE, MAX_SUBSCRIPTION_KEY_LEN, MAX_SUBSCRIPTION_REASON_LEN,
    MAX_SUBSCRIPTION_TOPIC_LEN,
};

pub use consumer::{Consumer, SubscriptionInfo};
pub use provider::{Provider, PublishOutcome, SubscriberInfo};

/// How the engine must send one subscription frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionDelivery {
    /// Sealed, on the reliable ladder, every one.
    Reliable,
    /// Sealed, on the reliable ladder, withdrawing any frame still queued to
    /// the same recipient under the same key first: at most one waits.
    Superseding(String),
    /// Sealed and ephemeral over a live direct link when there is one, which
    /// also withdraws a frame queued under the key; otherwise
    /// [`Self::Superseding`] with the key.
    LiveOrSuperseding(String),
}

/// One frame for the engine to seal and send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionFrame {
    /// The peer it goes to.
    pub recipient: String,
    /// The plaintext, prefix included. Sealed by the engine, never sent as is.
    pub content: String,
    /// How it is sent.
    pub delivery: SubscriptionDelivery,
}

/// What a call, a frame or a tick produced.
#[derive(Debug, Default)]
pub struct SubscriptionOutput {
    /// Frames to seal and send, in order.
    pub frames: Vec<SubscriptionFrame>,
    /// Events for the application, in order.
    pub events: Vec<ServiceEvent>,
}

impl SubscriptionOutput {
    /// Appends `other`'s frames and events after this one's.
    pub fn extend(&mut self, other: SubscriptionOutput) {
        self.frames.extend(other.frames);
        self.events.extend(other.events);
    }
}

/// Which side of a subscription this device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionRole {
    /// This device subscribed to another.
    Consumer,
    /// Another device subscribed to this one.
    Provider,
}

impl SubscriptionRole {
    /// The event spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Consumer => "consumer",
            Self::Provider => "provider",
        }
    }
}

/// Where a subscription stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionState {
    /// Waiting for a session, a reply, or the provider's application.
    Pending,
    /// Accepted and within its lease.
    Active,
    /// Consumer side: renewals have gone unanswered; retried on rediscovery.
    Paused,
    /// Over. Nothing more is sent or accepted for it.
    Ended,
}

impl SubscriptionState {
    /// The event spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Ended => "ended",
        }
    }
}

/// Who may subscribe to a service. Set per service; [`Self::Restricted`]
/// unless the application chooses otherwise, because who can watch a device
/// should be a decision rather than a default.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SubscriptionPolicy {
    /// The application is asked about every new subscription.
    #[default]
    Restricted,
    /// Every peer with a confirmed session is accepted.
    AcceptAll,
    /// Peers in `allow_peers`, or members of a group in `allow_groups`, are
    /// accepted; anyone else is handled by `fallback`.
    Rules {
        /// Subscriber addresses accepted outright.
        allow_peers: Vec<String>,
        /// Group ids whose members are accepted outright.
        allow_groups: Vec<String>,
        /// What happens to a subscriber no rule matches.
        fallback: RulesFallback,
    },
}

/// What [`SubscriptionPolicy::Rules`] does with a subscriber no rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulesFallback {
    /// Ask the application.
    Ask,
    /// Refuse.
    Reject,
}

/// The policy's verdict on one subscriber, worked out by the engine, which is
/// the side that knows group membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// Accept without asking.
    Accept,
    /// Hold the subscription and ask the application.
    Ask,
    /// Refuse.
    Reject,
}

/// FNV-1a over `(key, seq)` pairs, which the caller supplies in key order.
/// Both sides compute it the same way, so a renewal can say "this is what I
/// hold" in eight bytes and the provider can tell whether anything is missing.
pub(crate) fn digest<'a>(pairs: impl Iterator<Item = (&'a str, u64)>) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    for (key, seq) in pairs {
        feed(key.as_bytes());
        feed(&[0]);
        feed(&seq.to_be_bytes());
    }
    hash
}

/// A topic name: 1 to [`MAX_SUBSCRIPTION_TOPIC_LEN`] bytes of
/// `A-Z a-z 0-9 . _ - /`.
pub(crate) fn validate_topic(topic: &str) -> Result<(), ServiceError> {
    let ok = !topic.is_empty()
        && topic.len() <= MAX_SUBSCRIPTION_TOPIC_LEN
        && topic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'));
    if ok {
        Ok(())
    } else {
        Err(ServiceError::InvalidArgument(format!(
            "topic must be 1 to {MAX_SUBSCRIPTION_TOPIC_LEN} bytes of A-Z a-z 0-9 . _ - /"
        )))
    }
}

/// A key of a `latest` topic: 1 to [`MAX_SUBSCRIPTION_KEY_LEN`] bytes.
pub(crate) fn validate_key(key: &str) -> Result<(), ServiceError> {
    if key.is_empty() || key.len() > MAX_SUBSCRIPTION_KEY_LEN {
        return Err(ServiceError::InvalidArgument(format!(
            "key must be 1 to {MAX_SUBSCRIPTION_KEY_LEN} bytes"
        )));
    }
    Ok(())
}

/// A reason string cut to [`MAX_SUBSCRIPTION_REASON_LEN`] bytes on a
/// character boundary.
pub(crate) fn clamp_reason(reason: &str) -> String {
    if reason.len() <= MAX_SUBSCRIPTION_REASON_LEN {
        return reason.to_string();
    }
    let mut end = MAX_SUBSCRIPTION_REASON_LEN;
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_string()
}

/// `prefix` followed by the payload's JSON.
pub(crate) fn encode<T: Serialize>(prefix: &str, payload: &T) -> Result<String, ServiceError> {
    let json =
        serde_json::to_string(payload).map_err(|e| ServiceError::Serialization(e.to_string()))?;
    Ok(format!("{prefix}{json}"))
}

/// The JSON after `prefix`, if `content` starts with it and is within the
/// service payload bound. A frame over the bound is dropped unparsed, as every
/// service frame is.
pub(crate) fn decode<T: serde::de::DeserializeOwned>(content: &str, prefix: &str) -> Option<T> {
    let data = content.strip_prefix(prefix)?;
    if data.len() > MAX_SERVICE_PAYLOAD_SIZE {
        tracing::warn!(size = data.len(), "Dropping oversized subscription frame");
        return None;
    }
    match serde_json::from_str(data) {
        Ok(payload) => Some(payload),
        Err(err) => {
            tracing::warn!(error = %err, "Dropping malformed subscription frame");
            None
        }
    }
}

/// The service a `__SVC_SUB__` names, so that the engine can work out the
/// service's registration and the subscriber's admission before handing the
/// frame to [`Provider::handle_subscribe`].
pub fn requested_service(content: &str) -> Option<String> {
    decode::<crate::payloads::SubscribePayload>(content, crate::payloads::SVC_SUBSCRIBE)
        .map(|payload| payload.service_id)
}

/// The event every state change is reported with.
pub(crate) fn changed(
    sub_id: &str,
    role: SubscriptionRole,
    service_id: &str,
    topic: &str,
    peer: &str,
    state: SubscriptionState,
    reason: &str,
) -> ServiceEvent {
    ServiceEvent::ServiceSubscriptionChanged {
        sub_id: sub_id.to_string(),
        role: role.as_str().to_string(),
        service_id: service_id.to_string(),
        topic: topic.to_string(),
        peer: peer.to_string(),
        state: state.as_str().to_string(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests;
