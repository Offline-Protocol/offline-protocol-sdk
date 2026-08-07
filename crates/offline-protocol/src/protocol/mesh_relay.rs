//! Admission control and pacing for mesh forwarding.
//!
//! Forwarding is what turns a set of nearby devices into a network: a frame
//! addressed to someone out of radio range is carried by the devices in
//! between. The hazard is that the same property which gives coverage —
//! every node repeating what it hears — also multiplies traffic, and in a
//! dense room the multiplication can outrun the radio. This module is the
//! governor on that: it decides whether a frame is forwarded at all, when,
//! and how fast frames may leave.
//!
//! Four mechanisms, each covering something the others cannot:
//!
//! 1. **Suppression** ([`RelaySeenCache`]) — an id is forwarded once. Without
//!    it, copies circulate until their hop limit runs out, and every node
//!    repeats every copy.
//! 2. **Jitter with cancellation** — a forward waits a short randomized delay
//!    before going out, and is dropped if the same id arrives again while it
//!    waits. In a dense cluster the first neighbor to fire covers everyone who
//!    can hear it, and the rest stand down. This is what makes cost scale with
//!    *coverage* rather than with the number of links, and it adapts on its own
//!    as the room fills up — no threshold to tune.
//! 3. **Hop limits** — an arriving frame's remaining hop budget is clamped
//!    before use, so a frame that claims an implausible budget (nothing
//!    authenticates it at this layer) cannot circulate longer than one our own
//!    policy would have issued.
//! 4. **Budgets** — a hard ceiling on frames per second, in total and per
//!    neighbor. The first three assume frames are what they appear to be;
//!    budgets hold even when they are not. Whatever arrives, and however it is
//!    shaped, a node cannot be made to transmit faster than its budget: that
//!    invariant is what bounds the worst case, so the failure mode under attack
//!    or overload is added delay rather than a radio that never goes quiet.
//!
//! Ordering matters. Suppression runs before anything expensive, admission
//! before queueing (so a flood cannot grow the queue), and pacing at the moment
//! of transmission (so a forward cancelled while waiting costs no budget).

use offline_protocol_core::{Message, MessagePriority};
use offline_protocol_reliability::{RelaySeenCache, RelaySeenConfig};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// Hop budget a forwarded frame is clamped to under normal density.
pub const DEFAULT_RELAY_MAX_TTL: u8 = 8;

/// Hop budget used once the local neighborhood is dense.
///
/// With many neighbors, coverage is reached in fewer hops — the same message
/// travels further per hop — so a lower ceiling reaches the same devices for
/// less traffic.
pub const DEFAULT_RELAY_DENSE_MAX_TTL: u8 = 5;

/// Neighbor count at or above which the dense hop budget applies.
pub const DEFAULT_RELAY_DENSE_DEGREE: usize = 6;

/// Absolute ceiling on how many times a frame may be forwarded, regardless of
/// the hop budget it carries. A backstop against a frame whose hop fields have
/// been shaped to evade the clamp.
pub const RELAY_MAX_HOP_COUNT: u8 = 16;

/// Neighbors a single frame is forwarded to.
pub const DEFAULT_RELAY_FANOUT: usize = 3;

/// Shortest delay before a queued forward is transmitted.
pub const DEFAULT_RELAY_JITTER_MIN_MS: u64 = 20;

/// Longest delay before a queued forward is transmitted, at low density.
pub const DEFAULT_RELAY_JITTER_MAX_MS: u64 = 200;

/// Sustained ceiling on forwarded frames leaving this node, per second.
pub const DEFAULT_RELAY_RATE_PER_SEC: f32 = 10.0;

/// Burst allowance on top of the sustained forwarding rate.
pub const DEFAULT_RELAY_BURST: f32 = 30.0;

/// Sustained ceiling on frames accepted for forwarding from any one neighbor.
///
/// Keeps a single noisy or hostile link from consuming the whole node's
/// forwarding budget: its frames are refused at the door, leaving the budget
/// for everyone else's traffic.
pub const DEFAULT_RELAY_PEER_RATE_PER_SEC: f32 = 5.0;

/// Burst allowance for a single neighbor.
pub const DEFAULT_RELAY_PEER_BURST: f32 = 15.0;

/// Maximum forwards waiting for their delay to elapse.
pub const DEFAULT_RELAY_QUEUE_CAPACITY: usize = 256;

/// Maximum neighbors tracked for per-neighbor rate limiting.
pub const MAX_RELAY_RATE_PEERS: usize = 256;

/// How long a queued forward may sit past its due time before being abandoned.
///
/// If a node is so far over budget that a frame waits this long, the frame's
/// usefulness has passed — other paths have carried it or the sender has
/// retransmitted — and holding it only displaces newer traffic.
const RELAY_QUEUE_MAX_OVERDUE: Duration = Duration::from_secs(5);

/// Tunables for mesh forwarding.
#[derive(Debug, Clone)]
pub struct MeshRelayConfig {
    /// Hop budget a forwarded frame is clamped to.
    pub max_ttl: u8,
    /// Hop budget applied once the neighborhood is dense.
    pub dense_max_ttl: u8,
    /// Neighbor count at which the dense budget applies.
    pub dense_degree: usize,
    /// Neighbors a frame is forwarded to.
    pub fanout: usize,
    /// Shortest pre-transmit delay.
    pub jitter_min: Duration,
    /// Longest pre-transmit delay at low density.
    pub jitter_max: Duration,
    /// Sustained forwarding rate, frames per second.
    pub rate_per_sec: f32,
    /// Burst allowance above the sustained rate.
    pub burst: f32,
    /// Sustained per-neighbor acceptance rate.
    pub peer_rate_per_sec: f32,
    /// Per-neighbor burst allowance.
    pub peer_burst: f32,
    /// Maximum forwards awaiting transmission.
    pub queue_capacity: usize,
    /// Suppression cache sizing.
    pub seen: RelaySeenConfig,
}

