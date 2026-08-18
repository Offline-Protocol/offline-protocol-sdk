//! Per-recipient reachability facts.
//!
//! Transport choice is otherwise a property of *this device's carriers*: DORS
//! scores links, and `can_reach_without_carrying` answers yes for every
//! recipient while any infrastructure carrier is up. That is correct in a
//! single-zone world where every peer is either a neighbour or behind the same
//! relay, and wrong the moment a carrier can be up while a particular recipient
//! is not on it.
//!
//! This table holds the facts that contradict it. A fact is
//! `(recipient, carrier, claim, source, recorded_at)`, and every producer
//! already existed before this module: a gateway's `recipient_unreachable`
//! verdict (today the relay's, tomorrow any gateway's) and a gateway's presence
//! answer.
//!
//! Three rules the shape depends on:
//!
//! - **Absent facts mean today's behaviour.** An unknown answer is never
//!   "unreachable"; it is "no opinion", and every consumer must fall back to
//!   what it did before. A table that has never been written must not change a
//!   single routing decision.
//! - **Facts decay.** Each carries a TTL, after which it reverts to unknown.
//!   Without decay a stale "unreachable" would keep a recovered path shut for
//!   the life of the process, and a stale "reachable" would pin traffic at a
//!   dead one. With decay the worst case is bounded latency, never loss.
//! - **Live mesh links are not stored here.** `connected_peers()` is ground
//!   truth and is already live-queried; copying it into a cache would create
//!   exactly the stale-link failure the transport layer's live-view contract
//!   exists to prevent. This table holds *claims* made by something else.
//!
//! Nothing here settles delivery. A claim opens or economises paths; only the
//! recipient's end-to-end acknowledgement (or terminal outbox expiry) settles a
//! message, so a lying or broken gateway costs latency and battery, never a
//! lost message.

use offline_protocol_transport::TransportType;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How many recipients the table tracks before it evicts.
///
/// Bounds a map keyed by a remote-supplied identifier. Sized like
/// `known_peers`: large enough that a real neighbourhood never evicts, small
/// enough that a flood of invented recipients cannot grow it without bound.
pub(crate) const MAX_TRACKED_PEERS: usize = 1000;

/// How long a delivery verdict stands before reverting to unknown.
///
/// Matched to the unreachable-probe escalation cap: once probes have stretched
/// to their slowest interval, the last verdict is old enough that "we do not
/// know" is the more honest answer than repeating it.
pub(crate) const VERDICT_TTL: Duration = Duration::from_secs(600);

/// How long a presence answer stands before reverting to unknown.
///
/// Shorter than a verdict on purpose. A verdict is evidence about a delivery
/// this device actually attempted; presence is a third party's report about
/// someone else's connection, which a phone invalidates by walking into a lift.
pub(crate) const PRESENCE_TTL: Duration = Duration::from_secs(300);

/// What a fact claims about reaching a recipient over one carrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Claim {
    /// The carrier reported it could not reach this recipient.
    Unreachable,
    /// The carrier reported this recipient as present on it.
    Reachable,
}

/// Where a claim came from. Determines how long it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FactSource {
    /// A gateway refused a frame for this recipient (`recipient_unreachable`).
    GatewayVerdict,
    /// A gateway answered a presence query about this recipient.
    GatewayPresence,
}

