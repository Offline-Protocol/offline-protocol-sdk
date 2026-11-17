//! The fast link: a Wi-Fi network one device hosts for one peer, whose
//! credentials travel inside their confirmed 1:1 session (ADR 0038).
//!
//! This module is the state machine of `docs/state-machines/fast-link.md` and
//! the `__FAST_LINK__` frame of `docs/spec/control-messages.md`, and nothing
//! else. It holds no engine, no clock and no randomness: every input arrives
//! as an argument, and every output waits in one of three queues the engine
//! drains (frames for the peer, directives for the platform, changes for the
//! application). That is what lets the tests below drive both ends of a pair
//! through every transition without a radio, a session or a timer.
//!
//! The engine glue is `protocol/fast_link.rs`: it seals the frames, checks
//! that a request may be made at all, feeds this machine the stream's proof
//! and loss, and turns changes into events.

use std::collections::VecDeque;
use std::fmt;
use std::net::IpAddr;

use offline_protocol_core::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

/// The frame schema version, the `v` of every body.
pub const FAST_LINK_FRAME_VERSION: u8 = 1;

/// How long an offer stays good, from the moment it is made (host) or
/// arrives (joiner). Covers the user reading the iOS prompt and the ~14 s a
/// join took on the lab pair, with room for a second dial.
pub const OFFER_TTL: Duration = Duration::from_secs(120);

/// The longest lifetime a receiver accepts on an offer.
pub const MAX_OFFER_TTL_MS: u64 = 300_000;

/// How far an offer's stated time may disagree with the receiver's clock,
/// beyond its lifetime. Phones disagree by seconds; this bounds only a frame
/// that is plainly not from now.
pub const OFFER_CLOCK_SLACK_MS: i64 = 300_000;

/// How long a host's platform has to bring its network up. Group creation
/// took 0.15 s on the lab phone.
pub const HOSTING_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a requester waits for an offer.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an up link may carry nothing large, with no transfer to the peer
/// in flight, before it ends. It costs the joiner its own Wi-Fi.
pub const IDLE_LIMIT: Duration = Duration::from_secs(60);

/// The smallest body that counts as activity for [`IDLE_LIMIT`]: half a media
/// chunk on the slot. Presence, receipts and chat are smaller and go over
/// Bluetooth LE perfectly well, and counting them kept a link up for as long
/// as an application sent presence every 15 s (two phones, ADR 0038).
pub const ACTIVITY_MIN_BYTES: usize = 16 * 1024;

/// Hex digits in a link id, 128 bits.
pub const LINK_ID_HEX_LEN: usize = 32;

/// Hex digits in a passphrase the engine makes, 128 bits.
pub const PASSPHRASE_HEX_LEN: usize = 32;

/// How long an ended link's id is remembered, so a frame for it that arrives
/// late is refused rather than acted on: the longest an offer lives, plus
/// the clock slack.
pub const ENDED_MEMORY: Duration = Duration::from_millis(MAX_OFFER_TTL_MS + 300_000);

/// How long, after this device's user declined a peer's network, that peer's
/// unrequested offers are refused. Every join on iOS puts a system prompt in
/// front of the user; without a pause a session peer could put up another
/// the moment one was declined.
pub const DECLINE_BACKOFF: Duration = Duration::from_secs(600);

/// How many ended link ids are remembered. One link at a time, so a few.
pub const MAX_ENDED_REMEMBERED: usize = 16;

/// The largest body after the prefix a receiver parses. An offer is about
/// 250 bytes.
pub const MAX_FRAME_BODY_BYTES: usize = 2048;

/// What this device's platform can do. Both false until it says otherwise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FastLinkRoles {
    /// It can bring up a network for a peer and admit only that peer.
    pub can_host: bool,
    /// It can join a network by name and passphrase and dial an address on it.
    pub can_join: bool,
}

/// Which side of a link this device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastLinkRole {
    /// It brought the network up.
    Host,
    /// It joins the network the peer brought up.
    Join,
}

impl FastLinkRole {
    /// The token events carry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Join => "join",
        }
    }
}

/// Where a link is. The six states of the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastLinkState {
    /// A request is out; waiting for an offer.
    Requested,
    /// The platform is bringing a network up.
    Hosting,
    /// The offer is out; waiting for the peer's stream.
    Offered,
    /// The platform is joining the offered network.
    Joining,
    /// A stream proved the peer.
    Up,
    /// Over.
    Closed,
}

impl FastLinkState {
    /// The token events carry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Hosting => "hosting",
            Self::Offered => "offered",
            Self::Joining => "joining",
            Self::Up => "up",
            Self::Closed => "closed",
        }
    }
}

/// Why a link ended, the `reason` of a `close` frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The joiner's user declined the network.
    Declined,
    /// Something failed that has no more specific reason.
    Failed,
    /// The host's platform did not bring its network up.
    HostFailed,
    /// The peer cannot host.
    CannotHost,
    /// The peer cannot join.
    CannotJoin,
    /// The peer holds a link with someone else.
    Busy,
    /// An offer failed validation.
    Invalid,
    /// The offer's lifetime ran out before a stream proved the peer.
    Expired,
    /// No offer answered a request in time.
    NoOffer,
    /// The stream over the link ended.
    Lost,
    /// The link carried nothing for [`IDLE_LIMIT`].
    Idle,
    /// The joiner's application left the foreground and its platform drops
    /// the network.
    Background,
    /// The application closed it.
    Closed,
}

impl CloseReason {
    /// The wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declined => "declined",
            Self::Failed => "failed",
            Self::HostFailed => "host_failed",
            Self::CannotHost => "cannot_host",
            Self::CannotJoin => "cannot_join",
            Self::Busy => "busy",
            Self::Invalid => "invalid",
            Self::Expired => "expired",
            Self::NoOffer => "no_offer",
            Self::Lost => "lost",
            Self::Idle => "idle",
            Self::Background => "background",
            Self::Closed => "closed",
        }
    }

    /// Reads a wire or platform token. An unknown one is `failed`, so a
    /// later release's reason still ends the link.
    pub fn parse(token: &str) -> Self {
        match token {
            "declined" => Self::Declined,
            "host_failed" => Self::HostFailed,
            "cannot_host" => Self::CannotHost,
            "cannot_join" => Self::CannotJoin,
            "busy" => Self::Busy,
            "invalid" => Self::Invalid,
            "expired" => Self::Expired,
            "no_offer" => Self::NoOffer,
            "lost" => Self::Lost,
            "idle" => Self::Idle,
            "background" => Self::Background,
            "closed" => Self::Closed,
            _ => Self::Failed,
        }
    }
}

/// One `__FAST_LINK__` body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Frame {
    pub(crate) v: u8,
    pub(crate) link: String,
    #[serde(flatten)]
    pub(crate) body: FrameBody,
}

/// The three kinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum FrameBody {
    Request,
    Offer(Offer),
    Close { reason: String },
}

/// What an offer names.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Offer {
    pub(crate) ssid: String,
    pub(crate) passphrase: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) issued_at_ms: i64,
    pub(crate) ttl_ms: u64,
}

impl fmt::Debug for Offer {
    // Never the passphrase, in any log (invariant 5).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Offer")
            .field("ssid", &self.ssid)
            .field("passphrase", &"[REDACTED]")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("issued_at_ms", &self.issued_at_ms)
            .field("ttl_ms", &self.ttl_ms)
            .finish()
    }
}