impl Default for MeshRelayConfig {
    fn default() -> Self {
        Self {
            max_ttl: DEFAULT_RELAY_MAX_TTL,
            dense_max_ttl: DEFAULT_RELAY_DENSE_MAX_TTL,
            dense_degree: DEFAULT_RELAY_DENSE_DEGREE,
            fanout: DEFAULT_RELAY_FANOUT,
            jitter_min: Duration::from_millis(DEFAULT_RELAY_JITTER_MIN_MS),
            jitter_max: Duration::from_millis(DEFAULT_RELAY_JITTER_MAX_MS),
            rate_per_sec: DEFAULT_RELAY_RATE_PER_SEC,
            burst: DEFAULT_RELAY_BURST,
            peer_rate_per_sec: DEFAULT_RELAY_PEER_RATE_PER_SEC,
            peer_burst: DEFAULT_RELAY_PEER_BURST,
            queue_capacity: DEFAULT_RELAY_QUEUE_CAPACITY,
            seen: RelaySeenConfig::default(),
        }
    }
}

/// Why a frame was not queued for forwarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayRejection {
    /// Already handled; this is another copy of a frame we have dealt with.
    AlreadySeen,
    /// Another copy arrived while this one was waiting, so the pending forward
    /// was cancelled — a neighbor has covered it.
    SupersededByDuplicate,
    /// The frame has no hop budget left.
    HopLimitReached,
    /// The neighbor that sent it is over its acceptance rate.
    PeerRateLimited,
    /// No room in the pending queue.
    QueueFull,
}

/// The outcome of offering an inbound frame to the governor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayAdmission {
    /// Queued; it will be transmitted once its delay elapses.
    Queued,
    /// Not queued.
    Rejected(RelayRejection),
}

/// A forward waiting for its delay to elapse.
#[derive(Debug, Clone)]
pub struct PendingRelay {
    /// The frame to forward, with hop fields already adjusted.
    pub message: Message,
    /// The neighbor it arrived from, which must not receive it back.
    pub arrival_peer: Option<String>,
    /// When it becomes eligible to transmit.
    pub due_at: Instant,
}

/// Running totals, for telemetry and for tests that assert a flood stayed
/// bounded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeshRelayCounters {
    /// Frames queued for forwarding.
    pub queued: u64,
    /// Times a frame was put on a link, counting each link separately.
    ///
    /// This is the airtime meter — what the send budget actually bounds — so it
    /// counts this device handing over its *own* messages too. For "how much
    /// have I carried for other people", see [`Self::forwarded`].
    pub transmissions: u64,
    /// Third-party frames that crossed at least one link on someone's behalf.
    ///
    /// Counted once per frame carried, not once per link, and never for this
    /// device's own traffic.
    pub forwarded: u64,
    /// Frames put back on the queue because the device ran out of budget
    /// before they reached any neighbor.
    pub requeued_for_budget: u64,
    /// Queued forwards displaced to make room for a higher-priority frame.
    pub queue_evicted: u64,
    /// Arrivals suppressed as already handled.
    pub duplicates_suppressed: u64,
    /// Pending forwards cancelled because a neighbor covered them.
    pub cancelled_by_duplicate: u64,
    /// Frames refused because their sender was over its rate.
    pub peer_rate_limited: u64,
    /// Frames refused because the pending queue was full.
    pub queue_full: u64,
    /// Frames dropped for having no hop budget left.
    pub hop_limit_reached: u64,
    /// Transmissions deferred because the node was at its forwarding rate.
    pub rate_deferred: u64,
    /// Queued forwards abandoned after waiting too long past their due time.
    pub abandoned_overdue: u64,
    /// Hop budgets reduced because the frame claimed more than policy allows.
    pub ttl_clamped: u64,
}

/// What this device has carried for other people.
///
/// A snapshot, safe to poll: every field counts since start-up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeshRelayStats {
    /// Frames carried onward on someone else's behalf, counted once each.
    pub forwarded: u64,
    /// Times this device put a frame on a link, counting each link separately
    /// and including hand-offs of its own messages.
    ///
    /// This is what the per-second forwarding budget bounds, so it is the
    /// number to compare against that ceiling — [`Self::forwarded`] is the
    /// contribution figure to show a user.
    pub transmissions: u64,
    /// Frames accepted for forwarding.
    pub queued: u64,
    /// Frames waiting to go out right now.
    pub awaiting_transmission: usize,
    /// Arrivals ignored because the frame had already been handled.
    pub duplicates_suppressed: u64,
    /// Forwards dropped because a neighbor transmitted the same frame first —
    /// the saving that keeps a crowded room from repeating itself.
    pub covered_by_a_neighbor: u64,
    /// Frames refused because the neighbor sending them was over its share.
    pub peer_rate_limited: u64,
    /// Transmissions held back because this device was at its forwarding rate.
    ///
    /// A frame counted here is delayed, not dropped: it goes back on the queue
    /// and is tried again on the next tick, until it is either sent or has
    /// waited so long past its turn that carrying it no longer helps.
    pub rate_deferred: u64,
    /// Frames that had travelled as far as they were allowed to.
    pub hop_limit_reached: u64,
    /// Frames whose claimed reach was cut down to local policy.
    pub reach_clamped: u64,
    /// Ids forgotten for lack of room rather than age.
    ///
    /// Expected to stay at zero. A non-zero value means this device is seeing
    /// more traffic than it can remember having handled, and a frame it forgets
    /// can be forwarded a second time.
    pub dropped_for_capacity: u64,
}

/// A refilling allowance, in frames.
#[derive(Debug, Clone)]
struct TokenBucket {
    tokens: f32,
    capacity: f32,
    refill_per_sec: f32,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(capacity: f32, refill_per_sec: f32, now: Instant) -> Self {
        Self {
            tokens: capacity.max(0.0),
            capacity: capacity.max(0.0),
            refill_per_sec: refill_per_sec.max(0.0),
            last_refill: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f32();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
            self.last_refill = now;
        }
    }

