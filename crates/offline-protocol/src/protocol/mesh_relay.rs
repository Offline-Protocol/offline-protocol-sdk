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
//!
//! On top of those four sits **capability bias**: how willing this particular
//! device is, right now, to spend itself on other people's traffic. A charging
//! laptop and a phone at 20% are both allowed to forward, and the mechanisms
//! above treat them identically — so the phone pays the same share as the
//! laptop. Bias tilts three of the existing dials by battery and charging
//! state, continuously: a weaker device waits longer before transmitting (so a
//! stronger neighbor holding the same frame wins the cancellation race and it
//! stands down having spent nothing), forwards to fewer neighbors, and refills
//! its send budget more slowly.
//!
//! This is deliberately a *bias* and not a role. A threshold that switches
//! forwarding on and off makes the network's shape depend on a state machine,
//! and the failure mode of getting that wrong is a partition — devices that
//! could carry a frame declining to. Scaling instead means every device
//! forwards, capable ones simply forward first and more, and the worst a
//! misjudged scale costs is some redundancy. The relay *role* reported to apps
//! is derived the other way round, from what this device has actually carried
//! ([`MeshRelayGovernor::observe_activity`]), so it describes behaviour rather
//! than predicting it.

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

/// Transmissions held back from carrying other people's traffic so this device
/// can always get its own frames out.
///
/// Own traffic and forwarded traffic share one budget, because they share one
/// radio. Without a reserve they also share one queue discipline, and a device
/// forwarding at its ceiling would starve its *own* sends behind strangers'
/// — including the delivery acknowledgement for a message it just received,
/// whose absence makes a delivered message look lost. Carving the reserve out
/// of the burst rather than adding a second bucket keeps the ceiling exactly
/// where it was: this changes who may spend the last few tokens, not how many
/// there are.
///
/// It costs forwarding no throughput at saturation. Tokens refill continuously,
/// so a forward simply waits for the level to climb back above the reserve —
/// which at the sustained rate is a fraction of a second.
pub const RELAY_OWN_TRAFFIC_RESERVE: f32 = 5.0;

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
///
/// Public because it bounds what the delay tunables may be set to: a jitter
/// window plus a bias handicap that could exceed it would turn every biased
/// forward into an abandoned one, which `ProtocolConfig::validate` refuses.
pub const RELAY_QUEUE_MAX_OVERDUE: Duration = Duration::from_secs(5);

/// Smallest share of the full forwarding effort a device is scaled down to by
/// capability bias.
///
/// A floor rather than zero, because bias must never become an off switch:
/// a device at 1% battery that is still allowed to forward at all (the
/// [`RelayConfig::min_battery_for_relay`] gate is what decides that, not this)
/// keeps a usable share, so a network of uniformly low devices still carries
/// traffic instead of quietly ceasing to be a network.
///
/// [`RelayConfig::min_battery_for_relay`]: offline_protocol_router::RelayConfig::min_battery_for_relay
pub const DEFAULT_RELAY_BIAS_MIN_SCALE: f32 = 0.25;

/// Longest extra delay capability bias adds before a weaker device transmits.
///
/// This is the whole mechanism by which a stronger neighbor gets to cover a
/// frame first: the weaker device's delay window opens later, so in the common
/// case it is still holding the frame when the neighbor's copy arrives and it
/// stands down having spent no airtime at all.
///
/// A fixed ceiling rather than a multiple of the jitter span, and that matters:
/// the span already widens with density, and compounding the two would push a
/// weak device in a crowded room past [`RELAY_QUEUE_MAX_OVERDUE`], where its
/// forwards are abandoned rather than merely late. Bias must cost redundancy,
/// never delivery.
pub const DEFAULT_RELAY_BIAS_MAX_HANDICAP_MS: u64 = 400;

/// How long a stretch of forwarding activity is measured over.
pub const DEFAULT_RELAY_ACTIVITY_WINDOW_SECS: u64 = 60;

/// Frames carried for other people within one window at or above which this
/// device reads as an active relay.
pub const DEFAULT_RELAY_ACTIVITY_MIN_FORWARDS: u64 = 3;

/// Consecutive quiet windows before an active relay reads as inactive again.
///
/// Asymmetric with promotion on purpose: becoming a relay takes one busy
/// window, ceasing to be one takes several quiet ones. A relay in a mesh that
/// simply has nothing to say for a minute has not stopped being a relay, and
/// reporting that it has would produce churn no app can act on.
pub const DEFAULT_RELAY_ACTIVITY_IDLE_WINDOWS: u32 = 2;

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
    /// Smallest share of the full forwarding effort capability bias scales a
    /// device down to. `1.0` disables bias: every device forwards as eagerly
    /// as every other, whatever its battery.
    pub bias_min_scale: f32,
    /// Longest extra pre-transmit delay bias adds to a weaker device.
    pub bias_max_handicap: Duration,
    /// How long a stretch of forwarding activity is measured over.
    pub activity_window: Duration,
    /// Frames carried within one window at or above which this device reads as
    /// an active relay.
    pub activity_min_forwards: u64,
    /// Consecutive quiet windows before an active relay reads as inactive.
    pub activity_idle_windows: u32,
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
            bias_min_scale: DEFAULT_RELAY_BIAS_MIN_SCALE,
            bias_max_handicap: Duration::from_millis(DEFAULT_RELAY_BIAS_MAX_HANDICAP_MS),
            activity_window: Duration::from_secs(DEFAULT_RELAY_ACTIVITY_WINDOW_SECS),
            activity_min_forwards: DEFAULT_RELAY_ACTIVITY_MIN_FORWARDS,
            activity_idle_windows: DEFAULT_RELAY_ACTIVITY_IDLE_WINDOWS,
        }
    }
}

/// This device's own condition, as far as forwarding is concerned.
///
/// Pushed in from the engine each tick rather than read here, because the
/// battery reading belongs to the host and reaches the SDK through
/// `set_battery_state` — the governor holds no transports and asks nothing.
#[derive(Debug, Clone, Copy, Default)]
struct RelayConditions {
    /// Host-reported battery percentage, or `None` while nothing has reported
    /// one.
    battery: Option<u8>,
    /// Whether the device is plugged in.
    is_charging: bool,
    /// Whether the configuration asks this device to relay eagerly
    /// (`RelayPriority::Always`).
    eager: bool,
    /// The soft battery floor forwarding is gated on, which is also where the
    /// bias ramp starts.
    soft_floor: u8,
}