impl Frame {
    fn new(link: &str, body: FrameBody) -> Self {
        Self {
            v: FAST_LINK_FRAME_VERSION,
            link: link.to_string(),
            body,
        }
    }

    /// The body after the prefix.
    pub(crate) fn encode(&self) -> String {
        // A struct of strings and integers always serializes.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parses a body after the prefix. `None` for anything this version does
    /// not speak: another `v`, a malformed link id, an oversized body.
    pub(crate) fn parse(body: &str) -> Option<Self> {
        if body.len() > MAX_FRAME_BODY_BYTES {
            return None;
        }
        let frame: Self = serde_json::from_str(body).ok()?;
        (frame.v == FAST_LINK_FRAME_VERSION && is_link_id(&frame.link)).then_some(frame)
    }
}

/// A link id: 32 lowercase hex digits.
pub(crate) fn is_link_id(id: &str) -> bool {
    id.len() == LINK_ID_HEX_LEN && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `host` is an address a joiner may dial: an IP literal in a private
/// or link-local range, so an offer can never name a host to resolve or a
/// route off the network just joined.
pub(crate) fn is_local_host(host: &str) -> bool {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_private() || v4.is_link_local(),
        Ok(IpAddr::V6(v6)) => {
            let first = v6.segments()[0];
            // fc00::/7 unique local, fe80::/10 link local.
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

impl Offer {
    /// The bounds of the control-messages chapter, against the receiver's
    /// clock.
    pub(crate) fn is_valid(&self, now_ms: i64) -> bool {
        let ssid_ok = (1..=32).contains(&self.ssid.len());
        let passphrase_ok = (8..=63).contains(&self.passphrase.len())
            && self.passphrase.bytes().all(|b| (0x20..=0x7e).contains(&b));
        let ttl_ok = self.ttl_ms > 0 && self.ttl_ms <= MAX_OFFER_TTL_MS;
        // ttl_ms is at most MAX_OFFER_TTL_MS here, so the cast is exact.
        let oldest = now_ms - self.ttl_ms as i64 - OFFER_CLOCK_SLACK_MS;
        let newest = now_ms + OFFER_CLOCK_SLACK_MS;
        let time_ok = ttl_ok && (oldest..=newest).contains(&self.issued_at_ms);
        ssid_ok && passphrase_ok && is_local_host(&self.host) && self.port != 0 && time_ok
    }
}

/// A link id and credentials, drawn by the engine from its random source.
#[derive(Clone, PartialEq, Eq)]
pub struct FreshLink {
    /// 32 lowercase hex digits.
    pub link_id: String,
    /// `DIRECT-xy-` and twelve hex digits.
    pub ssid: String,
    /// 32 hex digits.
    pub passphrase: String,
}

impl fmt::Debug for FreshLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FreshLink")
            .field("link_id", &self.link_id)
            .field("ssid", &self.ssid)
            .field("passphrase", &"[REDACTED]")
            .finish()
    }
}

impl FreshLink {
    /// Draws one from `fill`, which must be a cryptographic random source.
    ///
    /// The name is `DIRECT-` (Android requires it, and two characters after
    /// it), two random characters, `-` and twelve random hex digits: random,
    /// so nothing on the air derives from the application, the user or the
    /// device (ADR 0037). The passphrase is 128 bits.
    pub fn generate(mut fill: impl FnMut(&mut [u8])) -> Self {
        const ALNUM: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut id = [0u8; LINK_ID_HEX_LEN / 2];
        let mut pass = [0u8; PASSPHRASE_HEX_LEN / 2];
        let mut tag = [0u8; 2];
        let mut name = [0u8; 6];
        fill(&mut id);
        fill(&mut pass);
        fill(&mut tag);
        fill(&mut name);
        let tag: String = tag
            .iter()
            .map(|b| ALNUM[usize::from(*b) % ALNUM.len()] as char)
            .collect();
        Self {
            link_id: hex::encode(id),
            ssid: format!("DIRECT-{tag}-{}", hex::encode(name)),
            passphrase: hex::encode(pass),
        }
    }
}

/// What the platform is told to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastLinkDirectiveKind {
    /// Bring up a network with the given name and passphrase, admitting only
    /// the given peer, and report its address and port.
    Host,
    /// Join the given network and dial `host:port`, claiming the given peer.
    Join,
    /// End whatever this device holds for the link.
    Close,
}

impl FastLinkDirectiveKind {
    /// The token the platform reads.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Join => "join",
            Self::Close => "close",
        }
    }
}

/// One instruction for the platform.
#[derive(Clone, PartialEq, Eq)]
pub struct FastLinkDirective {
    /// What to do.
    pub kind: FastLinkDirectiveKind,
    /// The link it is for.
    pub link_id: String,
    /// The peer: the only address a host admits, and the address a joiner's
    /// stream must prove.
    pub peer_id: String,
    /// The network's name (`host`, `join`).
    pub ssid: Option<String>,
    /// The network's passphrase (`host`, `join`).
    pub passphrase: Option<String>,
    /// The address to dial (`join`).
    pub host: Option<String>,
    /// The port to dial (`join`).
    pub port: Option<u16>,
    /// How long the platform has: to bring the network up (`host`), or to
    /// prove the stream (`join`). Zero on `close`.
    pub expires_in_ms: u64,
    /// Why (`close`).
    pub reason: Option<CloseReason>,
}

impl fmt::Debug for FastLinkDirective {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FastLinkDirective")
            .field("kind", &self.kind)
            .field("link_id", &self.link_id)
            .field("peer_id", &"[REDACTED]")
            .field("ssid", &self.ssid)
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .field("host", &self.host)
            .field("port", &self.port)
            .field("expires_in_ms", &self.expires_in_ms)
            .field("reason", &self.reason)
            .finish()
    }
}

/// A transition the application hears about. Never a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastLinkChange {
    /// The peer.
    pub peer_id: String,
    /// The link.
    pub link_id: String,
    /// This device's side.
    pub role: FastLinkRole,
    /// Where the link is now.
    pub state: FastLinkState,
    /// Why it closed.
    pub reason: Option<CloseReason>,
}

/// Why a request was refused before anything was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastLinkRefusal {
    /// This device can neither host nor join.
    Unsupported,
    /// A link with another peer is in progress or up.
    Busy,
}

/// A directive and the instant its `expires_in_ms` counts down to.
#[derive(Debug, Clone)]
struct PendingDirective {
    directive: FastLinkDirective,
    deadline: Option<Instant>,
}

#[derive(Debug, Clone)]
struct Link {
    id: String,
    peer: String,
    role: FastLinkRole,
    state: FastLinkState,
    /// When the current state ends by itself.
    deadline: Option<Instant>,
    last_activity: Instant,
    /// The host's credentials, until the offer is made.
    credentials: Option<(String, String)>,
    /// The host's offer, kept to answer a repeated request.
    offer: Option<Offer>,
    /// The platform drained a directive for this link, so its end needs a
    /// `close` directive. Set when the platform takes it, not when it is
    /// queued: a link that ends before then has its directive withdrawn and
    /// owes the platform nothing.
    platform_engaged: bool,
    /// The peer knows of this link, so its end needs a `close` frame.
    peer_engaged: bool,
}

