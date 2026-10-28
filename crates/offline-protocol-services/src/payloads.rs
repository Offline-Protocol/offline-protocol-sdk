use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// TTL for deduplicating service discovery queries (seconds).
pub const DISCOVERY_QUERY_DEDUP_TTL_SECS: u64 = 60;
/// Default maximum hops for service discovery query gossip forwarding.
pub const DISCOVERY_QUERY_DEFAULT_MAX_HOPS: u8 = 10;
/// Maximum number of peers to forward a discovery query to per hop.
pub const DISCOVERY_GOSSIP_MAX_FANOUT: usize = 5;
/// Maximum number of peers the originator broadcasts an initial discovery query to.
pub const DISCOVERY_INITIAL_BROADCAST_MAX: usize = 20;
/// Maximum number of seen discovery query IDs to track for dedup.
pub const DISCOVERY_QUERY_MAX_DEDUP_ENTRIES: usize = 10_000;

/// Maximum allowed size (bytes) for the raw payload string after stripping the prefix.
pub const MAX_SERVICE_PAYLOAD_SIZE: usize = 131_072; // 128 KB
/// Maximum allowed size (bytes) for a service request/response body field.
pub const MAX_SERVICE_BODY_SIZE: usize = 65_536; // 64 KB
/// Maximum allowed length for a service request method field.
pub const MAX_SERVICE_METHOD_LEN: usize = 256;

/// Valid values for service response status.
pub const VALID_SERVICE_STATUSES: &[&str] = &["ok", "not_found", "error"];

/// Common prefix shared by all service message types.
pub const SVC_MESSAGE_PREFIX: &str = "__SVC_";
/// Prefix for service discovery query.
pub const SVC_DISCOVER_QUERY: &str = "__SVC_DISC_Q__";
/// Prefix for service discovery response.
pub const SVC_DISCOVER_RESPONSE: &str = "__SVC_DISC_R__";
/// Prefix for service request.
pub const SVC_REQUEST: &str = "__SVC_REQ__";
/// Prefix for service response.
pub const SVC_RESPONSE: &str = "__SVC_RESP__";

fn default_discovery_max_hops() -> u8 {
    DISCOVERY_QUERY_DEFAULT_MAX_HOPS
}

/// Wire-format payload for a service discovery query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceDiscoveryQueryPayload {
    pub query_id: String,
    pub originator: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    #[serde(default = "default_discovery_max_hops")]
    pub remaining_hops: u8,
}

/// Wire-format payload for a service discovery response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceDiscoveryResponsePayload {
    pub query_id: String,
    pub service_id: String,
    pub version: String,
    pub provider_peer_id: String,
    pub capabilities: HashMap<String, String>,
    pub hop_count: u8,
    /// The original query originator — used for multi-hop response routing.
    #[serde(default)]
    pub originator: String,
}

/// Wire-format payload for a service request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceRequestPayload {
    pub request_id: String,
    pub service_id: String,
    pub method: String,
    pub body: String,
}

/// Wire-format payload for a service response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceResponsePayload {
    pub request_id: String,
    pub service_id: String,
    pub status: String,
    pub body: String,
}

// ---------------------------------------------------------------------------
// Subscriptions (ADR 0035). Every frame below travels only inside the 1:1 MLS
// session between consumer and provider; one found in plaintext is dropped.
// ---------------------------------------------------------------------------

/// Prefix for a subscribe, renewal or resync request (consumer to provider).
pub const SVC_SUBSCRIBE: &str = "__SVC_SUB__";
/// Prefix for the provider's answer to [`SVC_SUBSCRIBE`].
pub const SVC_SUBSCRIBE_REPLY: &str = "__SVC_SUB_R__";
/// Prefix for one update on a subscribed topic (provider to consumer).
pub const SVC_UPDATE: &str = "__SVC_UPD__";
/// Prefix for ending a subscription, from either side.
pub const SVC_UNSUBSCRIBE: &str = "__SVC_UNSUB__";

/// The four subscription prefixes. None is a prefix of another, so a
/// `starts_with` test over this list names exactly one frame.
pub const SVC_SUBSCRIPTION_PREFIXES: [&str; 4] = [
    SVC_SUBSCRIBE,
    SVC_SUBSCRIBE_REPLY,
    SVC_UPDATE,
    SVC_UNSUBSCRIBE,
];

/// Whether `content` is one of the four subscription frames.
pub fn is_subscription_frame(content: &str) -> bool {
    SVC_SUBSCRIPTION_PREFIXES
        .iter()
        .any(|prefix| content.starts_with(prefix))
}

/// Version written into every subscription payload. 2 adds topic patterns
/// and key filters: a provider that answers with 1 applied neither, so a
/// consumer that asked for one ends the subscription rather than receive
/// what it did not ask for.
pub(crate) const SUBSCRIPTION_WIRE_VERSION: u8 = 2;
/// The first version that applies topic patterns and key filters.
pub(crate) const SUBSCRIPTION_FILTERS_VERSION: u8 = 2;