/// A change in whether this device is carrying traffic for other people.
///
/// Returned by [`MeshRelayGovernor::observe_activity`] only at the tick the
/// answer changes, so a caller can announce it exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayActivity {
    /// This device started carrying third-party traffic.
    Began {
        /// Frames carried in the window that triggered this.
        forwarded: u64,
    },
    /// This device stopped carrying third-party traffic.
    Ceased,
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
    /// Frames put back on the queue because they reached no neighbor at all —
    /// the budget ran out, the links chosen for them went away, or there was
    /// no usable link to choose.
    pub requeued: u64,
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
/// A snapshot, safe to poll. Every field is cumulative since start-up except
/// [`Self::awaiting_transmission`], which is a gauge: it is the queue depth
/// right now, and it goes down as well as up.
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
    /// Frames refused admission, or refused room on their way back to the
    /// queue, because the pending queue was full.
    ///
    /// This is a real loss: the frame reached nobody, and only a copy behind
    /// it or the sender's own retransmission will carry it now. A rising value
    /// means forwards are arriving faster than this device can transmit them,
    /// so the queue is the thing to raise, or the rate ceiling that is filling
    /// it — see [`Self::rate_deferred`].
    pub refused_queue_full: u64,
    /// Transmissions held back because this device was at its forwarding rate.
    ///
    /// A frame counted here is delayed, not dropped: it goes back on the queue
    /// and is tried again on the next tick, until it is either sent or has
    /// waited so long past its turn that carrying it no longer helps.
    pub rate_deferred: u64,
    /// Queued forwards given up on after waiting too long past their due time.
    ///
    /// The other end of [`Self::rate_deferred`]: this is where a frame that
    /// kept being delayed finally stops being carried. Deferral is free to
    /// look healthy while this climbs, so it is the pair that says whether
    /// back-pressure is costing anything.
    pub abandoned_overdue: u64,
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

    /// Re-sizes the allowance in place, keeping what has been spent spent.
    ///
    /// Refills against the *old* rate first, so the time already elapsed is
    /// credited at the terms that applied to it, then clamps the level to the
    /// new capacity. Clamping is what makes this safe to call every tick: a
    /// device whose bias improves gets a larger bucket to refill into, and one
    /// whose bias worsens gets a smaller one immediately, but neither is handed
    /// tokens it did not earn — otherwise a value oscillating around a
    /// threshold would mint a fresh burst on every crossing.
    fn resize(&mut self, capacity: f32, refill_per_sec: f32, now: Instant) {
        self.refill(now);
        self.capacity = capacity.max(0.0);
        self.refill_per_sec = refill_per_sec.max(0.0);
        self.tokens = self.tokens.min(self.capacity);
    }

    /// Spends a token, but only while more than `floor` would remain
    /// untouched. A `floor` of zero spends down to empty.
    fn try_take_above(&mut self, floor: f32, now: Instant) -> bool {
        self.refill(now);
        if self.tokens >= 1.0 + floor {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Whether a token could be spent right now leaving `floor` untouched,
    /// without spending it.
    ///
    /// For a caller that must satisfy two meters at once and may spend neither
    /// unless both allow it.
    fn has_above(&mut self, floor: f32, now: Instant) -> bool {
        self.refill(now);
        self.tokens >= 1.0 + floor
    }

    /// Spends a token already confirmed available by [`Self::has_above`].
    fn spend(&mut self) {
        self.tokens = (self.tokens - 1.0).max(0.0);
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
    /// This device's whole airtime ceiling — every frame it transmits, its own
    /// included. Never scaled by capability bias: it is what the device is
    /// allowed to say, and bias is about how much of that it spends on other
    /// people.
    send_budget: TokenBucket,
    /// The share of [`Self::send_budget`] capability bias leaves for forwarding.
    ///
    /// A second meter rather than a smaller shared bucket, and the distinction
    /// is the whole point: scaling the shared bucket would throttle this
    /// device's *own* messages and acknowledgements in step with its battery,
    /// which is delivery rather than redundancy. Forwarding must satisfy both
    /// meters; own traffic answers only to the shared one. At full capability
    /// this one matches the shared bucket and never binds first, so a capable
    /// device behaves exactly as it did before bias existed.
    forward_budget: TokenBucket,
    /// Tokens of [`Self::send_budget`] that only this device's own frames may
    /// spend. Clamped to half the burst so a small configured burst cannot
    /// reserve the whole bucket and stop forwarding outright.
    own_reserve: f32,
    peer_budgets: HashMap<String, TokenBucket>,
    counters: MeshRelayCounters,
    /// Seeds the per-frame delay so two nodes holding the same frame pick
    /// different delays.
    local_id: String,
    /// This device's condition, as last reported by the host.
    conditions: RelayConditions,
    /// Whether this device currently reads as carrying traffic for others.
    active_relay: bool,
    /// Start of the activity window in progress.
    window_started_at: Instant,
    /// Frames carried for other people within the window in progress.
    window_forwards: u64,
    /// Consecutive completed windows in which nothing was carried.
    idle_windows: u32,
}

impl MeshRelayGovernor {
    /// Creates a governor with explicit tunables.
    pub fn with_config(local_id: impl Into<String>, config: MeshRelayConfig) -> Self {
        let now = Instant::now();
        let seen = RelaySeenCache::with_config(config.seen.clone());
        Self {
            send_budget: TokenBucket::new(config.burst, config.rate_per_sec, now),
            forward_budget: TokenBucket::new(config.burst, config.rate_per_sec, now),
            own_reserve: RELAY_OWN_TRAFFIC_RESERVE.min(config.burst.max(0.0) / 2.0),
            seen,
            pending: Vec::new(),
            peer_budgets: HashMap::new(),
            counters: MeshRelayCounters::default(),
            local_id: local_id.into(),
            conditions: RelayConditions::default(),
            active_relay: false,
            window_started_at: now,
            window_forwards: 0,
            idle_windows: 0,
            config,
        }
    }

    /// Re-points the governor at this device's real identity once it is known.
    ///
    /// The governor is built before `initialize_mls` derives the address, so it
    /// starts out seeded with the profile. Both uses are hashes — fan-out
    /// spread and send jitter — so the change is only a reseed: it shifts which
    /// frames this node prefers, never whether a frame is relayed.
    pub fn set_local_id(&mut self, local_id: impl Into<String>) {
        self.local_id = local_id.into();
    }

    /// The tunables actually in force.
    ///
    /// This is the governor's own snapshot, taken at construction, and it is
    /// what every forwarding decision reads. Reporting it rather than
    /// `ProtocolConfig::mesh_relay` keeps the answer honest: the two agree
    /// today only because nothing can update the section after construction,
    /// and a reader that trusted the config copy would start lying the moment
    /// that changed.
    pub fn config(&self) -> &MeshRelayConfig {
        &self.config
    }

    /// Reports this device's condition, which scales how eagerly it forwards.
    ///
    /// Called from the process tick with the host's battery feed. Everything it
    /// changes is continuous — delay, fan-out, refill rate — so there is no
    /// value of any argument that stops this device forwarding. Whether it may
    /// forward at all is the caller's gate (`RelayConfig::allow_relay` and the
    /// battery floor), deliberately kept out of here so the two decisions
    /// cannot half-apply.
    ///
    /// `battery` of `None` means the host has reported nothing, which is
    /// treated as fully capable for the same reason the forwarding gate treats
    /// it as willing: most platforms report a level, and penalising the ones
    /// that do not would quietly thin the network on those devices.
    ///
    /// Applied to the forwarding budget immediately rather than on next use, so
    /// a device that has just been unplugged stops refilling at the plugged-in
    /// rate this tick.
    ///
    /// Only the *forwarding* share is scaled. This device's own airtime ceiling
    /// is left alone, so its messages and acknowledgements go out at the same
    /// rate on a dying phone as on a charging laptop — bias decides how much of
    /// the room's traffic it carries, never how well it can speak for itself.
    pub fn set_conditions(
        &mut self,
        battery: Option<u8>,
        is_charging: bool,
        eager: bool,
        soft_floor: u8,
    ) {
        self.conditions = RelayConditions {
            battery,
            is_charging,
            eager,
            soft_floor,
        };
        let scale = self.bias_scale();
        self.forward_budget.resize(
            self.config.burst * scale,
            self.config.rate_per_sec * scale,
            Instant::now(),
        );
    }

    /// How much of the full forwarding effort this device is currently willing
    /// to spend, in `[bias_min_scale, 1.0]`.
    ///
    /// Fully capable when charging, when configured to relay eagerly, or when
    /// no battery reading exists. Otherwise it ramps linearly from the soft
    /// floor — the level at which forwarding would stop altogether — up to a
    /// full battery, so the scale is a measure of headroom above the floor
    /// rather than of the raw percentage.
    fn bias_scale(&self) -> f32 {
        let min_scale = self.config.bias_min_scale.clamp(0.0, 1.0);
        if self.conditions.is_charging || self.conditions.eager {
            return 1.0;
        }
        let Some(level) = self.conditions.battery else {
            return 1.0;
        };
        let floor = self.conditions.soft_floor.min(100);
        if level >= 100 {
            return 1.0;
        }
        if level <= floor {
            return min_scale;
        }
        let headroom = (level - floor) as f32 / (100 - floor).max(1) as f32;
        min_scale + headroom * (1.0 - min_scale)
    }

    /// Whether this device currently reads as carrying traffic for other
    /// people.
    ///
    /// Derived from what it has actually forwarded, not from what its battery
    /// and neighbor count suggest it could — see [`Self::observe_activity`].
    pub fn is_active_relay(&self) -> bool {
        self.active_relay
    }

    /// Rolls the activity window and reports a change in relay activity.
    ///
    /// Answers "is this device carrying other people's traffic" from the only
    /// evidence that cannot be wrong about it: whether it has. A device becomes
    /// a relay by having carried [`MeshRelayConfig::activity_min_forwards`]
    /// frames within one window, and stops being one after
    /// [`MeshRelayConfig::activity_idle_windows`] consecutive windows carrying
    /// none.
    ///
    /// Returns `Some` only at the tick the answer changes, so the caller emits
    /// one event per transition. Cheap enough for every tick: it does nothing
    /// at all until a window has elapsed.
    pub fn observe_activity(&mut self, now: Instant) -> Option<RelayActivity> {
        if now.saturating_duration_since(self.window_started_at) < self.config.activity_window {
            return None;
        }

        let forwarded = std::mem::take(&mut self.window_forwards);
        self.window_started_at = now;

        if forwarded >= self.config.activity_min_forwards.max(1) {
            self.idle_windows = 0;
            if !self.active_relay {
                self.active_relay = true;
                return Some(RelayActivity::Began { forwarded });
            }
            return None;
        }

        if !self.active_relay {
            return None;
        }

        self.idle_windows = self.idle_windows.saturating_add(1);
        if self.idle_windows >= self.config.activity_idle_windows.max(1) {
            self.active_relay = false;
            return Some(RelayActivity::Ceased);
        }
        None
    }

    /// Drops the relay standing immediately, reporting whether it was held.
    ///
    /// For the caller's own gates closing — relaying switched off in
    /// configuration, or the battery falling through the forwarding floor.
    /// Those stop forwarding outright, so waiting out the idle windows would
    /// leave this device reported as a relay for a minute after it had stopped
    /// being one; the activity window is a measure of quiet, not of refusal.
    pub fn force_inactive(&mut self) -> bool {
        self.window_forwards = 0;
        self.idle_windows = 0;
        std::mem::replace(&mut self.active_relay, false)
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
                // Never made it onto a link, so its id must not stay
                // suppressed: the sender's retransmissions carry the same id,
                // and refusing them would close this route for the whole
                // retention window over a few seconds of congestion.
                self.seen.forget(&relay.message.id.as_str());
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

    /// Whether any budget remains to forward right now.
    ///
    /// Measured against both meters [`Self::take_send_token`] must satisfy, so
    /// `take_due` does not release a batch the flush is about to refuse.
    fn has_send_budget(&mut self, now: Instant) -> bool {
        self.send_budget.has_above(self.own_reserve, now) && self.forward_budget.has_above(0.0, now)
    }

    /// Claims budget for putting one **forwarded** frame on one link.
    ///
    /// Satisfies two meters. The shared ceiling, stopping short of the
    /// own-traffic reserve so this device can still send its own frames while
    /// forwarding at its limit (see [`RELAY_OWN_TRAFFIC_RESERVE`] and
    /// [`Self::take_own_send_token`]); and the capability-scaled forwarding
    /// share, which is how bias reduces what a weak device carries without
    /// touching what it can say for itself.
    ///
    /// Neither is spent unless both allow it, so a refusal by one cannot leak
    /// a token out of the other.
    pub fn take_send_token(&mut self) -> bool {
        let now = Instant::now();
        if !self.has_send_budget(now) {
            self.counters.rate_deferred = self.counters.rate_deferred.saturating_add(1);
            return false;
        }
        self.send_budget.spend();
        self.forward_budget.spend();
        self.counters.transmissions = self.counters.transmissions.saturating_add(1);
        true
    }

    /// Claims budget for putting one of **this device's own** frames on one
    /// link.
    ///
    /// Metered against the same ceiling — it is the same radio — but allowed
    /// into the reserve, so a device forwarding at its limit can still get its
    /// own messages and acknowledgements out.
    ///
    /// Untouched by capability bias, which lives on the forwarding meter alone:
    /// a device at 10% battery hands its own messages to the mesh exactly as
    /// fast as a charging one, and only its share of everyone else's traffic
    /// shrinks.
    pub fn take_own_send_token(&mut self) -> bool {
        self.claim_transmission(0.0)
    }

    fn claim_transmission(&mut self, floor: f32) -> bool {
        if self.send_budget.try_take_above(floor, Instant::now()) {
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
        self.window_forwards = self.window_forwards.saturating_add(1);
    }

    /// Puts a released forward back on the queue, keeping its original due
    /// time.
    ///
    /// For the frame that reached no neighbor at all between [`Self::take_due`]
    /// releasing it and the radio being asked — because the budget ran out,
    /// because every link picked for it had gone away, or because there was no
    /// usable link to pick. All three end the same way: the frame has travelled
    /// nowhere, its id is already recorded as handled, and dropping it would
    /// refuse the copies and retransmissions behind it for the whole retention
    /// window.
    ///
    /// It keeps its due time deliberately: the overdue cut-off in `take_due` is
    /// measured from that, so a frame that stays stuck is eventually abandoned
    /// instead of being retried forever.
    ///
    /// Refused only if the queue has filled meanwhile, which is the same
    /// bound every other queued frame is subject to.
    pub fn requeue(&mut self, relay: PendingRelay) {
        if self.pending.len() >= self.config.queue_capacity {
            // Refused room on the way back, so this frame is being dropped
            // having reached nobody. Release its id for the same reason the
            // overdue cut-off does — a copy behind it, or the sender's own
            // retransmission, is now the only way it travels.
            self.seen.forget(&relay.message.id.as_str());
            self.counters.queue_full = self.counters.queue_full.saturating_add(1);
            return;
        }
        self.counters.requeued = self.counters.requeued.saturating_add(1);
        self.pending.push(relay);
    }

    /// Records an id as handled without queueing a forward for it.
    ///
    /// Used for frames this device **originates** and hands to its neighbors:
    /// a copy circling back to us through the mesh is then recognized rather
    /// than carried onward as though it were someone else's.
    ///
    /// Deliberately not called for frames addressed to us. A later copy of one
    /// of those is still addressed to us, so it never reaches the forwarding
    /// path at all — recording it would only fill the cache at delivery rate,
    /// which the capacity is not sized for and which would make
    /// [`Self::seen_capacity_evictions`] climb for reasons unrelated to relay
    /// load.
    pub fn mark_handled(&mut self, message_id: &str) {
        self.seen.observe(message_id);
    }

    /// Absorbs an arrival that is a copy of a frame already dealt with and
    /// needs no further decision, reporting whether it did.
    ///
    /// This is [`Self::admit`]'s suppression check, split out so a caller can
    /// reach it without first assembling the arguments `admit` needs. In a
    /// dense neighborhood most third-party arrivals are copies, and the caller's
    /// `degree` and `is_last_hop` cost a status snapshot across every transport
    /// and an enumeration of every link — work the suppression check would only
    /// throw away.
    ///
    /// Deliberately answers `false` while we still hold a pending copy of the
    /// id, even though that is also a duplicate: standing down for a neighbor
    /// is the one duplicate outcome that depends on the neighbor set, so it has
    /// to go through the full path. Those are rare — the pending window is one
    /// jitter delay wide.
    pub fn absorb_settled_duplicate(&mut self, message_id: &str) -> bool {
        if !self.seen.contains(message_id) {
            return false;
        }
        if self
            .pending
            .iter()
            .any(|p| p.message.id.as_str() == message_id)
        {
            return false;
        }
        self.counters.duplicates_suppressed = self.counters.duplicates_suppressed.saturating_add(1);
        true
    }

    /// Neighbors a **forwarded** frame should be carried to, given the current
    /// neighbor set and the peers it must not go back to.
    ///
    /// Excludes the neighbor it arrived from and the peer that wrote it —
    /// both already have it, and returning it is how a frame ends up bouncing
    /// between two nodes.
    ///
    /// Capability-biased: a weaker device spends fewer of its neighbors on
    /// other people's traffic. That is safe here precisely because the frame is
    /// someone else's — neighbors hold copies of it and its sender is still
    /// retrying, so a narrower fan-out costs redundancy. Frames this device
    /// originated have neither, and go through
    /// [`Self::select_origination_targets`] instead.
    pub fn select_targets<'a>(
        &self,
        neighbors: impl IntoIterator<Item = (&'a str, u8)>,
        exclude: &[&str],
        message_id: &str,
    ) -> Vec<String> {
        self.select_within_fanout(neighbors, exclude, message_id, self.biased_fanout())
    }

    /// Neighbors one of **this device's own** frames should be handed to when
    /// no transport reaches the recipient directly.
    ///
    /// Deliberately *not* capability-biased, for the same reason the jitter
    /// handicap skips this path (see [`Self::jitter_for`]): nobody else is
    /// holding this frame. For a forward, a narrower fan-out drops paths that
    /// neighbors' copies duplicate; here the fan-out *is* the delivery attempt,
    /// so narrowing it would cost delivery rather than redundancy — and because
    /// target choice is stable per message id, every retransmission would funnel
    /// into the same reduced set rather than eventually trying another.
    ///
    /// Bias still reaches this device's own traffic through the send budget's
    /// forwarding share, which is the one dial that cannot mistake the two: see
    /// [`Self::take_own_send_token`].
    pub fn select_origination_targets<'a>(
        &self,
        neighbors: impl IntoIterator<Item = (&'a str, u8)>,
        exclude: &[&str],
        message_id: &str,
    ) -> Vec<String> {
        self.select_within_fanout(neighbors, exclude, message_id, self.config.fanout.max(1))
    }

    /// The fan-out width for other people's traffic.
    ///
    /// Never fewer than one: a fan-out of zero is not a cheaper forward, it is
    /// a silent drop, and this device has already taken the frame on.
    fn biased_fanout(&self) -> usize {
        ((self.config.fanout as f32 * self.bias_scale()).round() as usize).max(1)
    }

    fn select_within_fanout<'a>(
        &self,
        neighbors: impl IntoIterator<Item = (&'a str, u8)>,
        exclude: &[&str],
        message_id: &str,
        fanout: usize,
    ) -> Vec<String> {
        let mut candidates: Vec<(&str, u8)> = neighbors
            .into_iter()
            .filter(|(peer, _)| !exclude.contains(peer))
            .collect();

        if candidates.len() <= fanout {
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
            .take(fanout)
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
                let displaced = self.pending.remove(index);
                // Displaced before it ever reached a link. Its id goes back to
                // being unknown, so the copy behind it — or the sender's
                // retransmission — can still be carried by this device once
                // there is room again.
                self.seen.forget(&displaced.message.id.as_str());
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
            .map(|bucket| bucket.try_take_above(0.0, now))
            .unwrap_or(true)
    }

    /// The delay before a queued forward is transmitted.
    ///
    /// Derived from the frame and this node's identity, so two nodes holding
    /// the same frame wait different amounts and one of them gets to cancel.
    /// The spread widens with density, because that is where the contention is.
    ///
    /// Capability shifts the whole window later rather than widening it, so
    /// between two neighbors holding the same frame the capable one usually
    /// transmits and the weaker one stands down having spent nothing. That is
    /// the point — the saving is the forward that never happens, not a cheaper
    /// one.
    ///
    /// How reliably it wins depends on density, and the handicap is a fixed
    /// constant precisely so it cannot chase it. Below [`dense_degree`] the
    /// span is narrower than the handicap, so the weak device's earliest delay
    /// is later than the capable one's latest and the ordering is absolute.
    /// Past that the span grows and the two windows overlap, leaving a
    /// probabilistic bias — the capable device still wins most frames, and the
    /// weak one's wait stays bounded by one handicap rather than growing with
    /// the room. Widening the handicap to restore the absolute ordering is
    /// exactly the shape that would push a weak device in a crowded room past
    /// [`RELAY_QUEUE_MAX_OVERDUE`], where forwards are abandoned rather than
    /// merely late.
    ///
    /// Frames this device originated are not delayed at all — see
    /// [`Self::select_origination_targets`] and the caller in `send.rs`.
    ///
    /// [`dense_degree`]: MeshRelayConfig::dense_degree
    fn jitter_for(&self, message_id: &str, degree: usize) -> Duration {
        let min_ms = self.config.jitter_min.as_millis() as u64;
        let max_ms = (self.config.jitter_max.as_millis() as u64).max(min_ms);

        // More neighbors means more nodes racing to forward the same frame, so
        // give them more room to separate.
        let density_scale = 1 + (degree / self.config.dense_degree.max(1)) as u64;
        let span = (max_ms - min_ms).saturating_mul(density_scale).max(1);

        // Bounded by a constant rather than by the span, which already grows
        // with density: compounding the two would push a weak device in a
        // crowded room past the overdue cut-off, turning late forwards into
        // abandoned ones.
        let handicap_ms = ((1.0 - self.bias_scale()).clamp(0.0, 1.0)
            * self.config.bias_max_handicap.as_millis() as f32) as u64;

        let hash = Self::hash_of(&[message_id, &self.local_id]);
        Duration::from_millis(min_ms + handicap_ms + (hash % span))
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
        assert_eq!(gov.counters().requeued as usize, requeued);
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
    fn a_frame_abandoned_for_waiting_too_long_releases_its_id() {
        // A frame dropped for waiting past its turn never reached a link, so
        // nothing is circulating that a second forward could duplicate. Its id
        // must go back to being unknown, or the sender's retransmissions —
        // which carry the same id — are refused for the whole retention window
        // because of a few seconds of congestion.
        let mut gov = governor();
        let msg = frame();
        gov.admit(&msg, Some("b"), 3, false);

        let due = gov.take_due(Instant::now() + RELAY_QUEUE_MAX_OVERDUE + Duration::from_secs(1));
        assert!(due.is_empty());
        assert_eq!(gov.counters().abandoned_overdue, 1);

        assert_eq!(
            gov.admit(&msg, Some("c"), 3, false),
            RelayAdmission::Queued,
            "the sender's retransmission must still be carried"
        );
    }

    #[test]
    fn a_frame_displaced_from_the_queue_releases_its_id() {
        // Same rule for a frame evicted to make room for an urgent one: it was
        // never transmitted, so the route through this device must stay open.
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                queue_capacity: 1,
                peer_rate_per_sec: 10_000.0,
                peer_burst: 10_000.0,
                ..immediate_config()
            },
        );

        let routine = frame_with(8, MessagePriority::Low);
        gov.admit(&routine, Some("b"), 3, false);
        gov.admit(
            &frame_with(8, MessagePriority::Critical),
            Some("b"),
            3,
            false,
        );
        assert_eq!(gov.counters().queue_evicted, 1);

        // The urgent frame goes out, leaving room again. A later copy of the
        // displaced one — or the sender's retransmission — must be carryable.
        gov.take_due(Instant::now());
        assert_eq!(
            gov.admit(&routine, Some("c"), 3, false),
            RelayAdmission::Queued,
            "the displaced frame's id must not be blackholed"
        );
    }

    #[test]
    fn a_frame_refused_room_on_the_way_back_releases_its_id() {
        // The third drop that happens after the id was recorded: a frame that
        // reached no neighbor and finds the queue full when handed back.
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                queue_capacity: 1,
                peer_rate_per_sec: 10_000.0,
                peer_burst: 10_000.0,
                ..immediate_config()
            },
        );

        let starved = frame();
        gov.admit(&starved, Some("b"), 3, false);
        let mut due = gov.take_due(Instant::now());
        assert_eq!(due.len(), 1);

        // The queue fills while the frame is out being transmitted, so there is
        // no room for it on the way back.
        gov.admit(&frame(), Some("b"), 3, false);
        gov.requeue(due.remove(0));
        assert_eq!(gov.counters().queue_full, 1);
        assert_eq!(gov.pending_len(), 1, "the dropped frame is not held");

        // Once the queue drains, the copy behind it must still be carryable.
        gov.take_due(Instant::now());
        assert_eq!(
            gov.admit(&starved, Some("c"), 3, false),
            RelayAdmission::Queued,
            "a frame that got nowhere and could not be kept must stay carryable"
        );
    }

    #[test]
    fn forwarding_leaves_room_for_this_devices_own_traffic() {
        // One radio, one budget — but a device saturated with other people's
        // traffic must still be able to send its own, above all the delivery
        // acknowledgement whose absence makes a delivered message look lost.
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                queue_capacity: 512,
                peer_rate_per_sec: 10_000.0,
                peer_burst: 10_000.0,
                ..immediate_config()
            },
        );

        for _ in 0..200 {
            gov.admit(&frame(), Some("b"), 3, false);
        }
        gov.take_due(Instant::now());

        // Drain the budget the way a saturated forwarder does.
        let forwarded = (0..1000).filter(|_| gov.take_send_token()).count();
        assert!(forwarded > 0, "forwarding must get its share first");
        assert!(
            !gov.take_send_token(),
            "forwarding must stop at the reserve, not at empty"
        );

        // The reserve is still there for us.
        let own = (0..1000).filter(|_| gov.take_own_send_token()).count();
        assert!(
            own >= RELAY_OWN_TRAFFIC_RESERVE as usize,
            "own traffic got {own} transmissions against a reserve of {RELAY_OWN_TRAFFIC_RESERVE}"
        );

        // And the ceiling still holds across both: the reserve is carved out of
        // the burst, not added to it.
        assert!(
            (forwarded + own) <= DEFAULT_RELAY_BURST as usize + 1,
            "{forwarded} forwarded + {own} own exceeds the burst of {DEFAULT_RELAY_BURST}"
        );
    }

    #[test]
    fn a_small_configured_burst_still_forwards() {
        // The reserve is clamped to half the burst, so a device configured with
        // a burst smaller than the reserve does not silently stop forwarding
        // altogether.
        let mut gov = MeshRelayGovernor::with_config(
            "relay-node",
            MeshRelayConfig {
                burst: 2.0,
                rate_per_sec: 1.0,
                peer_rate_per_sec: 10_000.0,
                peer_burst: 10_000.0,
                ..immediate_config()
            },
        );

        gov.admit(&frame(), Some("b"), 3, false);
        assert!(
            !gov.take_due(Instant::now()).is_empty(),
            "a small burst must still release forwards"
        );
        assert!(gov.take_send_token(), "and still allow transmitting them");
    }

    #[test]
    fn a_settled_duplicate_is_absorbed_without_the_full_admission_path() {
        let mut gov = governor();
        let msg = frame();

        assert!(
            !gov.absorb_settled_duplicate(&msg.id.as_str()),
            "an id never seen is not a duplicate"
        );

        gov.admit(&msg, Some("b"), 3, false);
        // While our copy is still pending, standing down is a decision that
        // needs the neighbor set, so the cheap path must decline to make it.
        assert!(!gov.absorb_settled_duplicate(&msg.id.as_str()));

        // Once it has gone out, further copies are settled duplicates.
        gov.take_due(Instant::now());
        assert!(gov.absorb_settled_duplicate(&msg.id.as_str()));
        assert_eq!(gov.counters().duplicates_suppressed, 1);
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
    fn a_frame_we_originated_is_not_carried_back_for_someone_else() {
        // Our own message, handed to neighbors. When a neighbor's copy reaches
        // us again we must recognize it, not take it on as third-party traffic.
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

    // ====================================================================
    // Capability bias
    // ====================================================================

    /// A governor with a real jitter window, so bias has something to shift.
    fn biased_governor() -> MeshRelayGovernor {
        MeshRelayGovernor::with_config("relay-node", MeshRelayConfig::default())
    }

    #[test]
    fn a_weak_device_always_waits_longer_than_a_capable_one() {
        // The whole mechanism: the weak device's earliest delay must be later
        // than the capable device's latest, or the two windows overlap and the
        // weak one sometimes transmits first — spending the airtime the bias
        // exists to save.
        let msg = frame();

        let mut capable = biased_governor();
        capable.set_conditions(Some(100), false, false, 30);

        let mut weak = biased_governor();
        weak.set_conditions(Some(30), false, false, 30);

        let capable_delay = capable.jitter_for(&msg.id.as_str(), 3);
        let weak_delay = weak.jitter_for(&msg.id.as_str(), 3);
        assert!(
            weak_delay > capable_delay,
            "weak waited {weak_delay:?}, capable {capable_delay:?}"
        );

        // Independent of which frame it is: the handicap shifts the window, so
        // the worst case for the weak device beats the best case for the
        // capable one on every id.
        //
        // The bound that has to clear the span is the handicap a device can
        // actually pay, not the configured ceiling. A device at the floor
        // scales to `bias_min_scale`, never to zero, so the most it ever pays
        // is `(1 - bias_min_scale) * bias_max_handicap` — mirroring
        // `jitter_for`'s expression, truncation included. Asserting the raw
        // ceiling instead would stay green while a raised min scale silently
        // let the two windows overlap.
        let span = DEFAULT_RELAY_JITTER_MAX_MS - DEFAULT_RELAY_JITTER_MIN_MS;
        let reachable_handicap_ms = ((1.0 - DEFAULT_RELAY_BIAS_MIN_SCALE)
            * DEFAULT_RELAY_BIAS_MAX_HANDICAP_MS as f32) as u64;
        assert!(
            reachable_handicap_ms >= span,
            "the reachable handicap ({reachable_handicap_ms}ms: a \
             {DEFAULT_RELAY_BIAS_MAX_HANDICAP_MS}ms ceiling scaled by 1 - \
             {DEFAULT_RELAY_BIAS_MIN_SCALE}) must cover the jitter span ({span}ms) or the two \
             windows overlap"
        );
    }

    #[test]
    fn bias_adds_a_bounded_delay_rather_than_a_multiplied_one() {
        // Bias must cost redundancy, never delivery. The jitter span already
        // grows with density, so the one shape that must not exist is a
        // handicap that grows with it too: at every density the biased device
        // waits at most one fixed handicap longer than the capable one.
        let msg = frame();
        let cap = Duration::from_millis(DEFAULT_RELAY_BIAS_MAX_HANDICAP_MS);

        for degree in [0usize, 3, 6, 30, 100, MAX_RELAY_RATE_PEERS] {
            let mut capable = biased_governor();
            capable.set_conditions(Some(100), false, false, 30);
            let mut weak = biased_governor();
            weak.set_conditions(Some(0), false, false, 30);

            let capable_delay = capable.jitter_for(&msg.id.as_str(), degree);
            let weak_delay = weak.jitter_for(&msg.id.as_str(), degree);
            assert!(weak_delay >= capable_delay, "degree {degree}");
            assert!(
                weak_delay - capable_delay <= cap,
                "at degree {degree} bias added {:?}, past its {cap:?} ceiling",
                weak_delay - capable_delay
            );
        }
    }

    #[test]
    fn a_biased_forward_still_transmits_inside_the_overdue_cutoff() {
        // Asserted over *every* possible message id by computing the worst
        // case, not by sampling one: the delay is `min + handicap + hash %
        // span`, so a sampled id passes or fails on its hash and would make
        // this a coin toss rather than a guarantee.
        //
        // Bounded to the venue-sized neighborhood this design targets (~100
        // nodes). Past roughly 155 neighbors the delay does outrun the
        // cut-off — but the density scaling alone does that at ~167 with bias
        // switched off entirely, so it is a property of the pre-existing
        // density term rather than of bias, and out of scope here.
        let config = MeshRelayConfig::default();
        let min_ms = config.jitter_min.as_millis() as u64;
        let max_ms = config.jitter_max.as_millis() as u64;
        let degree = 100usize;

        let density_scale = 1 + (degree / config.dense_degree.max(1)) as u64;
        let span = (max_ms - min_ms).saturating_mul(density_scale).max(1);
        // The full handicap, which only a device biased to the floor pays.
        let worst = Duration::from_millis(
            min_ms + config.bias_max_handicap.as_millis() as u64 + (span - 1),
        );

        assert!(
            worst < RELAY_QUEUE_MAX_OVERDUE,
            "the worst biased wait at degree {degree} is {worst:?}, past the \
             {RELAY_QUEUE_MAX_OVERDUE:?} cut-off"
        );
    }

    #[test]
    fn charging_and_eager_devices_are_not_biased_down() {
        let msg = frame();
        let mut unfed = biased_governor();
        let baseline = unfed.jitter_for(&msg.id.as_str(), 3);

        // Charging at a level that would otherwise be heavily penalised.
        let mut charging = biased_governor();
        charging.set_conditions(Some(20), true, false, 30);
        assert_eq!(charging.jitter_for(&msg.id.as_str(), 3), baseline);

        // Configured to relay eagerly, on battery.
        let mut eager = biased_governor();
        eager.set_conditions(Some(20), false, true, 30);
        assert_eq!(eager.jitter_for(&msg.id.as_str(), 3), baseline);

        // And an unfed device is treated as fully capable, matching the
        // forwarding gate's "unknown means willing".
        unfed.set_conditions(None, false, false, 30);
        assert_eq!(unfed.jitter_for(&msg.id.as_str(), 3), baseline);
    }

    #[test]
    fn a_weak_device_forwards_to_fewer_neighbors_but_never_none() {
        let neighbors: Vec<(&str, u8)> = vec![("a", 90), ("b", 80), ("c", 70), ("d", 60)];

        let mut capable = biased_governor();
        capable.set_conditions(Some(100), false, false, 30);
        let capable_targets = capable.select_targets(neighbors.clone(), &[], "msg-1");
        assert_eq!(capable_targets.len(), DEFAULT_RELAY_FANOUT);

        let mut weak = biased_governor();
        weak.set_conditions(Some(0), false, false, 30);
        let weak_targets = weak.select_targets(neighbors, &[], "msg-1");
        assert!(
            !weak_targets.is_empty(),
            "a fan-out of zero is a silent drop, not a cheaper forward"
        );
        assert!(
            weak_targets.len() < capable_targets.len(),
            "weak fanned out to {} neighbors, capable to {}",
            weak_targets.len(),
            capable_targets.len()
        );
    }

    #[test]
    fn a_weak_device_offers_its_own_frames_to_every_neighbor_it_would_have() {
        // Bias narrows the fan-out for other people's traffic, where neighbors
        // hold copies and the sender is still retrying, so the cost is
        // redundancy. A frame this device originated has neither: the fan-out
        // *is* the delivery attempt, and because targets are chosen stably per
        // message id, a narrowed set would be the same narrowed set on every
        // retransmission — one dead-end neighbor forever instead of three
        // chances.
        let neighbors: Vec<(&str, u8)> = vec![("a", 90), ("b", 80), ("c", 70), ("d", 60)];

        let mut weak = biased_governor();
        weak.set_conditions(Some(0), false, false, 30);

        assert!(
            weak.select_targets(neighbors.clone(), &[], "msg-1").len() < DEFAULT_RELAY_FANOUT,
            "the forwarding path must still be biased",
        );
        assert_eq!(
            weak.select_origination_targets(neighbors.clone(), &[], "msg-1")
                .len(),
            DEFAULT_RELAY_FANOUT,
            "this device's own frame must reach as many neighbors as a capable device's",
        );

        // And identical to what a fully capable device would have picked, not
        // merely as many: same neighbors, same order.
        let mut capable = biased_governor();
        capable.set_conditions(Some(100), false, false, 30);
        assert_eq!(
            weak.select_origination_targets(neighbors.clone(), &[], "msg-1"),
            capable.select_origination_targets(neighbors, &[], "msg-1"),
        );
    }

    #[test]
    fn bias_does_not_throttle_this_devices_own_traffic() {
        // Bias scales the *forwarding* share of the airtime ceiling, not the
        // ceiling. Scaling the shared bucket would slow a low-battery device's
        // own messages and acknowledgements in step with its battery, which is
        // delivery rather than redundancy — and an absent acknowledgement is
        // what makes a delivered message look lost to its sender.
        let mut capable = MeshRelayGovernor::with_config("relay-node", immediate_config());
        capable.set_conditions(Some(100), false, false, 30);
        let capable_own = (0..1000).filter(|_| capable.take_own_send_token()).count();

        let mut weak = MeshRelayGovernor::with_config("relay-node", immediate_config());
        weak.set_conditions(Some(0), false, false, 30);
        let weak_own = (0..1000).filter(|_| weak.take_own_send_token()).count();

        assert_eq!(
            weak_own, capable_own,
            "a weak device got {weak_own} of its own transmissions against a capable \
             device's {capable_own}",
        );
        assert!(capable_own > 0, "the fixture must actually grant something");
    }

    #[test]
    fn a_weak_device_transmits_less_before_hitting_its_ceiling() {
        let mut capable = MeshRelayGovernor::with_config("relay-node", immediate_config());
        capable.set_conditions(Some(100), false, false, 30);
        let capable_grants = (0..1000).filter(|_| capable.take_send_token()).count();

        let mut weak = MeshRelayGovernor::with_config("relay-node", immediate_config());
        weak.set_conditions(Some(0), false, false, 30);
        let weak_grants = (0..1000).filter(|_| weak.take_send_token()).count();

        assert!(
            weak_grants < capable_grants,
            "weak got {weak_grants} transmissions, capable {capable_grants}"
        );
        assert!(
            weak_grants > 0,
            "bias scales the budget down, it does not switch forwarding off"
        );
    }

    #[test]
    fn a_worsening_battery_does_not_mint_a_fresh_burst() {
        // `set_conditions` runs every tick, so re-sizing the bucket must keep
        // what has been spent spent. If it refilled, a device whose battery
        // oscillates around any point in the ramp would transmit without limit.
        let mut gov = MeshRelayGovernor::with_config("relay-node", immediate_config());
        gov.set_conditions(Some(100), false, false, 30);

        let first = (0..1000).filter(|_| gov.take_send_token()).count();
        assert!(first > 0);
        assert!(!gov.take_send_token(), "the budget must be spent");

        for level in [90, 100, 90, 100] {
            gov.set_conditions(Some(level), false, false, 30);
            assert!(
                !gov.take_send_token(),
                "re-sizing the budget handed back tokens that were already spent"
            );
        }
    }

    // ====================================================================
    // Activity-derived relay standing
    // ====================================================================

    /// Short windows so activity transitions are reachable without waiting a
    /// real minute; `observe_activity` takes `now`, so the clock is explicit.
    fn activity_config() -> MeshRelayConfig {
        MeshRelayConfig {
            activity_window: Duration::from_secs(10),
            activity_min_forwards: 2,
            activity_idle_windows: 2,
            ..immediate_config()
        }
    }

    #[test]
    fn carrying_traffic_is_what_makes_a_device_a_relay() {
        let mut gov = MeshRelayGovernor::with_config("relay-node", activity_config());
        let start = Instant::now();
        assert!(!gov.is_active_relay());

        // A window that carried nothing changes nothing.
        assert_eq!(gov.observe_activity(start + Duration::from_secs(11)), None);
        assert!(!gov.is_active_relay());

        // Carrying enough in one window promotes, exactly once.
        gov.record_forwarded();
        gov.record_forwarded();
        assert_eq!(
            gov.observe_activity(start + Duration::from_secs(22)),
            Some(RelayActivity::Began { forwarded: 2 })
        );
        assert!(gov.is_active_relay());

        gov.record_forwarded();
        gov.record_forwarded();
        assert_eq!(
            gov.observe_activity(start + Duration::from_secs(33)),
            None,
            "a relay that keeps carrying traffic must not re-announce"
        );
        assert!(gov.is_active_relay());
    }

    #[test]
    fn a_relay_stops_being_one_only_after_sustained_quiet() {
        // Asymmetric on purpose: one busy window promotes, several quiet ones
        // demote. A mesh with nothing to say for a window has not stopped
        // having a relay in it.
        let mut gov = MeshRelayGovernor::with_config("relay-node", activity_config());
        let start = Instant::now();

        gov.record_forwarded();
        gov.record_forwarded();
        assert!(matches!(
            gov.observe_activity(start + Duration::from_secs(11)),
            Some(RelayActivity::Began { .. })
        ));

        // One quiet window is not enough.
        assert_eq!(gov.observe_activity(start + Duration::from_secs(22)), None);
        assert!(gov.is_active_relay());

        assert_eq!(
            gov.observe_activity(start + Duration::from_secs(33)),
            Some(RelayActivity::Ceased)
        );
        assert!(!gov.is_active_relay());

        // And it stays ceased rather than re-announcing every window.
        assert_eq!(gov.observe_activity(start + Duration::from_secs(44)), None);
    }

    #[test]
    fn traffic_within_a_window_resets_the_run_of_quiet_ones() {
        let mut gov = MeshRelayGovernor::with_config("relay-node", activity_config());
        let start = Instant::now();

        gov.record_forwarded();
        gov.record_forwarded();
        assert!(matches!(
            gov.observe_activity(start + Duration::from_secs(11)),
            Some(RelayActivity::Began { .. })
        ));

        // Quiet, then busy again, then quiet: the run restarts, so the second
        // quiet window must not be the one that demotes.
        assert_eq!(gov.observe_activity(start + Duration::from_secs(22)), None);
        gov.record_forwarded();
        gov.record_forwarded();
        assert_eq!(gov.observe_activity(start + Duration::from_secs(33)), None);
        assert_eq!(gov.observe_activity(start + Duration::from_secs(44)), None);
        assert!(gov.is_active_relay(), "the quiet run restarted");

        assert_eq!(
            gov.observe_activity(start + Duration::from_secs(55)),
            Some(RelayActivity::Ceased)
        );
    }

    #[test]
    fn observing_before_a_window_has_elapsed_does_nothing() {
        let mut gov = MeshRelayGovernor::with_config("relay-node", activity_config());
        let start = Instant::now();

        gov.record_forwarded();
        gov.record_forwarded();
        assert_eq!(gov.observe_activity(start + Duration::from_secs(5)), None);
        assert!(
            !gov.is_active_relay(),
            "the window must complete before it counts"
        );

        // And the frames carried are still counted toward it.
        assert!(matches!(
            gov.observe_activity(start + Duration::from_secs(11)),
            Some(RelayActivity::Began { forwarded: 2 })
        ));
    }

    #[test]
    fn a_closing_gate_drops_the_relay_standing_at_once() {
        // Configuration or battery closing the forwarding gate stops traffic
        // immediately, so waiting out the idle windows would report a relay
        // that had already stopped being one.
        let mut gov = MeshRelayGovernor::with_config("relay-node", activity_config());
        let start = Instant::now();

        gov.record_forwarded();
        gov.record_forwarded();
        assert!(matches!(
            gov.observe_activity(start + Duration::from_secs(11)),
            Some(RelayActivity::Began { .. })
        ));

        assert!(gov.force_inactive(), "reports that the standing was held");
        assert!(!gov.is_active_relay());
        assert!(
            !gov.force_inactive(),
            "a device that was not a relay must not report a demotion"
        );

        // The window's tally went with it, so re-opening the gate starts from
        // scratch rather than promoting on stale traffic.
        assert_eq!(gov.observe_activity(start + Duration::from_secs(22)), None);
        assert!(!gov.is_active_relay());
    }
}