/// The machine. One link at a time (invariant 3).
#[derive(Debug, Default)]
pub(crate) struct FastLinks {
    roles: FastLinkRoles,
    link: Option<Link>,
    directives: VecDeque<PendingDirective>,
    frames: VecDeque<(String, Frame)>,
    changes: VecDeque<FastLinkChange>,
    /// Links this device ended, and when. A request or offer for one that
    /// arrives afterwards is late, and acting on it would bring up or join a
    /// network nobody is waiting for: on two phones a request delayed 63 s
    /// reached the host after the requester had given up, and its offer then
    /// prompted the requester's user to join a network already removed.
    ended: VecDeque<(String, Instant)>,
    /// Peers whose network this device's user declined, and when; their
    /// offers are refused for [`DECLINE_BACKOFF`] unless the user asks.
    declined: VecDeque<(String, Instant)>,
}

impl FastLinks {
    pub(crate) fn set_roles(&mut self, roles: FastLinkRoles) {
        self.roles = roles;
    }

    /// The peer of the link in progress, if any.
    pub(crate) fn peer(&self) -> Option<&str> {
        self.link.as_ref().map(|link| link.peer.as_str())
    }

    /// The application asks for a link to `peer`. Returns the link id.
    pub(crate) fn request(
        &mut self,
        peer: &str,
        fresh: FreshLink,
        now: Instant,
    ) -> Result<String, FastLinkRefusal> {
        if let Some(link) = &self.link {
            return if link.peer == peer {
                Ok(link.id.clone())
            } else {
                Err(FastLinkRefusal::Busy)
            };
        }
        // The user asking is the user's answer to an earlier decline.
        self.declined.retain(|(declined, _)| declined != peer);
        if self.roles.can_host {
            let id = fresh.link_id.clone();
            self.start_host(peer, fresh, now, false);
            return Ok(id);
        }
        if !self.roles.can_join {
            return Err(FastLinkRefusal::Unsupported);
        }
        let id = fresh.link_id;
        self.frames
            .push_back((peer.to_string(), Frame::new(&id, FrameBody::Request)));
        self.link = Some(Link {
            id: id.clone(),
            peer: peer.to_string(),
            role: FastLinkRole::Join,
            state: FastLinkState::Requested,
            deadline: Some(now + REQUEST_TIMEOUT),
            last_activity: now,
            credentials: None,
            offer: None,
            platform_engaged: false,
            peer_engaged: true,
        });
        self.changed(None);
        Ok(id)
    }

    fn start_host(&mut self, peer: &str, fresh: FreshLink, now: Instant, peer_engaged: bool) {
        self.directives.push_back(PendingDirective {
            directive: FastLinkDirective {
                kind: FastLinkDirectiveKind::Host,
                link_id: fresh.link_id.clone(),
                peer_id: peer.to_string(),
                ssid: Some(fresh.ssid.clone()),
                passphrase: Some(fresh.passphrase.clone()),
                host: None,
                port: None,
                expires_in_ms: 0,
                reason: None,
            },
            deadline: Some(now + HOSTING_TIMEOUT),
        });
        self.link = Some(Link {
            id: fresh.link_id,
            peer: peer.to_string(),
            role: FastLinkRole::Host,
            state: FastLinkState::Hosting,
            deadline: Some(now + HOSTING_TIMEOUT),
            last_activity: now,
            credentials: Some((fresh.ssid, fresh.passphrase)),
            offer: None,
            platform_engaged: false,
            peer_engaged,
        });
        self.changed(None);
    }

    /// The host's platform has its network up at `host:port`.
    pub(crate) fn hosting(
        &mut self,
        link_id: &str,
        host: &str,
        port: u16,
        now: Instant,
        now_ms: i64,
    ) -> bool {
        let ours = self.link.as_ref().is_some_and(|link| {
            link.id == link_id
                && link.role == FastLinkRole::Host
                && link.state == FastLinkState::Hosting
                && link.credentials.is_some()
        });
        if !ours {
            return false;
        }
        if !is_local_host(host) || port == 0 {
            self.end(CloseReason::HostFailed, true, now);
            return false;
        }
        let Some(link) = self.link.as_mut() else {
            return false;
        };
        let Some((ssid, passphrase)) = link.credentials.take() else {
            return false;
        };
        let offer = Offer {
            ssid,
            passphrase,
            host: host.to_string(),
            port,
            issued_at_ms: now_ms,
            ttl_ms: OFFER_TTL.as_millis() as u64,
        };
        link.state = FastLinkState::Offered;
        link.deadline = Some(now + OFFER_TTL);
        link.peer_engaged = true;
        link.offer = Some(offer.clone());
        let frame = Frame::new(&link.id, FrameBody::Offer(offer));
        let peer = link.peer.clone();
        self.frames.push_back((peer, frame));
        self.changed(None);
        true
    }

    /// A frame from `peer`, who is its MLS-authenticated sender.
    pub(crate) fn on_frame(
        &mut self,
        peer: &str,
        local: &str,
        frame: Frame,
        fresh: FreshLink,
        now: Instant,
        now_ms: i64,
    ) {
        if self.ended_recently(&frame.link, now) {
            // A late request needs no answer: its sender has given up on it.
            // A late offer gets one, so the host takes its network down.
            if matches!(frame.body, FrameBody::Offer(_)) {
                self.reply_close(peer, &frame.link, CloseReason::Expired);
            }
            return;
        }
        match frame.body {
            FrameBody::Request => self.on_request(peer, &frame.link, fresh, now),
            FrameBody::Offer(offer) => self.on_offer(peer, local, &frame.link, offer, now, now_ms),
            FrameBody::Close { reason } => {
                if self
                    .link
                    .as_ref()
                    .is_some_and(|link| link.id == frame.link && link.peer == peer)
                {
                    self.end(CloseReason::parse(&reason), false, now);
                } else {
                    // A close can overtake the offer it ends: remembered, so
                    // the offer is refused as late when it lands.
                    self.remember_ended(&frame.link, now);
                }
            }
        }
    }

    fn reply_close(&mut self, peer: &str, link: &str, reason: CloseReason) {
        self.frames.push_back((
            peer.to_string(),
            Frame::new(
                link,
                FrameBody::Close {
                    reason: reason.as_str().to_string(),
                },
            ),
        ));
    }

    fn on_request(&mut self, peer: &str, link_id: &str, fresh: FreshLink, now: Instant) {
        if let Some(link) = self.link.clone() {
            if link.peer != peer {
                self.reply_close(peer, link_id, CloseReason::Busy);
                return;
            }
            match (link.role, link.state, link.offer) {
                // The request means our offer was lost: send it again.
                (FastLinkRole::Host, FastLinkState::Offered, Some(offer)) => {
                    let frame = Frame::new(&link.id, FrameBody::Offer(offer));
                    self.frames.push_back((peer.to_string(), frame));
                }
                // Our offer follows once the network is up.
                (FastLinkRole::Host, _, _) => {}
                // Each asked the other to host: neither can.
                (FastLinkRole::Join, _, _) => {
                    self.reply_close(peer, link_id, CloseReason::CannotHost);
                }
            }
            return;
        }
        if !self.roles.can_host {
            self.reply_close(peer, link_id, CloseReason::CannotHost);
            return;
        }
        // Under the requester's id, so its close and our offer name one link.
        let fresh = FreshLink {
            link_id: link_id.to_string(),
            ..fresh
        };
        self.start_host(peer, fresh, now, true);
    }