/// Longest topic name, in bytes.
pub const MAX_SUBSCRIPTION_TOPIC_LEN: usize = 128;
/// Longest key of a `latest` topic, in bytes.
pub const MAX_SUBSCRIPTION_KEY_LEN: usize = 256;
/// Entries in a key filter.
pub const MAX_SUBSCRIPTION_FILTER_KEYS: usize = 32;
/// Topics one pattern subscription follows. A provider stops adding topics
/// to it here, and a consumer stops tracking them, which bounds both the
/// catch-up a renewal can cost and what a hostile provider can make a
/// consumer remember.
pub const MAX_PATTERN_TOPICS: usize = 64;
/// Largest `params` a subscriber may hand the provider's application.
pub const MAX_SUBSCRIPTION_PARAMS_SIZE: usize = 4096;
/// Longest reason string carried in a reply or an unsubscribe.
pub const MAX_SUBSCRIPTION_REASON_LEN: usize = 256;
/// Subscriptions a provider holds across every service and subscriber.
pub const MAX_PROVIDER_SUBSCRIPTIONS: usize = 256;
/// Subscriptions one subscriber may hold on one provider.
pub const MAX_SUBSCRIPTIONS_PER_PEER: usize = 16;
/// Subscriptions a consumer holds.
pub const MAX_CONSUMER_SUBSCRIPTIONS: usize = 64;
/// Keys a `latest` topic holds.
pub const MAX_LATEST_KEYS: usize = 256;
/// Bytes of keys and values a `latest` topic holds.
pub const MAX_LATEST_BYTES: usize = 256 * 1024;
/// Keys a consumer tracks per `latest` subscription. Above the provider's own
/// cap so that a genuine provider never reaches it; it bounds a hostile one.
pub(crate) const MAX_CONSUMER_TRACKED_KEYS: usize = 4 * MAX_LATEST_KEYS;
/// Updates a `log` topic buffers for replay.
pub const MAX_LOG_ENTRIES: usize = 256;
/// Bytes of bodies a `log` topic buffers for replay.
pub const MAX_LOG_BYTES: usize = 512 * 1024;
/// Seqs a `log` consumer remembers above its contiguous position.
pub(crate) const MAX_LOG_SEEN_AHEAD: usize = MAX_LOG_ENTRIES;
/// Shortest lease a provider grants, in milliseconds.
pub const MIN_SUBSCRIPTION_LEASE_MS: u64 = 30_000;
/// Longest lease a provider grants, in milliseconds.
pub const MAX_SUBSCRIPTION_LEASE_MS: u64 = 600_000;
/// Lease a consumer asks for unless configured otherwise, in milliseconds.
pub const DEFAULT_SUBSCRIPTION_LEASE_MS: u64 = 120_000;
/// Renewals in a row without an answer before a subscription is paused.
pub(crate) const MAX_UNANSWERED_RENEWALS: u32 = 3;

/// How a topic treats its updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionMode {
    /// State: each update carries a key and only the newest value per key
    /// matters. A consumer can always be sent the current value of every key.
    Latest,
    /// Events: every update matters, in order. Recent updates are buffered for
    /// replay, and a checkpoint summarises what came before them.
    Log,
}

impl SubscriptionMode {
    /// The wire and event spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Latest => "latest",
            Self::Log => "log",
        }
    }
}

/// The provider's answer to a subscribe frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscribeStatus {
    /// The subscription is active; catch-up frames follow.
    Accepted,
    /// The provider's policy or application refused it.
    Rejected,
    /// The provider has no such service registered.
    NotFound,
    /// The topic exists in the other mode.
    ModeMismatch,
    /// The provider is at a subscription cap.
    Full,
}

impl SubscribeStatus {
    /// The wire and event spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::NotFound => "not_found",
            Self::ModeMismatch => "mode_mismatch",
            Self::Full => "full",
        }
    }
}

fn subscription_wire_version() -> u8 {
    SUBSCRIPTION_WIRE_VERSION
}

/// Where a renewing or resyncing consumer stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResumePosition {
    /// The stream the consumer last saw.
    pub stream: String,
    /// `latest`: the highest seq held for any key. `log`: the contiguous
    /// position, every seq up to which has been delivered.
    pub seq: u64,
    /// `latest` only: the digest of the (key, seq) pairs held.
    #[serde(default)]
    pub digest: u64,
    /// A pattern subscription: where it stands in each topic it has seen.
    /// The three fields above are unused for one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<TopicPosition>,
}

/// One topic's position in a pattern subscription's renewal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TopicPosition {
    pub topic: String,
    pub stream: String,
    pub seq: u64,
    #[serde(default)]
    pub digest: u64,
}

/// One topic's answer in a pattern subscription's reply: what
/// [`SubscribeReplyPayload`]'s `stream`, `seq` and `history_lost` say for an
/// exact topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TopicReply {
    pub topic: String,
    pub stream: String,
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub history_lost: bool,
}

