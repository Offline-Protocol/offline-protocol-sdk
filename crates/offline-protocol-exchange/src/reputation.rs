//! Local reputation reads for publishers.
//!
//! v1 reputation is **local-only**: it is derived from this node's own
//! observations (verified vs invalid attestations, settled receipts, key
//! consistency), not from a global reputation network. The read is surfaced
//! alongside discovery results so a consumer can weigh a publisher before
//! transacting. A key change for a known publisher is treated as a serious
//! signal and flags the publisher.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Coarse trust level derived from local observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReputationLevel {
    /// Never observed before.
    Unknown,
    /// Observed with valid attestations but no settled history.
    New,
    /// Valid attestations and at least one settled receipt.
    Established,
    /// Produced an invalid attestation or changed identity keys.
    Flagged,
}

impl ReputationLevel {
    /// Stable string for logging and FFI.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::New => "new",
            Self::Established => "established",
            Self::Flagged => "flagged",
        }
    }
}

/// The reputation read surfaced with a discovery result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReputationRead {
    /// Coarse trust level.
    pub level: ReputationLevel,
    /// Number of receipts settled with this publisher (as observed locally).
    pub settled_receipts: u32,
    /// Number of listings from this publisher that verified.
    pub verified_listings: u32,
    /// Number of invalid attestations observed.
    pub invalid_attestations: u32,
    /// Milliseconds since epoch when this publisher was first observed.
    pub first_seen_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PublisherRecord {
    settled_receipts: u32,
    verified_listings: u32,
    invalid_attestations: u32,
    first_seen_ms: u64,
    /// Pinned attestation public key (base64), first-use.
    pinned_key: Option<String>,
    key_changed: bool,
}

/// Tracks per-publisher observations and derives [`ReputationRead`]s.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReputationTracker {
    records: HashMap<String, PublisherRecord>,
}

impl ReputationTracker {
    /// Creates an empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a verified listing from a publisher, pinning their attestation
    /// key on first use. A different key later marks the publisher flagged.
    pub fn record_verified_listing(&mut self, publisher: &str, key_b64: &str, now_ms: u64) {
        let record = self.entry(publisher, now_ms);
        match &record.pinned_key {
            None => record.pinned_key = Some(key_b64.to_string()),
            Some(pinned) if pinned != key_b64 => record.key_changed = true,
            Some(_) => {}
        }
        record.verified_listings = record.verified_listings.saturating_add(1);
    }

    /// Records an invalid attestation from a publisher.
    pub fn record_invalid_attestation(&mut self, publisher: &str, now_ms: u64) {
        let record = self.entry(publisher, now_ms);
        record.invalid_attestations = record.invalid_attestations.saturating_add(1);
    }

    /// Records a settled receipt with a publisher.
    pub fn record_settled_receipt(&mut self, publisher: &str, now_ms: u64) {
        let record = self.entry(publisher, now_ms);
        record.settled_receipts = record.settled_receipts.saturating_add(1);
    }

    /// The current read for a publisher.
    pub fn read(&self, publisher: &str) -> ReputationRead {
        match self.records.get(publisher) {
            None => ReputationRead {
                level: ReputationLevel::Unknown,
                settled_receipts: 0,
                verified_listings: 0,
                invalid_attestations: 0,
                first_seen_ms: 0,
            },
            Some(record) => {
                let level = if record.key_changed || record.invalid_attestations > 0 {
                    ReputationLevel::Flagged
                } else if record.settled_receipts > 0 {
                    ReputationLevel::Established
                } else if record.verified_listings > 0 {
                    ReputationLevel::New
                } else {
                    ReputationLevel::Unknown
                };
                ReputationRead {
                    level,
                    settled_receipts: record.settled_receipts,
                    verified_listings: record.verified_listings,
                    invalid_attestations: record.invalid_attestations,
                    first_seen_ms: record.first_seen_ms,
                }
            }
        }
    }

    fn entry(&mut self, publisher: &str, now_ms: u64) -> &mut PublisherRecord {
        self.records
            .entry(publisher.to_string())
            .or_insert_with(|| PublisherRecord {
                first_seen_ms: now_ms,
                ..Default::default()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_publisher() {
        let tracker = ReputationTracker::new();
        assert_eq!(tracker.read("nobody").level, ReputationLevel::Unknown);
    }

    #[test]
    fn verified_listing_makes_new() {
        let mut tracker = ReputationTracker::new();
        tracker.record_verified_listing("alice", "key1", 100);
        let read = tracker.read("alice");
        assert_eq!(read.level, ReputationLevel::New);
        assert_eq!(read.verified_listings, 1);
        assert_eq!(read.first_seen_ms, 100);
    }

    #[test]
    fn settled_receipt_establishes() {
        let mut tracker = ReputationTracker::new();
        tracker.record_verified_listing("alice", "key1", 100);
        tracker.record_settled_receipt("alice", 200);
        assert_eq!(tracker.read("alice").level, ReputationLevel::Established);
    }

    #[test]
    fn invalid_attestation_flags() {
        let mut tracker = ReputationTracker::new();
        tracker.record_verified_listing("alice", "key1", 100);
        tracker.record_settled_receipt("alice", 200);
        tracker.record_invalid_attestation("alice", 300);
        // Flagged dominates settled history.
        assert_eq!(tracker.read("alice").level, ReputationLevel::Flagged);
    }

    #[test]
    fn key_change_flags() {
        let mut tracker = ReputationTracker::new();
        tracker.record_verified_listing("alice", "key1", 100);
        tracker.record_verified_listing("alice", "key2", 200);
        assert_eq!(tracker.read("alice").level, ReputationLevel::Flagged);
    }

    #[test]
    fn snapshot_roundtrip() {
        let mut tracker = ReputationTracker::new();
        tracker.record_verified_listing("alice", "key1", 100);
        let json = serde_json::to_string(&tracker).unwrap();
        let restored: ReputationTracker = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.read("alice").level, ReputationLevel::New);
    }
}