    fn on_offer(
        &mut self,
        peer: &str,
        local: &str,
        link_id: &str,
        offer: Offer,
        now: Instant,
        now_ms: i64,
    ) {
        if !offer.is_valid(now_ms) {
            self.reply_close(peer, link_id, CloseReason::Invalid);
            return;
        }
        self.declined
            .retain(|(_, at)| now.saturating_duration_since(*at) < DECLINE_BACKOFF);
        if self.declined.iter().any(|(declined, _)| declined == peer) {
            self.reply_close(peer, link_id, CloseReason::Declined);
            return;
        }
        if let Some(link) = self.link.clone() {
            if link.peer != peer {
                self.reply_close(peer, link_id, CloseReason::Busy);
                return;
            }
            match link.role {
                FastLinkRole::Host => {
                    // Both are hosting for each other. The lower address
                    // keeps hosting; the higher closes its own and joins.
                    if local.as_bytes() < peer.as_bytes() {
                        return;
                    }
                    self.end(CloseReason::Busy, true, now);
                }
                FastLinkRole::Join => {
                    if link.id == link_id && link.state != FastLinkState::Requested {
                        // A repeat of the offer this link already acts on.
                        return;
                    }
                    if link.state != FastLinkState::Requested {
                        // A new offer replaces the one before it: the host
                        // brought up a new network. Nothing to tell it.
                        self.end(CloseReason::Failed, false, now);
                    } else {
                        // The offer answers our request; it supersedes the
                        // request's link quietly, whatever id the host chose.
                        self.link = None;
                    }
                }
            }
        }
        if !self.roles.can_join {
            self.reply_close(peer, link_id, CloseReason::CannotJoin);
            return;
        }
        let ttl = Duration::from_millis(offer.ttl_ms.min(MAX_OFFER_TTL_MS));
        self.directives.push_back(PendingDirective {
            directive: FastLinkDirective {
                kind: FastLinkDirectiveKind::Join,
                link_id: link_id.to_string(),
                peer_id: peer.to_string(),
                ssid: Some(offer.ssid),
                passphrase: Some(offer.passphrase),
                host: Some(offer.host),
                port: Some(offer.port),
                expires_in_ms: 0,
                reason: None,
            },
            deadline: Some(now + ttl),
        });
        self.link = Some(Link {
            id: link_id.to_string(),
            peer: peer.to_string(),
            role: FastLinkRole::Join,
            state: FastLinkState::Joining,
            deadline: Some(now + ttl),
            last_activity: now,
            credentials: None,
            offer: None,
            platform_engaged: false,
            peer_engaged: true,
        });
        self.changed(None);
    }

    /// The platform could not do what it was told, or can no longer hold the
    /// link. It already let go, so no `close` directive follows.
    pub(crate) fn platform_ended(
        &mut self,
        link_id: &str,
        reason: CloseReason,
        now: Instant,
    ) -> bool {
        match self.link.as_mut() {
            Some(link) if link.id == link_id => {
                link.platform_engaged = false;
                self.end(reason, true, now);
                true
            }
            _ => false,
        }
    }

    /// The application ends the link with `peer`.
    pub(crate) fn close(&mut self, peer: &str, now: Instant) -> bool {
        if self.peer() != Some(peer) {
            return false;
        }
        self.end(CloseReason::Closed, true, now);
        true
    }

    /// Ends the link with `peer` without telling the peer: this device's user
    /// blocked it, and nothing more is sent to a blocked peer.
    pub(crate) fn forget(&mut self, peer: &str, now: Instant) -> bool {
        if self.peer() != Some(peer) {
            return false;
        }
        self.end(CloseReason::Closed, false, now);
        true
    }

    /// A stream proved `peer`.
    pub(crate) fn stream_proved(&mut self, peer: &str, now: Instant) {
        let Some(link) = self.link.as_mut() else {
            return;
        };
        if link.peer == peer
            && matches!(link.state, FastLinkState::Offered | FastLinkState::Joining)
        {
            link.state = FastLinkState::Up;
            link.deadline = None;
            link.last_activity = now;
            self.changed(None);
        }
    }

    /// The stream that proved `peer` ended.
    pub(crate) fn stream_lost(&mut self, peer: &str, now: Instant) {
        if self
            .link
            .as_ref()
            .is_some_and(|link| link.peer == peer && link.state == FastLinkState::Up)
        {
            self.end(CloseReason::Lost, true, now);
        }
    }

    /// The slot carried a body of `bytes` for `peer`. Only a large one moves
    /// the idle clock ([`ACTIVITY_MIN_BYTES`]).
    pub(crate) fn activity(&mut self, peer: &str, bytes: usize, now: Instant) {
        if bytes < ACTIVITY_MIN_BYTES {
            return;
        }
        if let Some(link) = self.link.as_mut() {
            if link.peer == peer {
                link.last_activity = now;
            }
        }
    }