impl FactSource {
    fn ttl(self) -> Duration {
        match self {
            FactSource::GatewayVerdict => VERDICT_TTL,
            FactSource::GatewayPresence => PRESENCE_TTL,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Fact {
    claim: Claim,
    source: FactSource,
    recorded_at: Instant,
}

impl Fact {
    fn is_fresh(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.recorded_at) < self.source.ttl()
    }
}

/// Every carrier's standing claim about one recipient.
///
/// A `Vec` rather than a map because the carrier set is tiny (the
/// infrastructure carriers, at most three today) and a linear scan over three
/// entries beats hashing.
#[derive(Debug, Default)]
struct PeerFacts {
    by_carrier: Vec<(TransportType, Fact)>,
    /// Last write, for eviction order.
    touched_at: Option<Instant>,
}

/// The per-recipient reachability facts this device currently holds.
#[derive(Debug, Default)]
pub(crate) struct ReachabilityFacts {
    peers: HashMap<String, PeerFacts>,
}

impl ReachabilityFacts {
    /// Records what `carrier` just claimed about `peer_id`.
    ///
    /// The newest claim for a carrier replaces the previous one outright: a
    /// verdict and a presence answer are answers to the same question, and the
    /// later one is the one that reflects the network now.
    pub(crate) fn record(
        &mut self,
        peer_id: &str,
        carrier: TransportType,
        claim: Claim,
        source: FactSource,
        now: Instant,
    ) {
        if peer_id.is_empty() {
            return;
        }
        if !self.peers.contains_key(peer_id) {
            self.evict_if_full(now);
        }
        let entry = self.peers.entry(peer_id.to_string()).or_default();
        let fact = Fact {
            claim,
            source,
            recorded_at: now,
        };
        match entry.by_carrier.iter_mut().find(|(t, _)| *t == carrier) {
            Some((_, existing)) => *existing = fact,
            None => entry.by_carrier.push((carrier, fact)),
        }
        entry.touched_at = Some(now);
    }

    /// What `carrier` currently claims about `peer_id`, if anything fresh.
    ///
    /// `None` means unknown, which every caller must read as "behave as though
    /// this table did not exist".
    pub(crate) fn claim_for(
        &self,
        peer_id: &str,
        carrier: TransportType,
        now: Instant,
    ) -> Option<Claim> {
        self.peers
            .get(peer_id)
            .and_then(|facts| facts.by_carrier.iter().find(|(t, _)| *t == carrier))
            .filter(|(_, fact)| fact.is_fresh(now))
            .map(|(_, fact)| fact.claim)
    }

    /// Drops every fact that has aged out, and any recipient left with none.
    ///
    /// Called from the engine's periodic cleanup. Expiry is already enforced on
    /// read, so this reclaims memory rather than changing any answer.
    pub(crate) fn prune(&mut self, now: Instant) {
        self.peers.retain(|_, facts| {
            facts.by_carrier.retain(|(_, fact)| fact.is_fresh(now));
            !facts.by_carrier.is_empty()
        });
    }

    /// How many recipients the table currently holds. Test and diagnostic use.
    #[cfg(test)]
    pub(crate) fn tracked_peers(&self) -> usize {
        self.peers.len()
    }

    /// Makes room for one new recipient, oldest write first.
    ///
    /// Prunes first: an expired entry is free to drop and costs a live one
    /// nothing. Only if the table is still full does the least recently written
    /// recipient go.
    fn evict_if_full(&mut self, now: Instant) {
        if self.peers.len() < MAX_TRACKED_PEERS {
            return;
        }
        self.prune(now);
        while self.peers.len() >= MAX_TRACKED_PEERS {
            let oldest = self
                .peers
                .iter()
                .min_by_key(|(_, facts)| facts.touched_at)
                .map(|(peer, _)| peer.clone());
            match oldest {
                Some(peer) => {
                    self.peers.remove(&peer);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn an_unwritten_table_has_no_opinion_about_anyone() {
        let facts = ReachabilityFacts::default();
        assert_eq!(
            facts.claim_for("bob", TransportType::Internet, Instant::now()),
            None,
            "absent facts must read as unknown, never as unreachable"
        );
    }

    #[test]
    fn a_fact_answers_only_for_the_carrier_that_made_it() {
        let now = Instant::now();
        let mut facts = ReachabilityFacts::default();
        facts.record(
            "bob",
            TransportType::Internet,
            Claim::Unreachable,
            FactSource::GatewayVerdict,
            now,
        );

        assert_eq!(
            facts.claim_for("bob", TransportType::Internet, now),
            Some(Claim::Unreachable)
        );
        assert_eq!(
            facts.claim_for("bob", TransportType::Reticulum, now),
            None,
            "one carrier's verdict says nothing about another's reach"
        );
        assert_eq!(
            facts.claim_for("carol", TransportType::Internet, now),
            None,
            "and nothing about another recipient"
        );
    }

    #[test]
    fn a_verdict_reverts_to_unknown_rather_than_standing_forever() {
        // The failure decay prevents: a stale "unreachable" keeping a path
        // shut long after the recipient came back.
        let now = Instant::now();
        let mut facts = ReachabilityFacts::default();
        facts.record(
            "bob",
            TransportType::Internet,
            Claim::Unreachable,
            FactSource::GatewayVerdict,
            now,
        );

        assert_eq!(
            facts.claim_for(
                "bob",
                TransportType::Internet,
                at(now, VERDICT_TTL.as_secs() - 1)
            ),
            Some(Claim::Unreachable),
            "inside its lifetime the verdict stands"
        );
        assert_eq!(
            facts.claim_for(
                "bob",
                TransportType::Internet,
                at(now, VERDICT_TTL.as_secs() + 1)
            ),
            None,
            "past it the answer is unknown, which means today's behaviour"
        );
    }

    #[test]
    fn a_presence_answer_decays_faster_than_a_verdict() {
        // A verdict is evidence about a delivery this device attempted;
        // presence is a third party's report about someone else's connection.
        let now = Instant::now();
        let mut facts = ReachabilityFacts::default();
        facts.record(
            "bob",
            TransportType::Internet,
            Claim::Reachable,
            FactSource::GatewayPresence,
            now,
        );

        let after_presence_ttl = at(now, PRESENCE_TTL.as_secs() + 1);
        assert_eq!(
            facts.claim_for("bob", TransportType::Internet, after_presence_ttl),
            None
        );
        assert!(
            PRESENCE_TTL < VERDICT_TTL,
            "the ordering is the point, not the specific numbers"
        );
    }

    #[test]
    fn the_newest_answer_for_a_carrier_replaces_the_last() {
        let now = Instant::now();
        let mut facts = ReachabilityFacts::default();
        facts.record(
            "bob",
            TransportType::Internet,
            Claim::Unreachable,
            FactSource::GatewayVerdict,
            now,
        );
        facts.record(
            "bob",
            TransportType::Internet,
            Claim::Reachable,
            FactSource::GatewayPresence,
            at(now, 5),
        );

        assert_eq!(
            facts.claim_for("bob", TransportType::Internet, at(now, 6)),
            Some(Claim::Reachable),
            "both answer the same question; the later one reflects the network now"
        );
    }

    #[test]
    fn the_table_stays_bounded_under_a_flood_of_recipients() {
        // Keyed by a remote-supplied identifier, so it must not grow without
        // bound just because someone invents recipients.
        let now = Instant::now();
        let mut facts = ReachabilityFacts::default();
        for n in 0..(MAX_TRACKED_PEERS + 50) {
            facts.record(
                &format!("peer{n}"),
                TransportType::Internet,
                Claim::Unreachable,
                FactSource::GatewayVerdict,
                at(now, n as u64),
            );
        }

        assert!(
            facts.tracked_peers() <= MAX_TRACKED_PEERS,
            "the table must stay within its cap, found {}",
            facts.tracked_peers()
        );
        assert_eq!(
            facts.claim_for("peer0", TransportType::Internet, at(now, 1)),
            None,
            "the oldest write is the one evicted"
        );
        assert_eq!(
            facts.claim_for(
                &format!("peer{}", MAX_TRACKED_PEERS + 49),
                TransportType::Internet,
                at(now, (MAX_TRACKED_PEERS + 49) as u64)
            ),
            Some(Claim::Unreachable),
            "the newest write survives"
        );
    }

    #[test]
    fn pruning_drops_aged_facts_and_the_recipients_left_empty() {
        let now = Instant::now();
        let mut facts = ReachabilityFacts::default();
        facts.record(
            "bob",
            TransportType::Internet,
            Claim::Unreachable,
            FactSource::GatewayVerdict,
            now,
        );
        assert_eq!(facts.tracked_peers(), 1);

        facts.prune(at(now, VERDICT_TTL.as_secs() + 1));
        assert_eq!(
            facts.tracked_peers(),
            0,
            "a recipient whose every fact expired holds no memory"
        );
    }

    #[test]
    fn an_empty_recipient_is_never_recorded() {
        let mut facts = ReachabilityFacts::default();
        facts.record(
            "",
            TransportType::Internet,
            Claim::Unreachable,
            FactSource::GatewayVerdict,
            Instant::now(),
        );
        assert_eq!(facts.tracked_peers(), 0);
    }
}