    fn try_take(&mut self, now: Instant) -> bool {
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Decides what gets forwarded, when, and how fast.
///
/// Holds no transports and sends nothing itself: it answers questions and hands
/// back due work, so its behavior is testable without a radio and its policy
/// stays in one place rather than spread through the receive path.
#[derive(Debug)]
pub struct MeshRelayGovernor {
    config: MeshRelayConfig,
    seen: RelaySeenCache,
    /// Forwards awaiting their delay, in the order they were queued.
    pending: Vec<PendingRelay>,
    send_budget: TokenBucket,
    peer_budgets: HashMap<String, TokenBucket>,
    counters: MeshRelayCounters,
    /// Seeds the per-frame delay so two nodes holding the same frame pick
    /// different delays.
    local_id: String,
}

impl MeshRelayGovernor {
    /// Creates a governor with explicit tunables.
    pub fn with_config(local_id: impl Into<String>, config: MeshRelayConfig) -> Self {
        let now = Instant::now();
        let seen = RelaySeenCache::with_config(config.seen.clone());
        Self {
            send_budget: TokenBucket::new(config.burst, config.rate_per_sec, now),
            seen,
            pending: Vec::new(),
            peer_budgets: HashMap::new(),
            counters: MeshRelayCounters::default(),
            local_id: local_id.into(),
            config,
        }
    }

    /// The tunables in force. Apps read these from
    /// `ProtocolConfig::mesh_relay`; this is for the tests that assert the
    /// defaults hold together.
    #[cfg(test)]
    pub fn config(&self) -> &MeshRelayConfig {
        &self.config
    }

    /// Running totals.
    pub fn counters(&self) -> &MeshRelayCounters {
        &self.counters
    }

    /// Forwards awaiting transmission.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Ids dropped from the suppression cache for capacity rather than age.
    /// Expected to stay zero; see [`RelaySeenCache::capacity_evictions`].
    pub fn seen_capacity_evictions(&self) -> u64 {
        self.seen.capacity_evictions()
    }

    /// Offers an inbound third-party frame for forwarding.
    ///
    /// Runs the full admission sequence and, when the frame is accepted,
    /// queues an adjusted copy for later transmission. `degree` is the current
    /// neighbor count, which sets both the hop budget and the delay spread.
    /// `is_last_hop` says whether the frame's recipient is one of our own
    /// neighbors, which changes what a duplicate means — see below.
    pub fn admit(
        &mut self,
        message: &Message,
        arrival_peer: Option<&str>,
        degree: usize,
        is_last_hop: bool,
    ) -> RelayAdmission {
        let now = Instant::now();
        let message_id = message.id.as_str();

        // Has this id been dealt with? Checked without recording, because a
        // frame that is about to be *refused* must not be remembered as
        // handled: another copy of it arriving over a healthy link a moment
        // later is the alternate path that should still carry it. Recording on
        // rejection would leave the id suppressed for the whole retention
        // window, so a single link running hot — or one frame with its hop
        // budget rewritten to nothing, which nothing here can authenticate —
        // would blank every path through this device.
        if self.seen.contains(&message_id) {
            self.counters.duplicates_suppressed =
                self.counters.duplicates_suppressed.saturating_add(1);

            // Our own copy is still waiting, so a neighbor has transmitted
            // this frame already.
            if let Some(index) = self
                .pending
                .iter()
                .position(|p| p.message.id.as_str() == message_id)
            {
                // Standing down is right in the middle of a network, where a
                // neighbor's transmission reaches the same region ours would.
                // It is wrong at the last hop: our pending copy is addressed
                // to the recipient themselves, and no other device can be
                // assumed to hold that link. Dropping it there loses the
                // delivery outright — and leaves the id suppressed, so the
                // sender's retries cannot rescue it either.
                if is_last_hop {
                    return RelayAdmission::Rejected(RelayRejection::AlreadySeen);
                }

                self.pending.remove(index);
                self.counters.cancelled_by_duplicate =
                    self.counters.cancelled_by_duplicate.saturating_add(1);
                return RelayAdmission::Rejected(RelayRejection::SupersededByDuplicate);
            }

            return RelayAdmission::Rejected(RelayRejection::AlreadySeen);
        }

        // Per-neighbor admission, before the frame can occupy queue space.
        //
        // A frame whose arrival link the carrier did not identify skips this
        // step: there is no link to charge it to, and inventing a shared bucket
        // for "unknown" would let one such carrier throttle every other. Those
        // frames are still bounded by the queue capacity and the per-second
        // send budget below, which is what makes skipping it acceptable — the
        // per-neighbor limit shapes *fairness between links*, and a frame with
        // no known link has no share to exceed.
        if let Some(peer) = arrival_peer {
            if !self.take_peer_token(peer, now) {
                self.counters.peer_rate_limited = self.counters.peer_rate_limited.saturating_add(1);
                return RelayAdmission::Rejected(RelayRejection::PeerRateLimited);
            }
        }

        // Hop accounting. The arriving budget is clamped to what our own policy
        // would have issued before it is spent, so an inflated claim buys at
        // most this one hop.
        let Some(forwarded) = self.prepare_hop(message, degree) else {
            self.counters.hop_limit_reached = self.counters.hop_limit_reached.saturating_add(1);
            return RelayAdmission::Rejected(RelayRejection::HopLimitReached);
        };

        if self.pending.len() >= self.config.queue_capacity && !self.evict_for(&forwarded, now) {
            self.counters.queue_full = self.counters.queue_full.saturating_add(1);
            return RelayAdmission::Rejected(RelayRejection::QueueFull);
        }

        // Accepted: now it is genuinely handled, and further copies can be
        // suppressed. Recording here rather than at the top also means the
        // cache fills at the rate frames are *taken on*, which the per-neighbor
        // and per-device limits bound — not at the rate they arrive, which
        // nothing bounds.
        self.seen.observe(&message_id);

        let due_at = now + self.jitter_for(&message_id, degree);
        self.pending.push(PendingRelay {
            message: forwarded,
            arrival_peer: arrival_peer.map(str::to_string),
            due_at,
        });
        self.counters.queued = self.counters.queued.saturating_add(1);

        RelayAdmission::Queued
    }

    /// Returns the forwards whose delay has elapsed, removing them from the
    /// queue.
    ///
    /// Frames whose delay has not elapsed stay queued, as do frames the device
    /// has no budget left to transmit. A frame that has waited far past its
    /// turn is abandoned instead: by then other paths have carried it or the
    /// sender has retransmitted, and holding it only displaces newer traffic.
    ///
    /// Budget is *checked* here and *spent* in [`Self::take_send_token`], once
    /// per link a frame actually crosses. Spending it here instead would
    /// undercount by the fan-out — one release from this queue can put the
    /// frame on several links — and the whole point of the ceiling is that it
    /// bounds what reaches the radio.
    ///
    /// The check is therefore not a reservation, and cannot be: a released
    /// frame may still find the budget gone by the time it reaches the radio,
    /// because the frames released alongside it spend from the same bucket. A
    /// caller that gets nothing onto a link for that reason must hand the frame
    /// back with [`Self::requeue`] rather than drop it — the id is already
    /// recorded as handled here, so dropping it would lose this copy *and*
    /// refuse the copies and retransmissions that follow.
    pub fn take_due(&mut self, now: Instant) -> Vec<PendingRelay> {
        self.seen.expire(now);

        let mut due = Vec::new();
        let mut keep: Vec<PendingRelay> = Vec::with_capacity(self.pending.len());

        for relay in std::mem::take(&mut self.pending) {
            if relay.due_at > now {
                keep.push(relay);
                continue;
            }

            if now.saturating_duration_since(relay.due_at) > RELAY_QUEUE_MAX_OVERDUE {
                self.counters.abandoned_overdue = self.counters.abandoned_overdue.saturating_add(1);
                continue;
            }

            if self.has_send_budget(now) {
                due.push(relay);
            } else {
                self.counters.rate_deferred = self.counters.rate_deferred.saturating_add(1);
                keep.push(relay);
            }
        }

        self.pending = keep;
        due
    }

    /// Whether any budget remains to transmit right now.
    fn has_send_budget(&mut self, now: Instant) -> bool {
        self.send_budget.refill(now);
        self.send_budget.tokens >= 1.0
    }

    /// Claims budget for putting one frame on one link.
    ///
    /// Every transmission goes through here — forwarding another device's
    /// frame and handing over one of our own alike — because the ceiling is
    /// about what this device puts on the air, not about whose message it is.
    /// Returns false when the device is at its limit and the caller should not
    /// transmit.
    pub fn take_send_token(&mut self) -> bool {
        if self.send_budget.try_take(Instant::now()) {
            self.counters.transmissions = self.counters.transmissions.saturating_add(1);
            true
        } else {
            self.counters.rate_deferred = self.counters.rate_deferred.saturating_add(1);
            false
        }
    }

    /// Records that a third-party frame was carried onward.
    ///
    /// Separate from [`Self::take_send_token`], which meters airtime for every
    /// frame this device transmits including its own. Counted once per frame
    /// carried rather than once per link, so it reads as "messages I moved for
    /// other people".
    pub fn record_forwarded(&mut self) {
        self.counters.forwarded = self.counters.forwarded.saturating_add(1);
    }

    /// Puts a released forward back on the queue, keeping its original due
    /// time.
    ///
    /// For the frame that reached no neighbor because the budget ran out
    /// between [`Self::take_due`] releasing it and the radio being asked. It
    /// keeps its due time deliberately: the overdue cut-off in `take_due` is
    /// measured from that, so a frame that stays starved is eventually
    /// abandoned instead of being retried forever.
    ///
    /// Refused only if the queue has filled meanwhile, which is the same
    /// bound every other queued frame is subject to.
    pub fn requeue(&mut self, relay: PendingRelay) {
        if self.pending.len() >= self.config.queue_capacity {
            self.counters.queue_full = self.counters.queue_full.saturating_add(1);
            return;
        }
        self.counters.requeued_for_budget = self.counters.requeued_for_budget.saturating_add(1);
        self.pending.push(relay);
    }

    /// Records an id as handled without forwarding it.
    ///
    /// Used for frames we consume ourselves, so a copy arriving later by
    /// another path is not forwarded on their behalf.
    pub fn mark_handled(&mut self, message_id: &str) {
        self.seen.observe(message_id);
    }

    /// Neighbors a frame should be forwarded to, given the current neighbor
    /// set and the peers it must not go back to.
    ///
    /// Excludes the neighbor it arrived from and the peer that wrote it —
    /// both already have it, and returning it is how a frame ends up bouncing
    /// between two nodes.
    pub fn select_targets<'a>(
        &self,
        neighbors: impl IntoIterator<Item = (&'a str, u8)>,
        exclude: &[&str],
        message_id: &str,
    ) -> Vec<String> {
        let mut candidates: Vec<(&str, u8)> = neighbors
            .into_iter()
            .filter(|(peer, _)| !exclude.contains(peer))
            .collect();

        if candidates.len() <= self.config.fanout {
            return candidates
                .into_iter()
                .map(|(peer, _)| peer.to_string())
                .collect();
        }

        // Strongest links first: a fan-out slot spent on a barely-connected
        // neighbor is one the frame probably does not survive, and there are
        // only a few slots.
        //
        // Ties break on a hash of (id, peer, self), which does two things.
        // Within a group of equally good links it spreads different messages
        // across different neighbors rather than pinning every frame to the
        // same one. And it is stable per frame: a second copy of the same
        // message picks the same neighbors instead of a fresh set, so a
        // retransmission does not widen its own footprint.
        candidates.sort_by(|(a_peer, a_quality), (b_peer, b_quality)| {
            b_quality.cmp(a_quality).then_with(|| {
                Self::hash_of(&[message_id, a_peer, &self.local_id]).cmp(&Self::hash_of(&[
                    message_id,
                    b_peer,
                    &self.local_id,
                ]))
            })
        });
        candidates
            .into_iter()
            .take(self.config.fanout)
            .map(|(peer, _)| peer.to_string())
            .collect()
    }

    /// Drops per-neighbor rate state for a peer that has gone away.
    pub fn forget_peer(&mut self, peer_id: &str) {
        self.peer_budgets.remove(peer_id);
    }

    /// Releases expired suppression entries. Safe to call from a periodic tick.
    pub fn maintain(&mut self, now: Instant) {
        self.seen.expire(now);
    }

    /// Builds the copy to forward, or `None` when the frame has no hop budget
    /// left.
    fn prepare_hop(&mut self, message: &Message, degree: usize) -> Option<Message> {
        if message.hop_count.value() >= RELAY_MAX_HOP_COUNT {
            return None;
        }

        let ceiling = if degree >= self.config.dense_degree {
            self.config.dense_max_ttl
        } else {
            self.config.max_ttl
        };

        let claimed = message.ttl.value();
        let effective = claimed.min(ceiling);
        if effective < claimed {
            self.counters.ttl_clamped = self.counters.ttl_clamped.saturating_add(1);
        }

        // A budget of 1 or less is spent: this node is the last that may hold
        // the frame, matching `TTL::is_exhausted`.
        let remaining = offline_protocol_core::TTL::from_value(effective).decrement()?;

        let mut forwarded = message.clone();
        // Rewrite rather than decrement in place: the frame leaves with a
        // budget this node vouches for, so an inflated claim cannot survive
        // past this hop.
        forwarded.ttl = remaining;
        let _ = forwarded.increment_hop();
        Some(forwarded)
    }

    /// Makes room for a higher-priority forward by displacing the
    /// lowest-priority one that is not yet due.
    ///
    /// Frames already past their due time are left alone: they are on their way
    /// to the radio this tick, so taking one costs a transmission that was
    /// about to happen while the frame replacing it still has to wait out its
    /// hold. Only when every lower-priority frame is already due does it
    /// displace one of those instead of refusing outright.
    fn evict_for(&mut self, candidate: &Message, now: Instant) -> bool {
        let candidate_rank = priority_rank(candidate.priority);
        let lower_priority = |p: &PendingRelay| priority_rank(p.message.priority) < candidate_rank;

        let victim = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, p)| lower_priority(p) && p.due_at > now)
            .min_by_key(|(_, p)| priority_rank(p.message.priority))
            .map(|(index, _)| index)
            .or_else(|| {
                self.pending
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| lower_priority(p))
                    .min_by_key(|(_, p)| priority_rank(p.message.priority))
                    .map(|(index, _)| index)
            });