    /// Runs the timers. `busy` says whether a transfer to the link's peer is
    /// in flight, which holds the idle limit off.
    pub(crate) fn tick(&mut self, now: Instant, busy: impl FnOnce(&str) -> bool) {
        let Some(link) = self.link.as_mut() else {
            return;
        };
        let due = link.deadline.is_some_and(|deadline| now >= deadline);
        let reason = match link.state {
            FastLinkState::Hosting if due => Some(CloseReason::HostFailed),
            FastLinkState::Offered | FastLinkState::Joining if due => Some(CloseReason::Expired),
            FastLinkState::Requested if due => Some(CloseReason::NoOffer),
            FastLinkState::Up => {
                if busy(&link.peer) {
                    link.last_activity = now;
                    None
                } else if now.saturating_duration_since(link.last_activity) >= IDLE_LIMIT {
                    Some(CloseReason::Idle)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(reason) = reason {
            self.end(reason, true, now);
        }
    }

    fn remember_ended(&mut self, link_id: &str, now: Instant) {
        if self.ended.len() >= MAX_ENDED_REMEMBERED {
            self.ended.pop_front();
        }
        self.ended.push_back((link_id.to_string(), now));
    }

    fn ended_recently(&mut self, link_id: &str, now: Instant) -> bool {
        while self
            .ended
            .front()
            .is_some_and(|(_, at)| now.saturating_duration_since(*at) >= ENDED_MEMORY)
        {
            self.ended.pop_front();
        }
        self.ended.iter().any(|(id, _)| id == link_id)
    }

    /// Ends the link: a `close` directive when the platform holds something,
    /// a `close` frame when the peer knows of it and `tell_peer`, and one
    /// change (invariant 4).
    fn end(&mut self, reason: CloseReason, tell_peer: bool, now: Instant) {
        let Some(link) = self.link.take() else {
            return;
        };
        // A directive the platform never took is withdrawn: handed out after
        // the link ended it would bring up a network nobody waits on, or put
        // a join prompt in front of the user for one already gone.
        self.directives
            .retain(|pending| pending.directive.link_id != link.id);
        self.remember_ended(&link.id, now);
        if reason == CloseReason::Declined && link.role == FastLinkRole::Join {
            if self.declined.len() >= MAX_ENDED_REMEMBERED {
                self.declined.pop_front();
            }
            self.declined.push_back((link.peer.clone(), now));
        }
        if link.platform_engaged {
            self.directives.push_back(PendingDirective {
                directive: FastLinkDirective {
                    kind: FastLinkDirectiveKind::Close,
                    link_id: link.id.clone(),
                    peer_id: link.peer.clone(),
                    ssid: None,
                    passphrase: None,
                    host: None,
                    port: None,
                    expires_in_ms: 0,
                    reason: Some(reason),
                },
                deadline: None,
            });
        }
        if tell_peer && link.peer_engaged {
            self.reply_close(&link.peer, &link.id, reason);
        }
        self.changes.push_back(FastLinkChange {
            peer_id: link.peer,
            link_id: link.id,
            role: link.role,
            state: FastLinkState::Closed,
            reason: Some(reason),
        });
    }

    fn changed(&mut self, reason: Option<CloseReason>) {
        if let Some(link) = &self.link {
            self.changes.push_back(FastLinkChange {
                peer_id: link.peer.clone(),
                link_id: link.id.clone(),
                role: link.role,
                state: link.state,
                reason,
            });
        }
    }

    pub(crate) fn has_directives(&self) -> bool {
        !self.directives.is_empty()
    }

    /// The next directive, its lifetime counted from `now`. A `host` or
    /// `join` whose time has run out is skipped: the tick ends its link.
    pub(crate) fn next_directive(&mut self, now: Instant) -> Option<FastLinkDirective> {
        let pending = loop {
            let pending = self.directives.pop_front()?;
            let spent = pending.deadline.is_some_and(|deadline| now >= deadline);
            if !spent || pending.directive.kind == FastLinkDirectiveKind::Close {
                break pending;
            }
        };
        if let Some(link) = self.link.as_mut() {
            if link.id == pending.directive.link_id
                && pending.directive.kind != FastLinkDirectiveKind::Close
            {
                link.platform_engaged = true;
            }
        }
        let mut directive = pending.directive;
        directive.expires_in_ms = pending
            .deadline
            .map(|deadline| deadline.saturating_duration_since(now).as_millis() as u64)
            .unwrap_or(0);
        Some(directive)
    }

    pub(crate) fn take_frames(&mut self) -> Vec<(String, Frame)> {
        self.frames.drain(..).collect()
    }

    pub(crate) fn take_changes(&mut self) -> Vec<FastLinkChange> {
        self.changes.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOW: &str = "off1aaaa";
    const HIGH: &str = "off1zzzz";

    fn fresh(seed: u8) -> FreshLink {
        let mut n = seed;
        FreshLink::generate(|buf| {
            for byte in buf {
                *byte = n;
                n = n.wrapping_add(1);
            }
        })
    }

    fn host_roles() -> FastLinkRoles {
        FastLinkRoles {
            can_host: true,
            can_join: true,
        }
    }

    fn join_roles() -> FastLinkRoles {
        FastLinkRoles {
            can_host: false,
            can_join: true,
        }
    }

    fn machine(roles: FastLinkRoles) -> FastLinks {
        let mut links = FastLinks::default();
        links.set_roles(roles);
        links
    }

    /// Carries every frame `from` queued to `to`, as the session would.
    fn deliver(
        from: &mut FastLinks,
        from_addr: &str,
        to: &mut FastLinks,
        to_addr: &str,
        now: Instant,
        now_ms: i64,
    ) -> usize {
        let frames = from.take_frames();
        let count = frames.len();
        for (recipient, frame) in frames {
            assert_eq!(recipient, to_addr);
            let wire = frame.encode();
            let parsed = Frame::parse(&wire).expect("every frame we send parses");
            to.on_frame(from_addr, to_addr, parsed, fresh(99), now, now_ms);
        }
        count
    }

    fn states(links: &mut FastLinks) -> Vec<FastLinkState> {
        links.take_changes().into_iter().map(|c| c.state).collect()
    }

    #[test]
    fn credentials_are_random_shaped_and_never_logged() {
        let link = fresh(7);
        assert!(is_link_id(&link.link_id));
        assert_eq!(link.passphrase.len(), PASSPHRASE_HEX_LEN);
        assert!(link.ssid.starts_with("DIRECT-"));
        assert_eq!(link.ssid.len(), 22);
        assert_eq!(&link.ssid[9..10], "-");
        assert!(link.ssid.len() <= 32);
        assert_ne!(fresh(7).passphrase, fresh(8).passphrase);
        let shown = format!("{link:?}");
        assert!(!shown.contains(&link.passphrase), "{shown}");
    }

    #[test]
    fn an_android_requester_hosts_and_an_iphone_joins() {
        let t0 = Instant::now();
        let mut android = machine(host_roles());
        let mut iphone = machine(join_roles());

        let id = android.request(HIGH, fresh(1), t0).unwrap();
        assert_eq!(states(&mut android), [FastLinkState::Hosting]);
        let host = android.next_directive(t0).unwrap();
        assert_eq!(host.kind, FastLinkDirectiveKind::Host);
        assert_eq!(host.peer_id, HIGH);
        assert_eq!(host.expires_in_ms, HOSTING_TIMEOUT.as_millis() as u64);
        // Nothing reaches the peer before the network is up.
        assert!(android.take_frames().is_empty());

        assert!(android.hosting(&id, "192.168.49.1", 8988, t0, 1_000));
        assert_eq!(states(&mut android), [FastLinkState::Offered]);
        assert_eq!(deliver(&mut android, LOW, &mut iphone, HIGH, t0, 1_000), 1);

        let join = iphone.next_directive(t0).unwrap();
        assert_eq!(join.kind, FastLinkDirectiveKind::Join);
        assert_eq!(join.link_id, id);
        assert_eq!(join.peer_id, LOW);
        assert_eq!(join.ssid, host.ssid);
        assert_eq!(join.passphrase, host.passphrase);
        assert_eq!(join.host.as_deref(), Some("192.168.49.1"));
        assert_eq!(join.port, Some(8988));
        assert_eq!(join.expires_in_ms, OFFER_TTL.as_millis() as u64);
        assert_eq!(states(&mut iphone), [FastLinkState::Joining]);

        android.stream_proved(HIGH, t0);
        iphone.stream_proved(LOW, t0);
        assert_eq!(states(&mut android), [FastLinkState::Up]);
        assert_eq!(states(&mut iphone), [FastLinkState::Up]);
    }

    #[test]
    fn an_iphone_requester_asks_the_android_to_host() {
        let t0 = Instant::now();
        let mut android = machine(host_roles());
        let mut iphone = machine(join_roles());

        let id = iphone.request(LOW, fresh(1), t0).unwrap();
        assert_eq!(states(&mut iphone), [FastLinkState::Requested]);
        assert!(
            !iphone.has_directives(),
            "a requester's platform has nothing to do yet"
        );
        deliver(&mut iphone, HIGH, &mut android, LOW, t0, 0);

        let host = android.next_directive(t0).unwrap();
        assert_eq!(host.kind, FastLinkDirectiveKind::Host);
        assert_eq!(host.link_id, id, "the host answers under the request's id");
        assert!(android.hosting(&id, "192.168.49.1", 8988, t0, 0));
        deliver(&mut android, LOW, &mut iphone, HIGH, t0, 0);

        let join = iphone.next_directive(t0).unwrap();
        assert_eq!(join.kind, FastLinkDirectiveKind::Join);
        assert_eq!(states(&mut iphone), [FastLinkState::Joining]);
    }

    #[test]
    fn a_device_that_can_do_neither_refuses_at_the_call() {
        let mut links = machine(FastLinkRoles::default());
        assert_eq!(
            links.request(HIGH, fresh(1), Instant::now()),
            Err(FastLinkRefusal::Unsupported)
        );
        assert!(links.take_frames().is_empty());
    }

    #[test]
    fn a_second_peer_is_refused_while_a_link_is_held() {
        let t0 = Instant::now();
        let mut links = machine(host_roles());
        let id = links.request(HIGH, fresh(1), t0).unwrap();
        assert_eq!(links.request(HIGH, fresh(2), t0), Ok(id));
        assert_eq!(
            links.request("off1other", fresh(3), t0),
            Err(FastLinkRefusal::Busy)
        );

        // A peer's request is answered busy, under its own id.
        let other = fresh(4).link_id;
        links.on_frame(
            "off1other",
            LOW,
            Frame::new(&other, FrameBody::Request),
            fresh(5),
            t0,
            0,
        );
        let frames = links.take_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, "off1other");
        assert_eq!(frames[0].1.link, other);
        assert_eq!(
            frames[0].1.body,
            FrameBody::Close {
                reason: "busy".to_string()
            }
        );
    }

    #[test]
    fn a_device_that_cannot_host_answers_a_request_so() {
        let mut iphone = machine(join_roles());
        let id = fresh(1).link_id;
        iphone.on_frame(
            LOW,
            HIGH,
            Frame::new(&id, FrameBody::Request),
            fresh(2),
            Instant::now(),
            0,
        );
        let frames = iphone.take_frames();
        assert_eq!(
            frames[0].1.body,
            FrameBody::Close {
                reason: "cannot_host".to_string()
            }
        );
        assert!(iphone.peer().is_none());
    }

    #[test]
    fn two_hosts_racing_leave_the_lower_address_hosting() {
        let t0 = Instant::now();
        let mut low = machine(host_roles());
        let mut high = machine(host_roles());
        let low_id = low.request(HIGH, fresh(1), t0).unwrap();
        let high_id = high.request(LOW, fresh(2), t0).unwrap();
        low.next_directive(t0);
        high.next_directive(t0);
        assert!(low.hosting(&low_id, "192.168.49.1", 8988, t0, 0));
        assert!(high.hosting(&high_id, "192.168.49.1", 8988, t0, 0));
        low.take_changes();
        high.take_changes();

        // Each receives the other's offer.
        let from_low = low.take_frames();
        let from_high = high.take_frames();
        for (_, frame) in from_high {
            low.on_frame(HIGH, LOW, frame, fresh(3), t0, 0);
        }
        for (_, frame) in from_low {
            high.on_frame(LOW, HIGH, frame, fresh(4), t0, 0);
        }

        assert!(
            low.take_frames().is_empty(),
            "the lower one ignores the offer"
        );
        assert_eq!(low.peer(), Some(HIGH));

        // The higher one closed its own (platform and peer) and joins.
        let closes: Vec<_> = high.take_frames();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].1.link, high_id);
        let close = high.next_directive(t0).unwrap();
        assert_eq!(close.kind, FastLinkDirectiveKind::Close);
        assert_eq!(close.link_id, high_id);
        let join = high.next_directive(t0).unwrap();
        assert_eq!(join.kind, FastLinkDirectiveKind::Join);
        assert_eq!(join.link_id, low_id);

        // The lower one ignores the close for the link it never held.
        for (_, frame) in closes {
            low.on_frame(HIGH, LOW, frame, fresh(5), t0, 0);
        }
        assert_eq!(low.peer(), Some(HIGH));
    }

    #[test]
    fn an_invalid_offer_is_answered_and_never_joined() {
        let t0 = Instant::now();
        let good = Offer {
            ssid: "DIRECT-ab-0123456789ab".to_string(),
            passphrase: "b".repeat(32),
            host: "192.168.49.1".to_string(),
            port: 8988,
            issued_at_ms: 1_000_000,
            ttl_ms: 120_000,
        };
        assert!(good.is_valid(1_000_000));
        let bad = [
            Offer {
                host: "8.8.8.8".to_string(),
                ..good.clone()
            },
            Offer {
                host: "printer.local".to_string(),
                ..good.clone()
            },
            Offer {
                passphrase: "short".to_string(),
                ..good.clone()
            },
            Offer {
                ssid: String::new(),
                ..good.clone()
            },
            Offer {
                port: 0,
                ..good.clone()
            },
            Offer {
                ttl_ms: MAX_OFFER_TTL_MS + 1,
                ..good.clone()
            },
            Offer {
                issued_at_ms: 1_000_000 + OFFER_CLOCK_SLACK_MS + 1,
                ..good.clone()
            },
            Offer {
                issued_at_ms: 1_000_000 - 120_000 - OFFER_CLOCK_SLACK_MS - 1,
                ..good.clone()
            },
        ];
        for offer in bad {
            let mut iphone = machine(join_roles());
            let id = fresh(1).link_id;
            iphone.on_frame(
                LOW,
                HIGH,
                Frame::new(&id, FrameBody::Offer(offer.clone())),
                fresh(2),
                t0,
                1_000_000,
            );
            assert!(!iphone.has_directives(), "joined {offer:?}");
            let frames = iphone.take_frames();
            assert_eq!(
                frames[0].1.body,
                FrameBody::Close {
                    reason: "invalid".to_string()
                },
                "{offer:?}"
            );
        }
        assert!(is_local_host("fe80::1"));
        assert!(is_local_host("fd00::1"));
        assert!(is_local_host("169.254.3.4"));
        assert!(!is_local_host("2001:db8::1"));
    }

    #[test]
    fn frames_round_trip_and_unknown_shapes_are_refused() {
        let id = fresh(1).link_id;
        for body in [
            FrameBody::Request,
            FrameBody::Close {
                reason: "idle".to_string(),
            },
        ] {
            let frame = Frame::new(&id, body);
            assert_eq!(Frame::parse(&frame.encode()), Some(frame));
        }
        let wire = Frame::new(&id, FrameBody::Request).encode();
        assert!(wire.starts_with("{\"v\":1,\"link\":"), "{wire}");
        assert!(Frame::parse(&wire.replace("\"v\":1", "\"v\":2")).is_none());
        assert!(Frame::parse(&wire.replace(&id, "NOT-A-LINK-ID")).is_none());
        assert!(
            Frame::parse(&format!("{{\"v\":1,\"link\":\"{id}\",\"kind\":\"dance\"}}")).is_none()
        );
        assert!(Frame::parse(&" ".repeat(MAX_FRAME_BODY_BYTES + 1)).is_none());
        // A later field is ignored, not refused.
        let later = wire.replacen('{', "{\"later\":true,", 1);
        assert!(Frame::parse(&later).is_some());
        assert_eq!(
            CloseReason::parse("a reason from 2030"),
            CloseReason::Failed
        );
    }

    #[test]
    fn every_end_tells_the_platform_and_the_peer_once() {
        let t0 = Instant::now();
        let mut android = machine(host_roles());
        let id = android.request(HIGH, fresh(1), t0).unwrap();
        android.next_directive(t0);
        assert!(android.hosting(&id, "192.168.49.1", 8988, t0, 0));
        android.take_frames();
        android.stream_proved(HIGH, t0);
        android.take_changes();

        android.stream_lost(HIGH, t0);
        let close = android.next_directive(t0).unwrap();
        assert_eq!(close.kind, FastLinkDirectiveKind::Close);
        assert_eq!(close.reason, Some(CloseReason::Lost));
        assert!(android.next_directive(t0).is_none());
        let frames = android.take_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].1.body,
            FrameBody::Close {
                reason: "lost".to_string()
            }
        );
        let changes = android.take_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].state, FastLinkState::Closed);
        assert_eq!(changes[0].reason, Some(CloseReason::Lost));
        assert!(android.peer().is_none());
    }

    #[test]
    fn a_close_from_the_peer_is_not_echoed() {
        let t0 = Instant::now();
        let mut iphone = machine(join_roles());
        let id = iphone.request(LOW, fresh(1), t0).unwrap();
        iphone.take_frames();
        iphone.on_frame(
            LOW,
            HIGH,
            Frame::new(
                &id,
                FrameBody::Close {
                    reason: "cannot_host".to_string(),
                },
            ),
            fresh(2),
            t0,
            0,
        );
        assert!(iphone.take_frames().is_empty());
        assert!(
            !iphone.has_directives(),
            "a requester's platform held nothing"
        );
        let changes = iphone.take_changes();
        assert_eq!(
            changes.last().unwrap().reason,
            Some(CloseReason::CannotHost)
        );
    }

    #[test]
    fn a_platform_report_ends_the_link_without_a_directive() {
        let t0 = Instant::now();
        let mut iphone = machine(join_roles());
        let id = fresh(1).link_id;
        let offer = Offer {
            ssid: "DIRECT-ab-0123456789ab".to_string(),
            passphrase: "b".repeat(32),
            host: "192.168.49.1".to_string(),
            port: 8988,
            issued_at_ms: 0,
            ttl_ms: 120_000,
        };
        iphone.on_frame(
            LOW,
            HIGH,
            Frame::new(&id, FrameBody::Offer(offer)),
            fresh(2),
            t0,
            0,
        );
        iphone.next_directive(t0);
        assert!(iphone.platform_ended(&id, CloseReason::Declined, t0));
        assert!(!iphone.has_directives());
        let frames = iphone.take_frames();
        assert_eq!(
            frames[0].1.body,
            FrameBody::Close {
                reason: "declined".to_string()
            }
        );
    }

    #[test]
    fn timers_end_each_waiting_state() {
        let t0 = Instant::now();

        let mut host = machine(host_roles());
        host.request(HIGH, fresh(1), t0).unwrap();
        host.tick(t0 + HOSTING_TIMEOUT, |_| false);
        assert_eq!(
            host.take_changes().last().unwrap().reason,
            Some(CloseReason::HostFailed)
        );

        let mut host = machine(host_roles());
        let id = host.request(HIGH, fresh(1), t0).unwrap();
        host.hosting(&id, "192.168.49.1", 8988, t0, 0);
        host.tick(t0 + OFFER_TTL - Duration::from_secs(1), |_| false);
        assert!(host.peer().is_some());
        host.tick(t0 + OFFER_TTL, |_| false);
        assert_eq!(
            host.take_changes().last().unwrap().reason,
            Some(CloseReason::Expired)
        );

        let mut joiner = machine(join_roles());
        joiner.request(LOW, fresh(1), t0).unwrap();
        joiner.tick(t0 + REQUEST_TIMEOUT, |_| false);
        assert_eq!(
            joiner.take_changes().last().unwrap().reason,
            Some(CloseReason::NoOffer)
        );
    }

    #[test]
    fn an_idle_link_ends_unless_a_transfer_holds_it() {
        let t0 = Instant::now();
        let mut host = machine(host_roles());
        let id = host.request(HIGH, fresh(1), t0).unwrap();
        host.hosting(&id, "192.168.49.1", 8988, t0, 0);
        host.stream_proved(HIGH, t0);
        host.take_changes();

        host.activity(HIGH, ACTIVITY_MIN_BYTES - 1, t0 + Duration::from_secs(20));
        host.activity(HIGH, ACTIVITY_MIN_BYTES, t0 + Duration::from_secs(30));
        host.activity(HIGH, 300, t0 + Duration::from_secs(80));
        host.tick(t0 + IDLE_LIMIT, |_| false);
        assert!(host.peer().is_some(), "a large body moved the idle clock");
        host.tick(t0 + Duration::from_secs(30) + IDLE_LIMIT, |_| false);
        assert_eq!(
            host.take_changes().last().unwrap().reason,
            Some(CloseReason::Idle),
            "presence-sized chatter does not hold a link up"
        );

        let id = host.request(HIGH, fresh(2), t0).unwrap();
        host.hosting(&id, "192.168.49.1", 8988, t0, 0);
        host.stream_proved(HIGH, t0);
        host.take_changes();

        host.tick(t0 + Duration::from_secs(200), |peer| peer == HIGH);
        assert!(host.peer().is_some(), "a transfer in flight is not idle");

        host.tick(t0 + Duration::from_secs(200) + IDLE_LIMIT, |_| false);
        assert_eq!(
            host.take_changes().last().unwrap().reason,
            Some(CloseReason::Idle)
        );
    }

    #[test]
    fn a_repeated_request_gets_the_same_offer_again() {
        let t0 = Instant::now();
        let mut android = machine(host_roles());
        let id = fresh(1).link_id;
        android.on_frame(
            HIGH,
            LOW,
            Frame::new(&id, FrameBody::Request),
            fresh(2),
            t0,
            0,
        );
        android.hosting(&id, "192.168.49.1", 8988, t0, 5);
        let first = android.take_frames();
        android.on_frame(
            HIGH,
            LOW,
            Frame::new(&id, FrameBody::Request),
            fresh(3),
            t0,
            0,
        );
        let second = android.take_frames();
        assert_eq!(first, second);
        assert_eq!(
            android.next_directive(t0).unwrap().kind,
            FastLinkDirectiveKind::Host
        );
        assert!(android.next_directive(t0).is_none(), "one network, not two");
    }

    #[test]
    fn a_late_offer_for_a_link_given_up_on_is_refused_not_joined() {
        let t0 = Instant::now();
        let mut android = machine(host_roles());
        let mut iphone = machine(join_roles());
        let id = iphone.request(LOW, fresh(1), t0).unwrap();
        let request = iphone.take_frames();
        iphone.tick(t0 + REQUEST_TIMEOUT, |_| false);
        iphone.take_frames();
        iphone.take_changes();

        // The request arrives after the requester gave up, and is hosted.
        for (_, frame) in request {
            android.on_frame(HIGH, LOW, frame, fresh(2), t0 + REQUEST_TIMEOUT, 0);
        }
        android.next_directive(t0);
        assert!(android.hosting(&id, "192.168.49.1", 8988, t0 + REQUEST_TIMEOUT, 0));
        let late = REQUEST_TIMEOUT + Duration::from_secs(1);
        deliver(&mut android, LOW, &mut iphone, HIGH, t0 + late, 0);

        assert!(
            !iphone.has_directives(),
            "the user was prompted for a link nobody waits on"
        );
        assert!(iphone.take_changes().is_empty());
        let answer = iphone.take_frames();
        assert_eq!(answer.len(), 1);
        assert_eq!(
            answer[0].1.body,
            FrameBody::Close {
                reason: "expired".to_string()
            }
        );
        // The answer takes the host's network down.
        for (_, frame) in answer {
            android.on_frame(HIGH, LOW, frame, fresh(3), t0 + late, 0);
        }
        assert_eq!(
            android.next_directive(t0).unwrap().kind,
            FastLinkDirectiveKind::Close
        );

        // Remembered for a while, then forgotten.
        assert!(iphone.ended_recently(
            &id,
            t0 + REQUEST_TIMEOUT + ENDED_MEMORY - Duration::from_secs(1)
        ));
        assert!(!iphone.ended_recently(&id, t0 + REQUEST_TIMEOUT + ENDED_MEMORY));
    }

    #[test]
    fn a_late_request_for_an_ended_link_is_not_hosted_again() {
        let t0 = Instant::now();
        let mut android = machine(host_roles());
        let id = fresh(1).link_id;
        android.on_frame(
            HIGH,
            LOW,
            Frame::new(&id, FrameBody::Request),
            fresh(2),
            t0,
            0,
        );
        android.next_directive(t0);
        android.tick(t0 + HOSTING_TIMEOUT, |_| false);
        android.next_directive(t0);
        android.take_frames();

        android.on_frame(
            HIGH,
            LOW,
            Frame::new(&id, FrameBody::Request),
            fresh(3),
            t0 + HOSTING_TIMEOUT,
            0,
        );
        assert!(!android.has_directives());
        assert!(android.take_frames().is_empty());
        assert!(android.peer().is_none());
    }

    #[test]
    fn a_directive_the_platform_never_took_is_withdrawn_with_its_link() {
        let t0 = Instant::now();
        // A paused platform drains nothing while the link times out.
        let mut android = machine(host_roles());
        android.request(HIGH, fresh(1), t0).unwrap();
        android.tick(t0 + HOSTING_TIMEOUT, |_| false);
        assert!(
            android.next_directive(t0 + HOSTING_TIMEOUT).is_none(),
            "no stale host, and no close owed"
        );

        // A join whose time ran out before it was taken is skipped.
        let mut iphone = machine(join_roles());
        let id = fresh(2).link_id;
        let offer = Offer {
            ssid: "DIRECT-ab-0123456789ab".to_string(),
            passphrase: "b".repeat(32),
            host: "192.168.49.1".to_string(),
            port: 8988,
            issued_at_ms: 0,
            ttl_ms: 120_000,
        };
        iphone.on_frame(
            LOW,
            HIGH,
            Frame::new(&id, FrameBody::Offer(offer)),
            fresh(3),
            t0,
            0,
        );
        assert!(
            iphone.next_directive(t0 + OFFER_TTL).is_none(),
            "no prompt for a spent offer"
        );
        iphone.tick(t0 + OFFER_TTL, |_| false);
        assert!(
            iphone.next_directive(t0 + OFFER_TTL).is_none(),
            "nothing was taken, so no close"
        );
    }

    #[test]
    fn a_declined_peer_cannot_prompt_again_until_the_user_asks() {
        let t0 = Instant::now();
        let offer = |id: &str, links: &mut FastLinks, at: Instant| {
            let offer = Offer {
                ssid: "DIRECT-ab-0123456789ab".to_string(),
                passphrase: "b".repeat(32),
                host: "192.168.49.1".to_string(),
                port: 8988,
                issued_at_ms: 0,
                ttl_ms: 120_000,
            };
            links.on_frame(
                LOW,
                HIGH,
                Frame::new(id, FrameBody::Offer(offer)),
                fresh(9),
                at,
                0,
            );
        };
        let mut iphone = machine(join_roles());
        let first = fresh(1).link_id;
        offer(&first, &mut iphone, t0);
        iphone.next_directive(t0);
        assert!(iphone.platform_ended(&first, CloseReason::Declined, t0));
        iphone.take_frames();

        // Another unasked-for offer at once: refused, no prompt.
        let second = fresh(2).link_id;
        offer(&second, &mut iphone, t0 + Duration::from_secs(1));
        assert!(!iphone.has_directives());
        assert_eq!(
            iphone.take_frames()[0].1.body,
            FrameBody::Close {
                reason: "declined".to_string()
            }
        );

        // After the backoff it may prompt again.
        let third = fresh(3).link_id;
        offer(&third, &mut iphone, t0 + DECLINE_BACKOFF);
        assert_eq!(
            iphone.next_directive(t0 + DECLINE_BACKOFF).unwrap().kind,
            FastLinkDirectiveKind::Join
        );
        iphone.close(LOW, t0 + DECLINE_BACKOFF);
        iphone.take_frames();

        // And the user's own request clears it at once.
        let fourth = fresh(4).link_id;
        offer(&fourth, &mut iphone, t0 + DECLINE_BACKOFF);
        iphone.next_directive(t0);
        iphone.platform_ended(&fourth, CloseReason::Declined, t0 + DECLINE_BACKOFF);
        let asked = iphone.request(LOW, fresh(5), t0 + DECLINE_BACKOFF).unwrap();
        iphone.take_frames();
        offer(
            &asked,
            &mut iphone,
            t0 + DECLINE_BACKOFF + Duration::from_secs(1),
        );
        assert_eq!(
            iphone.next_directive(t0 + DECLINE_BACKOFF).unwrap().kind,
            FastLinkDirectiveKind::Join
        );
    }

    #[test]
    fn a_close_that_overtakes_its_offer_keeps_the_offer_from_prompting() {
        let t0 = Instant::now();
        let mut iphone = machine(join_roles());
        let id = fresh(1).link_id;
        iphone.on_frame(
            LOW,
            HIGH,
            Frame::new(
                &id,
                FrameBody::Close {
                    reason: "expired".to_string(),
                },
            ),
            fresh(2),
            t0,
            0,
        );
        let offer = Offer {
            ssid: "DIRECT-ab-0123456789ab".to_string(),
            passphrase: "b".repeat(32),
            host: "192.168.49.1".to_string(),
            port: 8988,
            issued_at_ms: 0,
            ttl_ms: 120_000,
        };
        iphone.on_frame(
            LOW,
            HIGH,
            Frame::new(&id, FrameBody::Offer(offer)),
            fresh(3),
            t0,
            0,
        );
        assert!(!iphone.has_directives());
    }

    #[test]
    fn a_host_address_off_the_network_is_never_offered() {
        let t0 = Instant::now();
        let mut android = machine(host_roles());
        let id = android.request(HIGH, fresh(1), t0).unwrap();
        android.next_directive(t0);
        assert!(!android.hosting(&id, "8.8.8.8", 8988, t0, 0));
        assert!(
            android.take_frames().is_empty(),
            "nothing offered, and the peer knew nothing"
        );
        assert_eq!(
            android.next_directive(t0).unwrap().kind,
            FastLinkDirectiveKind::Close
        );
    }
}