/// `__SVC_SUB__`: subscribe, renew or resync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SubscribePayload {
    #[serde(default = "subscription_wire_version")]
    pub v: u8,
    pub sub_id: String,
    pub service_id: String,
    pub topic: String,
    pub mode: SubscriptionMode,
    #[serde(default)]
    pub params: String,
    /// `latest` only: the keys to send, each exact or a prefix ending in
    /// `*`. Empty: every key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    pub lease_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<ResumePosition>,
    /// Asks for the state again now: a full snapshot (`latest`), or a replay
    /// from `resume` falling back to the checkpoint (`log`).
    #[serde(default)]
    pub resync: bool,
}

/// `__SVC_SUB_R__`: the provider's answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SubscribeReplyPayload {
    #[serde(default = "subscription_wire_version")]
    pub v: u8,
    pub sub_id: String,
    pub status: SubscribeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub lease_ms: u64,
    #[serde(default)]
    pub stream: String,
    /// The position the consumer may treat itself as caught up to before the
    /// catch-up frames that follow this reply.
    #[serde(default)]
    pub seq: u64,
    /// The consumer's `log` position is no longer covered by the buffer or a
    /// checkpoint; it has been moved to the present.
    #[serde(default)]
    pub history_lost: bool,
    /// A pattern subscription: the position in each topic it follows, in
    /// place of `stream`, `seq` and `history_lost`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<TopicReply>,
}

/// `__SVC_UPD__`: one update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UpdatePayload {
    #[serde(default = "subscription_wire_version")]
    pub v: u8,
    pub sub_id: String,
    /// A pattern subscription: the topic the update was published to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    pub stream: String,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub body: String,
    /// Sent to catch the consumer up rather than as a fresh publish: a
    /// `latest` key's current value, or a `log` topic's checkpoint.
    #[serde(default)]
    pub snapshot: bool,
}

/// `__SVC_UNSUB__`: either side ends the subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UnsubscribePayload {
    #[serde(default = "subscription_wire_version")]
    pub v: u8,
    pub sub_id: String,
    #[serde(default)]
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame is recognised by `starts_with`, so a prefix that began another
    /// would route the longer frame to the wrong handler.
    #[test]
    fn no_subscription_prefix_begins_another() {
        for a in SVC_SUBSCRIPTION_PREFIXES {
            assert!(a.starts_with(SVC_MESSAGE_PREFIX), "{a} is outside __SVC_");
            for b in SVC_SUBSCRIPTION_PREFIXES {
                if a != b {
                    assert!(!b.starts_with(a), "{a} begins {b}");
                }
            }
            for other in [
                SVC_DISCOVER_QUERY,
                SVC_DISCOVER_RESPONSE,
                SVC_REQUEST,
                SVC_RESPONSE,
            ] {
                assert!(!other.starts_with(a) && !a.starts_with(other));
            }
        }
    }

    #[test]
    fn subscription_frames_are_recognised() {
        assert!(is_subscription_frame("__SVC_SUB__{}"));
        assert!(is_subscription_frame("__SVC_SUB_R__{}"));
        assert!(is_subscription_frame("__SVC_UPD__{}"));
        assert!(is_subscription_frame("__SVC_UNSUB__{}"));
        assert!(!is_subscription_frame("__SVC_REQ__{}"));
        assert!(!is_subscription_frame("__SVC_SUBX{}"));
    }

    /// A field a later version adds must not stop an older reader, and a
    /// field this version omits must default rather than fail.
    #[test]
    fn payloads_read_with_absent_and_unknown_fields() {
        let sub: SubscribePayload = serde_json::from_str(
            r#"{"sub_id":"s","service_id":"svc","topic":"t","mode":"latest","lease_ms":1,"later":true}"#,
        )
        .unwrap();
        assert_eq!(sub.v, SUBSCRIPTION_WIRE_VERSION);
        assert!(sub.resume.is_none() && !sub.resync && sub.params.is_empty());
        assert!(sub.keys.is_empty());

        let reply: SubscribeReplyPayload =
            serde_json::from_str(r#"{"sub_id":"s","status":"mode_mismatch"}"#).unwrap();
        assert_eq!(reply.status, SubscribeStatus::ModeMismatch);
        assert!(!reply.history_lost && reply.topics.is_empty());

        let update: UpdatePayload =
            serde_json::from_str(r#"{"sub_id":"s","stream":"x","seq":3,"body":"b"}"#).unwrap();
        assert!(update.key.is_none() && !update.snapshot && update.topic.is_none());
    }

    #[test]
    fn modes_and_statuses_spell_as_on_the_wire() {
        for mode in [SubscriptionMode::Latest, SubscriptionMode::Log] {
            assert_eq!(
                serde_json::to_string(&mode).unwrap(),
                format!("\"{}\"", mode.as_str())
            );
        }
        for status in [
            SubscribeStatus::Accepted,
            SubscribeStatus::Rejected,
            SubscribeStatus::NotFound,
            SubscribeStatus::ModeMismatch,
            SubscribeStatus::Full,
        ] {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{}\"", status.as_str())
            );
        }
    }
}