        match victim {
            Some(index) => {
                self.pending.remove(index);
                self.counters.queue_evicted = self.counters.queue_evicted.saturating_add(1);
                true
            }
            None => false,
        }
    }

    fn take_peer_token(&mut self, peer_id: &str, now: Instant) -> bool {
        if !self.peer_budgets.contains_key(peer_id) {
            // Bound the map: an attacker cannot grow it with invented peer ids
            // because entries are only created for peers we hold a link to,
            // but churn over a long session can still accumulate.
            if self.peer_budgets.len() >= MAX_RELAY_RATE_PEERS {
                // Make room by forgetting the link that is spending least,
                // rather than by clearing the table. A wholesale reset hands
                // every neighbor a fresh burst — including the noisy one the
                // per-link limit exists to hold back, which is the entry it can
                // least afford to lose. The bucket that has recovered the most
                // tokens is the one whose absence changes the least: at full
                // capacity it is indistinguishable from a link never tracked.
                //
                // Costs a scan of the table, but only once the ceiling is
                // reached and only when a genuinely new link appears.
                for bucket in self.peer_budgets.values_mut() {
                    bucket.refill(now);
                }
                let most_recovered = self
                    .peer_budgets
                    .iter()
                    .max_by(|(_, a), (_, b)| a.tokens.total_cmp(&b.tokens))
                    .map(|(peer, _)| peer.clone());
                if let Some(peer) = most_recovered {
                    self.peer_budgets.remove(&peer);
                }
            }
            self.peer_budgets.insert(
                peer_id.to_string(),
                TokenBucket::new(self.config.peer_burst, self.config.peer_rate_per_sec, now),
            );
        }

        self.peer_budgets
            .get_mut(peer_id)
            .map(|bucket| bucket.try_take(now))
            .unwrap_or(true)
    }

    /// The delay before a queued forward is transmitted.
    ///
    /// Derived from the frame and this node's identity, so two nodes holding
    /// the same frame wait different amounts and one of them gets to cancel.
    /// The spread widens with density, because that is where the contention is.
    fn jitter_for(&self, message_id: &str, degree: usize) -> Duration {
        let min_ms = self.config.jitter_min.as_millis() as u64;
        let max_ms = (self.config.jitter_max.as_millis() as u64).max(min_ms);

        // More neighbors means more nodes racing to forward the same frame, so
        // give them more room to separate.
        let density_scale = 1 + (degree / self.config.dense_degree.max(1)) as u64;
        let span = (max_ms - min_ms).saturating_mul(density_scale).max(1);

        let hash = Self::hash_of(&[message_id, &self.local_id]);
        Duration::from_millis(min_ms + (hash % span))
    }

    /// Stable hash used to spread delays and fan-out choices.
    ///
    /// `DefaultHasher` is unseeded, so this is predictable to anyone who knows
    /// the inputs — both of which (a message id and a user id) are visible on
    /// the wire. That is accepted: knowing it buys only the ability to guess
    /// which neighbor fires first or which three links a frame takes, and every
    /// path it could steer a frame down is already bounded by the fan-out cap
    /// and the send budget. It must stay *stable*, which a randomly-seeded
    /// hasher would not be — a second copy of a frame has to pick the same
    /// neighbors, or a retransmission widens its own footprint.
    fn hash_of(parts: &[&str]) -> u64 {
        let mut hasher = DefaultHasher::new();
        for part in parts {
            part.hash(&mut hasher);
        }
        hasher.finish()
    }
}

fn priority_rank(priority: MessagePriority) -> u8 {
    match priority {
        MessagePriority::Low => 0,
        MessagePriority::Medium => 1,
        MessagePriority::High => 2,
        MessagePriority::Critical => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, UserId, TTL};

    /// Zero jitter so `admit` queues a forward that is immediately due, which
    /// keeps the tests deterministic without a clock abstraction.
    fn immediate_config() -> MeshRelayConfig {
        MeshRelayConfig {
            jitter_min: Duration::from_millis(0),
            jitter_max: Duration::from_millis(0),
            ..Default::default()
        }
    }

    fn governor() -> MeshRelayGovernor {
        MeshRelayGovernor::with_config("relay-node", immediate_config())
    }

    fn frame_with(ttl: u8, priority: MessagePriority) -> Message {
        Message::builder(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("app").unwrap(),
        )
        .content("hello")
        .priority(priority)
        .ttl(TTL::new(ttl).unwrap())
        .build()
    }

    fn frame() -> Message {
        frame_with(8, MessagePriority::Medium)
    }

    #[test]
    fn a_frame_is_forwarded_once_however_many_copies_arrive() {
        let mut gov = governor();
        let msg = frame();

        assert_eq!(gov.admit(&msg, Some("b"), 3, false), RelayAdmission::Queued);

        // Copies arriving after ours has gone out are suppressed, not forwarded.
        let due = gov.take_due(Instant::now());
        assert_eq!(due.len(), 1);

        for _ in 0..5 {
            assert_eq!(
                gov.admit(&msg, Some("c"), 3, false),
                RelayAdmission::Rejected(RelayRejection::AlreadySeen)
            );
        }
        assert_eq!(gov.counters().queued, 1);
        assert_eq!(gov.counters().duplicates_suppressed, 5);
    }

    #[test]
    fn a_neighbor_beating_us_to_it_cancels_our_pending_forward() {
        // The density control: in a cluster where everyone hears everyone, the
        // first node to transmit covers the rest, and they stand down instead
        // of each sending their own copy.
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                jitter_min: Duration::from_millis(50),
                jitter_max: Duration::from_millis(200),
                ..Default::default()
            },
        );
        let msg = frame();

        assert_eq!(gov.admit(&msg, Some("b"), 6, false), RelayAdmission::Queued);
        assert_eq!(gov.pending_len(), 1);

        // The same frame arrives again while ours is still waiting.
        assert_eq!(
            gov.admit(&msg, Some("c"), 6, false),
            RelayAdmission::Rejected(RelayRejection::SupersededByDuplicate)
        );

        assert_eq!(gov.pending_len(), 0);
        assert_eq!(gov.counters().cancelled_by_duplicate, 1);
        assert!(gov
            .take_due(Instant::now() + Duration::from_secs(1))
            .is_empty());
        assert_eq!(gov.counters().transmissions, 0);
    }

    #[test]
    fn an_inflated_hop_budget_is_cut_to_local_policy() {
        // Nothing authenticates the hop budget at this layer, so a frame can
        // claim any value. It must not buy more than one hop past our own
        // ceiling.
        let mut gov = governor();
        let msg = frame_with(255, MessagePriority::Medium);

        assert_eq!(gov.admit(&msg, Some("b"), 2, false), RelayAdmission::Queued);
        let due = gov.take_due(Instant::now());

        assert_eq!(due.len(), 1);
        assert_eq!(due[0].message.ttl.value(), DEFAULT_RELAY_MAX_TTL - 1);
        assert_eq!(gov.counters().ttl_clamped, 1);
    }

    #[test]
    fn a_dense_neighborhood_uses_the_lower_hop_ceiling() {
        let mut gov = governor();
        let msg = frame_with(255, MessagePriority::Medium);

        assert_eq!(
            gov.admit(&msg, Some("b"), DEFAULT_RELAY_DENSE_DEGREE, false),
            RelayAdmission::Queued
        );
        let due = gov.take_due(Instant::now());
        assert_eq!(due[0].message.ttl.value(), DEFAULT_RELAY_DENSE_MAX_TTL - 1);
    }

    #[test]
    fn a_frame_out_of_hops_is_not_forwarded() {
        let mut gov = governor();
        // A budget of 1 is spent: this node is the last that may hold it.
        let msg = frame_with(1, MessagePriority::Medium);

        assert_eq!(
            gov.admit(&msg, Some("b"), 3, false),
            RelayAdmission::Rejected(RelayRejection::HopLimitReached)
        );
        assert_eq!(gov.counters().hop_limit_reached, 1);
    }

    #[test]
    fn the_last_hop_is_forwarded_and_the_one_after_it_is_not() {
        let mut gov = governor();

        // Two hops left: goes out with one, which the next node will refuse to
        // forward. Verifies the budget lands exactly on `TTL::is_exhausted`
        // rather than a hop early or late.
        assert_eq!(
            gov.admit(&frame_with(2, MessagePriority::Medium), Some("b"), 3, false),
            RelayAdmission::Queued
        );
        let due = gov.take_due(Instant::now());
        assert_eq!(due[0].message.ttl.value(), 1);
        assert!(due[0].message.is_ttl_exhausted());
    }

    #[test]
    fn the_hop_ceiling_stops_a_frame_that_evaded_the_budget() {
        let mut gov = governor();
        let mut msg = frame_with(255, MessagePriority::Medium);
        for _ in 0..RELAY_MAX_HOP_COUNT {
            let _ = msg.increment_hop();
        }

        assert_eq!(
            gov.admit(&msg, Some("b"), 3, false),
            RelayAdmission::Rejected(RelayRejection::HopLimitReached)
        );
    }

    #[test]
    fn one_noisy_neighbor_cannot_consume_the_whole_node() {
        let mut gov = governor();

        // Spend the noisy neighbor's allowance on unique frames — the shape
        // suppression cannot help with, since every id is new.
        let mut accepted_from_noisy = 0;
        for _ in 0..200 {
            if gov.admit(&frame(), Some("noisy"), 3, false) == RelayAdmission::Queued {
                accepted_from_noisy += 1;
            }
        }

        assert!(
            accepted_from_noisy <= DEFAULT_RELAY_PEER_BURST as usize + 2,
            "accepted {accepted_from_noisy} frames from one neighbor"
        );
        assert!(gov.counters().peer_rate_limited > 0);

        // A different neighbor still gets through: the limit is per link, so
        // one bad actor degrades only itself.
        assert_eq!(
            gov.admit(&frame(), Some("quiet"), 3, false),
            RelayAdmission::Queued
        );
    }

    #[test]
    fn transmission_never_exceeds_the_node_budget() {
        // The invariant the whole design rests on: whatever arrives, what the
        // radio is asked to send is capped. Asserted on transmissions rather
        // than on queue releases, because one release can put a frame on
        // several links and it is the links that cost airtime.
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                queue_capacity: 512,
                // Admission is not what is under test here.
                peer_rate_per_sec: 10_000.0,
                peer_burst: 10_000.0,
                ..immediate_config()
            },
        );

        for _ in 0..200 {
            gov.admit(&frame(), Some("b"), 3, false);
        }
        let due = gov.take_due(Instant::now());
        assert!(!due.is_empty());

        // Ask for far more transmissions than the budget allows.
        let granted = (0..1000).filter(|_| gov.take_send_token()).count();

        assert!(
            granted <= DEFAULT_RELAY_BURST as usize + 1,
            "granted {granted} transmissions against a burst of {DEFAULT_RELAY_BURST}"
        );
        assert_eq!(gov.counters().transmissions as usize, granted);
        assert!(gov.counters().rate_deferred > 0);
    }

    #[test]
    fn a_frame_that_reached_no_neighbor_for_budget_goes_back_on_the_queue() {
        // `take_due` checks the budget without reserving it, so a batch of due
        // frames can be released against a single token: the first spends it
        // and the rest reach the radio with nothing left. Those frames have
        // been forwarded nowhere, and their ids are already recorded as
        // handled — dropping one would lose this copy and refuse both the
        // copies behind it and the sender's retransmissions for the whole
        // retention window. It has to go back on the queue.
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                peer_rate_per_sec: 10_000.0,
                peer_burst: 10_000.0,
                ..immediate_config()
            },
        );

        for _ in 0..40 {
            gov.admit(&frame(), Some("b"), 3, false);
        }
        let due = gov.take_due(Instant::now());
        assert!(
            due.len() > DEFAULT_RELAY_BURST as usize,
            "the batch must outrun the budget for this to be the case under test"
        );

        // Drain the budget the way a fan-out does, then hand back everything
        // that got nowhere.
        let mut requeued = 0;
        for relay in due {
            if gov.take_send_token() {
                continue;
            }
            gov.requeue(relay);
            requeued += 1;
        }

        assert!(requeued > 0, "the budget must have run out mid-batch");
        assert_eq!(gov.pending_len(), requeued, "every starved frame is kept");
        assert_eq!(gov.counters().requeued_for_budget as usize, requeued);
    }

    #[test]
    fn a_requeued_frame_is_still_abandoned_once_it_is_far_past_its_turn() {
        // Re-queueing must not become an unbounded retry: a frame kept for
        // budget keeps its original due time, so the overdue cut-off still
        // reaches it.
        let mut gov = governor();
        gov.admit(&frame(), Some("b"), 3, false);
        let mut due = gov.take_due(Instant::now());
        assert_eq!(due.len(), 1);

        gov.requeue(due.remove(0));
        assert_eq!(gov.pending_len(), 1);

        let later = gov.take_due(Instant::now() + RELAY_QUEUE_MAX_OVERDUE + Duration::from_secs(1));
        assert!(later.is_empty());
        assert_eq!(gov.counters().abandoned_overdue, 1);
        assert_eq!(gov.pending_len(), 0);
    }

    #[test]
    fn eviction_prefers_a_frame_that_has_not_yet_gone_out() {
        // A due frame is on its way to the radio this tick; taking it costs a
        // transmission that was about to happen, while the frame replacing it
        // still has to wait out its hold.
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                queue_capacity: 2,
                jitter_min: Duration::from_millis(0),
                jitter_max: Duration::from_millis(0),
                peer_rate_per_sec: 10_000.0,
                peer_burst: 10_000.0,
                ..Default::default()
            },
        );

        // One low-priority frame due immediately.
        let due_now = frame_with(8, MessagePriority::Low);
        gov.admit(&due_now, Some("b"), 3, false);

        // A second low-priority frame that is not due for a while.
        gov.config.jitter_min = Duration::from_millis(500);
        gov.config.jitter_max = Duration::from_millis(500);
        let waiting = frame_with(8, MessagePriority::Low);
        gov.admit(&waiting, Some("b"), 3, false);

        // An urgent frame needs the room.
        gov.admit(
            &frame_with(8, MessagePriority::Critical),
            Some("b"),
            3,
            false,
        );

        assert_eq!(gov.counters().queue_evicted, 1);
        assert_eq!(
            gov.counters().queue_full,
            0,
            "making room is not the same as refusing"
        );
        assert!(
            gov.pending
                .iter()
                .any(|p| p.message.id.as_str() == due_now.id.as_str()),
            "the frame already due must survive"
        );
        assert!(
            !gov.pending
                .iter()
                .any(|p| p.message.id.as_str() == waiting.id.as_str()),
            "the frame still waiting is the one displaced"
        );
    }

    #[test]
    fn a_full_peer_table_keeps_the_links_that_are_spending() {
        // Clearing the table wholesale hands every neighbor a fresh burst,
        // including the noisy one the per-link limit exists to hold back.
        let mut gov = MeshRelayGovernor::with_config("relay-node", immediate_config());

        // One neighbor spends most of its allowance.
        for _ in 0..(DEFAULT_RELAY_PEER_BURST as usize - 1) {
            gov.admit(&frame(), Some("noisy"), 3, false);
        }

        // Fill the table with links that arrive once and go quiet.
        for i in 0..MAX_RELAY_RATE_PEERS {
            gov.admit(&frame(), Some(&format!("idle-{i}")), 3, false);
        }

        assert!(gov.peer_budgets.len() <= MAX_RELAY_RATE_PEERS);
        // Still tracked at all: the entry the limit exists to hold back is the
        // one it can least afford to forget, and it never sends again here, so
        // an eviction would leave it absent rather than rebuilt.
        let noisy = gov
            .peer_budgets
            .get("noisy")
            .expect("a link mid-spend must not be forgotten to make room for idle ones");
        assert!(
            noisy.tokens < noisy.capacity,
            "the noisy link kept its spent allowance rather than being handed a fresh burst"
        );
    }

    #[test]
    fn a_refused_frame_leaves_the_way_open_for_another_copy() {
        // A frame turned away must not be remembered as handled: the copy
        // arriving over a healthy link a moment later is the alternate path,
        // and suppressing it would blank every route through this device for
        // the whole retention window.
        let mut gov = governor();

        // Refused for having no hops left — the cheapest thing an attacker can
        // forge, since nothing authenticates the claim.
        let msg = frame_with(1, MessagePriority::Medium);
        assert_eq!(
            gov.admit(&msg, Some("hostile"), 3, false),
            RelayAdmission::Rejected(RelayRejection::HopLimitReached)
        );

        // The same message, arriving intact by another route.
        let mut healthy = msg.clone();
        healthy.ttl = TTL::new(8).unwrap();
        assert_eq!(
            gov.admit(&healthy, Some("good"), 3, false),
            RelayAdmission::Queued,
            "a refused frame must not blackhole its id"
        );
    }

    #[test]
    fn a_rate_limited_neighbor_does_not_blackhole_the_ids_it_sent() {
        let mut gov = governor();

        // Exhaust one neighbor's allowance with distinct frames.
        let mut refused = Vec::new();
        for _ in 0..200 {
            let msg = frame();
            if gov.admit(&msg, Some("noisy"), 3, false)
                == RelayAdmission::Rejected(RelayRejection::PeerRateLimited)
            {
                refused.push(msg);
            }
        }
        assert!(!refused.is_empty());

        // Those same messages, reaching us through someone else, must still be
        // carried.
        let victim = &refused[0];
        assert_eq!(
            gov.admit(victim, Some("quiet"), 3, false),
            RelayAdmission::Queued
        );
    }

    #[test]
    fn the_last_hop_does_not_stand_down_for_a_neighbor() {
        // Standing down assumes a neighbor's transmission covers the same
        // ground ours would. At the last hop it does not: our copy is going to
        // the recipient over a link only we are known to hold, so dropping it
        // loses the delivery.
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                jitter_min: Duration::from_millis(50),
                jitter_max: Duration::from_millis(200),
                ..Default::default()
            },
        );
        let msg = frame();

        assert_eq!(gov.admit(&msg, Some("b"), 3, true), RelayAdmission::Queued);
        // Another forwarder hands us the same frame.
        assert_eq!(
            gov.admit(&msg, Some("c"), 3, true),
            RelayAdmission::Rejected(RelayRejection::AlreadySeen)
        );

        assert_eq!(gov.pending_len(), 1, "the delivering copy must survive");
        assert_eq!(gov.counters().cancelled_by_duplicate, 0);
    }

    #[test]
    fn a_forward_waiting_far_past_its_turn_is_abandoned() {
        let mut gov = governor();
        gov.admit(&frame(), Some("b"), 3, false);

        // Long enough that other paths have carried it or the sender has
        // retransmitted; holding it would only displace newer traffic.
        let due = gov.take_due(Instant::now() + RELAY_QUEUE_MAX_OVERDUE + Duration::from_secs(1));
        assert!(due.is_empty());
        assert_eq!(gov.counters().abandoned_overdue, 1);
        assert_eq!(gov.pending_len(), 0);
    }

    #[test]
    fn a_full_queue_refuses_rather_than_growing() {
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                queue_capacity: 4,
                jitter_min: Duration::from_millis(500),
                jitter_max: Duration::from_millis(500),
                peer_rate_per_sec: 10_000.0,
                peer_burst: 10_000.0,
                ..Default::default()
            },
        );

        for _ in 0..4 {
            assert_eq!(
                gov.admit(&frame(), Some("b"), 3, false),
                RelayAdmission::Queued
            );
        }
        assert_eq!(
            gov.admit(&frame(), Some("b"), 3, false),
            RelayAdmission::Rejected(RelayRejection::QueueFull)
        );
        assert_eq!(gov.pending_len(), 4);
    }

    #[test]
    fn a_full_queue_still_takes_an_urgent_frame_over_a_routine_one() {
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                queue_capacity: 2,
                jitter_min: Duration::from_millis(500),
                jitter_max: Duration::from_millis(500),
                peer_rate_per_sec: 10_000.0,
                peer_burst: 10_000.0,
                ..Default::default()
            },
        );

        gov.admit(&frame_with(8, MessagePriority::Low), Some("b"), 3, false);
        gov.admit(&frame_with(8, MessagePriority::Low), Some("b"), 3, false);

        assert_eq!(
            gov.admit(
                &frame_with(8, MessagePriority::Critical),
                Some("b"),
                3,
                false
            ),
            RelayAdmission::Queued
        );
        assert_eq!(gov.pending_len(), 2);
        assert!(gov
            .take_due(Instant::now() + Duration::from_secs(1))
            .iter()
            .any(|r| r.message.priority == MessagePriority::Critical));
    }

    #[test]
    fn a_frame_is_never_sent_back_where_it_came_from() {
        let gov = governor();
        let targets = gov.select_targets(
            [("alice", 80), ("b", 80), ("c", 80), ("d", 80)],
            &["b", "alice"], // arrival neighbor and the peer that wrote it
            "msg-1",
        );

        assert!(!targets.contains(&"b".to_string()));
        assert!(!targets.contains(&"alice".to_string()));
        assert!(targets.contains(&"c".to_string()));
        assert!(targets.contains(&"d".to_string()));
    }

    #[test]
    fn fanout_is_capped_and_stable_for_a_given_frame() {
        let gov = governor();
        // All equally good, so the tie-break is what decides — which is the
        // property under test.
        let neighbors = [
            ("n0", 70),
            ("n1", 70),
            ("n2", 70),
            ("n3", 70),
            ("n4", 70),
            ("n5", 70),
            ("n6", 70),
            ("n7", 70),
        ];

        let first = gov.select_targets(neighbors, &[], "msg-1");
        let again = gov.select_targets(neighbors, &[], "msg-1");

        assert_eq!(first.len(), DEFAULT_RELAY_FANOUT);
        assert_eq!(first, again, "a second copy must pick the same neighbors");

        // Different frames spread across different neighbors rather than
        // hammering one set.
        let other = gov.select_targets(neighbors, &[], "msg-2");
        assert_eq!(other.len(), DEFAULT_RELAY_FANOUT);
    }

    #[test]
    fn a_frame_we_consumed_ourselves_is_not_forwarded_for_someone_else() {
        let mut gov = governor();
        let msg = frame();

        gov.mark_handled(&msg.id.as_str());

        assert_eq!(
            gov.admit(&msg, Some("b"), 3, false),
            RelayAdmission::Rejected(RelayRejection::AlreadySeen)
        );
    }

    #[test]
    fn the_suppression_cache_holds_a_saturated_node_for_its_full_window() {
        // Capacity has to outlast in-flight copies at the maximum rate the node
        // will accept traffic. If it did not, an id could be forgotten while
        // copies were still circulating and be forwarded a second time — by
        // every node, which is how a flood becomes a storm.
        let gov = governor();
        let max_admissions_per_sec = DEFAULT_RELAY_PEER_RATE_PER_SEC * MAX_RELAY_RATE_PEERS as f32;
        let sustained_ceiling = DEFAULT_RELAY_RATE_PER_SEC.min(max_admissions_per_sec);
        let ids_in_window = sustained_ceiling * gov.config().seen.retention.as_secs_f32();

        assert!(
            (gov.config().seen.capacity as f32) >= ids_in_window,
            "cache holds {} ids but a saturated node sees {} within the retention window",
            gov.config().seen.capacity,
            ids_in_window
        );
    }
}
