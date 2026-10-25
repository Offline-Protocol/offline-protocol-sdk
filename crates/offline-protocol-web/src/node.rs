//! One engine and the links that carry it.
//!
//! A browser has no Bluetooth peripheral and no Wi-Fi Direct, and what it does
//! have (a `BroadcastChannel` between tabs, a WebRTC data channel between
//! devices) delivers whole messages between two endpoints the page set up.
//! Each one is a peer stream in the sense of `docs/spec/stream-framing.md`,
//! carried in the engine's peer-stream slot (`wifi_direct`, named for
//! history). A node the application gives relays also carries the Nostr
//! slot, driven from the worker ([`crate::nostr_host`]).
//!
//! # Invariants
//!
//! - **The page moves bytes; this module decides what they mean.** A link is
//!   an opaque id the page allocated a carrier for. Everything the chapter
//!   requires of a peer stream happens here, in the one Rust implementation
//!   of it ([`PeerStreamReader`] and [`frame`]): the identity preamble first
//!   in each direction, verified before anything is announced; the length
//!   prefix on every frame, even on a carrier that already has message
//!   boundaries; and nothing delivered from a link whose preamble did not
//!   verify. A JavaScript copy of those rules would be a second
//!   implementation that the conformance vectors never see.
//! - **One link per address.** A second link that proves an address already
//!   announced is refused, so the engine sees one announcement and one loss
//!   per peer however many carriers the page opened toward it.
//! - **A link that has not proved anyone is temporary.** At most
//!   [`MAX_LINKS`] are open, and one whose preamble has not verified within
//!   [`PREAMBLE_DEADLINE`] is closed. Both are the stream-framing chapter's
//!   local policy, at the mobile managers' values; without them a carrier
//!   that opens links and never speaks holds memory and a preamble each for
//!   as long as the worker lives. The page learns of a close the engine
//!   decided from [`Node::next_closed`], and tells the other end.
//! - **Nothing calls back into the page.** Events and outbound frames wait in
//!   queues the page drains. The engine fires its own callbacks while it holds
//!   its state lock, and a page that answered one by calling back in would
//!   re-enter that lock on the only thread there is.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use offline_protocol::mls::{InMemoryStorage, MlsStorage};
use offline_protocol::{
    Event, GroupInfo, GroupSendOptions, OfflineProtocol, ProtocolConfig, ProtocolStateStorage,
    SendMessageOptions,
};
use offline_protocol_core::time::{Duration, Instant};
use offline_protocol_transport::{
    frame, BleTransport, NostrTransport, PeerStreamReader, StreamEvent, Transport, TransportStatus,
    TransportType, WifiDirectTransport,
};
use tracing::warn;

use crate::ble_host::BleHost;
use crate::memory_store::MemoryProtocolStateStorage;
use crate::nostr_host::{NostrHost, NostrOptions};

/// The page's handle for one carrier.
pub type LinkId = u32;

/// Events held for the page, at most. The engine emits continuously and a
/// page that stops polling must not grow the worker without bound; the oldest
/// go first, as in the mobile bridges' polling queue.
const MAX_QUEUED_EVENTS: usize = 1024;

/// Inbound messages one [`Node::process`] call hands to the engine. Matches
/// the mobile tickers' drain cap, so one tick's cost is bounded the same way.
const MAX_RECEIVES_PER_TICK: usize = 100;

/// Links open at once, at most, as the mobile managers' sixteen streams.
pub const MAX_LINKS: usize = 16;

/// How long a link may stay open without a verified preamble, as the mobile
/// managers' ten seconds.
pub const PREAMBLE_DEADLINE: Duration = Duration::from_secs(10);

/// What went wrong, as one string the binding turns into a JavaScript error.
#[derive(Debug)]
pub struct NodeError(pub String);

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NodeError {}

impl From<offline_protocol::Error> for NodeError {
    fn from(error: offline_protocol::Error) -> Self {
        Self(error.to_string())
    }
}

pub(crate) type NodeResult<T> = Result<T, NodeError>;

/// Where [`Node::establish_secure_session`] left a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStart {
    /// This call created it and sent the Welcome.
    Created,
    /// One already existed.
    Existing,
    /// Waiting for the peer's key package.
    Pending,
}

impl SessionStart {
    /// The name the page sees.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Existing => "existing",
            Self::Pending => "pending",
        }
    }
}

/// What a node carries beyond its peer-stream links.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeOptions {
    /// The Nostr slot, when the application gives the node relays.
    pub nostr: Option<NostrOptions>,
    /// The Bluetooth LE slot, driven from the page over Web Bluetooth.
    pub ble: bool,
}

struct Link {
    reader: PeerStreamReader,
    /// The address this link's preamble proved, once it has.
    peer: Option<String>,
    /// When the page opened it, for the preamble deadline.
    opened: Instant,
}

/// One engine, its links, and the queues the page drains.
pub struct Node {
    pub(crate) protocol: OfflineProtocol,
    pub(crate) events: Arc<Mutex<VecDeque<String>>>,
    /// The worker's half of the Nostr slot, when the node has one.
    pub(crate) nostr_host: Option<NostrHost>,
    /// The worker's half of the Bluetooth slot, when the node has one.
    pub(crate) ble_host: Option<BleHost>,
    links: BTreeMap<LinkId, Link>,
    /// The one live link for each announced address.
    peers: HashMap<String, LinkId>,
    /// Frames this module produced itself (preambles), ahead of the engine's.
    outbound: VecDeque<(LinkId, Vec<u8>)>,
    /// Links this module closed on its own (the preamble deadline), for the
    /// page to close the carrier and tell the other end.
    closed: VecDeque<LinkId>,
    preamble_deadline: Duration,
    next_link: LinkId,
}

impl Node {
    /// A stopped engine with no identity yet, carrying peer-stream links only.
    pub fn new(app_id: &str, profile: &str) -> NodeResult<Self> {
        Self::with_options(app_id, profile, NodeOptions::default())
    }

    /// A stopped engine with no identity yet.
    ///
    /// The peer-stream slot is always registered. The Nostr slot is
    /// registered once the address exists, when `options.nostr` is set: the
    /// transport derives its routing tag from the address it is built with.
    /// The Bluetooth slot is registered with it when `options.ble` is set:
    /// the page is a central over Web Bluetooth, never a peripheral. The
    /// internet relay's socket is not wired.
    pub fn with_options(app_id: &str, profile: &str, options: NodeOptions) -> NodeResult<Self> {
        let mut config = ProtocolConfig::new(app_id, profile);
        config.transport.ble_enabled = options.ble;
        config.transport.internet_enabled = false;
        config.transport.reticulum_enabled = false;
        config.transport.nostr_enabled = options.nostr.is_some();
        if let Some(nostr) = options.nostr {
            config.transport.nostr_cold_contact_enabled = nostr.cold_contact;
            config.transport.nostr_username_discovery_enabled = nostr.username_discovery;
        }
        config.transport.wifi_direct_enabled = true;

        let mut protocol = OfflineProtocol::new(config)?;
        // A placeholder id until `initialize` derives the address, exactly as
        // the FFI crate does; the transport is rebuilt against the address.
        protocol.transport_manager_mut().add_transport(
            TransportType::WiFiDirect,
            Box::new(WifiDirectTransport::new(profile)),
        );

        let events = Arc::new(Mutex::new(VecDeque::new()));
        let sink = events.clone();
        protocol.on_event(move |event| push_event(&sink, &event));

        Ok(Self {
            protocol,
            events,
            nostr_host: options.nostr.map(|_| NostrHost::default()),
            ble_host: options.ble.then(BleHost::default),
            links: BTreeMap::new(),
            peers: HashMap::new(),
            outbound: VecDeque::new(),
            closed: VecDeque::new(),
            preamble_deadline: PREAMBLE_DEADLINE,
            next_link: 1,
        })
    }

    /// Mints this device's identity over in-memory stores and returns its
    /// address: a new device every time the worker starts.
    pub fn initialize(&mut self) -> NodeResult<String> {
        self.initialize_with(
            Arc::new(InMemoryStorage::new()),
            Arc::new(MemoryProtocolStateStorage::new()),
        )
    }

    /// Loads this device's identity from `mls`, minting it on the first run,
    /// and returns its address. Must run before [`Self::start`]: the address
    /// replaces the placeholder the transport was built on.
    ///
    /// The two stores must be separate instances; one backend behind both
    /// handles lets one category collide with the other's.
    pub fn initialize_with(
        &mut self,
        mls: Arc<dyn MlsStorage>,
        state: Arc<dyn ProtocolStateStorage>,
    ) -> NodeResult<String> {
        if let Some(address) = self.protocol.local_address() {
            return Ok(address.to_string());
        }
        self.protocol.initialize_mls(mls, state)?;
        let address = self
            .protocol
            .local_address()
            .ok_or_else(|| NodeError("MLS initialized without an address".to_string()))?
            .to_string();
        // Every replacement is built before any is installed, as the FFI
        // crate does, so the one fallible construction cannot leave the set
        // half rebuilt.
        let nostr = match self.nostr_host {
            Some(_) => Some(
                NostrTransport::new(address.as_str())
                    .map_err(|e| NodeError(format!("could not create the Nostr transport: {e}")))?,
            ),
            None => None,
        };
        self.protocol.transport_manager_mut().add_transport(
            TransportType::WiFiDirect,
            Box::new(WifiDirectTransport::new(address.as_str())),
        );
        if self.ble_host.is_some() {
            self.protocol.transport_manager_mut().add_transport(
                TransportType::BLE,
                Box::new(BleTransport::new(address.as_str())),
            );
        }
        if let Some(nostr) = nostr {
            self.protocol
                .transport_manager_mut()
                .add_transport(TransportType::Nostr, Box::new(nostr));
            // The transport did not exist while `initialize_mls` restored
            // state, so the persisted signing secret and receive watermark go
            // in now. Without this the install's Nostr identity rotates on
            // every start, stranding peers that seal to the key it advertised.
            self.protocol.restore_nostr_transport_state();
        }
        Ok(address)
    }

    /// Starts the engine and reports the peer-stream layer up. A link is
    /// announced only once its preamble verifies, so "up" here promises no
    /// peer.
    pub fn start(&mut self) -> NodeResult<()> {
        self.protocol.start()?;
        if let Some(transport) = self.slot() {
            transport.on_status_changed(TransportStatus::Available);
        }
        // Up as the peer-stream slot is: a link is announced only once its
        // peripheral's identity verified, so "up" promises no peer.
        if self.ble_host.is_some() {
            if let Some(transport) = self
                .protocol
                .transport_manager()
                .get_transport(TransportType::BLE)
            {
                transport.on_status_changed(TransportStatus::Available);
            }
        }
        Ok(())
    }

    /// Stops the engine. Every link is reported lost first, so the engine's
    /// view of who is reachable ends empty rather than stale.
    pub fn stop(&mut self) -> NodeResult<()> {
        let links: Vec<LinkId> = self.links.keys().copied().collect();
        for link in links {
            self.close_link(link);
        }
        self.protocol.stop()?;
        Ok(())
    }

    /// This device's address, once [`Self::initialize`] has run.
    pub fn local_address(&self) -> Option<String> {
        self.protocol.local_address().map(str::to_string)
    }

    /// One tick: links past the preamble deadline, the engine's timers, then
    /// up to 100 queued inbound messages. The page calls this every 100 ms,
    /// as both mobile bridges do.
    pub fn process(&mut self) -> NodeResult<()> {
        let silent: Vec<LinkId> = self
            .links
            .iter()
            .filter(|(_, state)| {
                state.peer.is_none() && state.opened.elapsed() >= self.preamble_deadline
            })
            .map(|(&link, _)| link)
            .collect();
        for link in silent {
            self.close_link(link);
            self.closed.push_back(link);
        }
        self.protocol.process()?;
        for _ in 0..MAX_RECEIVES_PER_TICK {
            if self.protocol.receive_message().is_none() {
                break;
            }
        }
        Ok(())
    }

    /// The next event, as the engine's JSON.
    pub fn poll_event(&self) -> Option<String> {
        self.events.lock().ok()?.pop_front()
    }

    /// Registers a carrier the page opened and queues this device's preamble
    /// on it. The page sends whatever [`Self::next_outbound`] yields for the
    /// returned id. Refused once [`MAX_LINKS`] are open.
    pub fn open_link(&mut self) -> NodeResult<LinkId> {
        if self.links.len() >= MAX_LINKS {
            return Err(NodeError(format!("{MAX_LINKS} links are already open")));
        }
        let preamble = self.preamble()?;
        let link = self.next_link;
        self.next_link = self
            .next_link
            .checked_add(1)
            .ok_or_else(|| NodeError("link ids exhausted".to_string()))?;
        let verify = Box::new(|body: &[u8]| {
            offline_protocol_mls::verify_identity_assertion(body)
                .ok()
                .map(|address| address.to_string())
        });
        self.links.insert(
            link,
            Link {
                reader: PeerStreamReader::new(verify),
                peer: None,
                opened: Instant::now(),
            },
        );
        self.outbound.push_back((link, preamble));
        Ok(link)
    }

    /// Bytes the carrier delivered on `link`. Returns why the link must close,
    /// or `None` to keep it open. A link that closes delivers nothing more,
    /// and the page closes the carrier.
    pub fn link_received(&mut self, link: LinkId, bytes: &[u8]) -> Option<String> {
        let events = match self.links.get_mut(&link) {
            Some(state) => state.reader.feed(bytes),
            None => return Some("unknown link".to_string()),
        };
        for event in events {
            match event {
                StreamEvent::Announced(address) => {
                    if let Some(reason) = self.announce(link, address) {
                        self.close_link(link);
                        return Some(reason);
                    }
                }
                StreamEvent::Message { from, body } => self.deliver(from, body),
                StreamEvent::Closed(reason) => {
                    self.close_link(link);
                    return Some(reason.to_string());
                }
            }
        }
        None
    }

    /// The carrier for `link` is gone. Reports its peer lost if this was the
    /// link the peer was announced on.
    pub fn close_link(&mut self, link: LinkId) {
        let Some(state) = self.links.remove(&link) else {
            return;
        };
        self.outbound.retain(|(queued, _)| *queued != link);
        let Some(peer) = state.peer else {
            return;
        };
        if self.peers.get(&peer) != Some(&link) {
            return;
        }
        self.peers.remove(&peer);
        if let Some(transport) = self.wifi_direct() {
            transport.on_peer_disconnected(&peer);
        }
        // Still reachable over Bluetooth: not lost, as the FFI decides.
        if self.ble_linked(&peer) {
            return;
        }
        self.protocol.on_neighbor_lost(&peer);
        push_event(&self.events, &Event::NeighborLost { peer_id: peer });
    }

    /// The next link this module closed on its own. The page closes its
    /// carrier and tells the other end, exactly as for a link
    /// [`Self::link_received`] refused.
    pub fn next_closed(&mut self) -> Option<LinkId> {
        self.closed.pop_front()
    }

    /// Shortens the preamble deadline, so a test need not wait ten seconds.
    #[cfg(test)]
    pub(crate) fn set_preamble_deadline(&mut self, deadline: Duration) {
        self.preamble_deadline = deadline;
    }

    /// The next frame to put on a carrier: this module's own preambles first,
    /// then the engine's queue, each framed for the link its recipient was
    /// announced on.
    pub fn next_outbound(&mut self) -> Option<(LinkId, Vec<u8>)> {
        if let Some(frame) = self.outbound.pop_front() {
            return Some(frame);
        }
        let transport = self.slot()?;
        loop {
            let (recipient, body) = match transport.get_next_message() {
                Ok(Some(next)) => next,
                Ok(None) => return None,
                Err(error) => {
                    warn!(%error, "peer stream: could not serialize an outbound message");
                    continue;
                }
            };
            // The transport queues only for a proved recipient, so a miss is
            // a link that closed after the engine queued for it.
            let Some(&link) = self.peers.get(&recipient) else {
                warn!(%recipient, "peer stream: dropping a frame for a closed link");
                continue;
            };
            match frame(&body) {
                Ok(framed) => return Some((link, framed)),
                Err(error) => warn!(%error, "peer stream: refusing an unframeable message"),
            }
        }
    }

    /// Sends a direct message, end-to-end encrypted once a session exists.
    ///
    /// `ephemeral` sends it once over the live link to `recipient` and never
    /// retries it (ADR 0032): for state the next send replaces. It fails at
    /// once with no link or no confirmed session.
    pub fn send_message(
        &mut self,
        recipient: &str,
        content: &str,
        ephemeral: bool,
    ) -> NodeResult<String> {
        self.reach_over_nostr(recipient);
        let id = self.protocol.send_message_with(
            recipient,
            content,
            SendMessageOptions {
                ephemeral,
                ..Default::default()
            },
        )?;
        Ok(id.to_string())
    }

    /// Creates a group this device administers, as JSON group info.
    pub fn create_group(&mut self, name: &str) -> NodeResult<String> {
        let info: GroupInfo = self.protocol.create_group(name)?;
        serde_json::to_string(&info).map_err(|e| NodeError(e.to_string()))
    }

    /// Adds `member` to `group_id`. Fails with the engine's no-key-package
    /// error until the member's post-session key package has arrived.
    pub fn invite_to_group(&mut self, group_id: &str, member: &str) -> NodeResult<()> {
        self.protocol.invite_to_group(group_id, member)?;
        Ok(())
    }

    /// Removes `member` from `group_id`, which this device administers. The
    /// remaining members move to a new epoch whose keys the removed member
    /// never holds, and the removed member is told it was removed.
    pub fn remove_from_group(&mut self, group_id: &str, member: &str) -> NodeResult<()> {
        self.protocol.remove_from_group(group_id, member)?;
        Ok(())
    }

    /// A group this device belongs to, as JSON: its members and its current
    /// epoch. `None` for a group this device does not hold.
    pub fn group_info(&self, group_id: &str) -> NodeResult<Option<String>> {
        let Some(info) = self.protocol.get_group_info(group_id)? else {
            return Ok(None);
        };
        serde_json::to_string(&info)
            .map(Some)
            .map_err(|e| NodeError(e.to_string()))
    }

    /// Sends to every member of `group_id`; returns the per-member ids as a
    /// JSON array. `ephemeral` as for [`Self::send_message`], to every member
    /// holding a live link.
    pub fn send_group_message(
        &mut self,
        group_id: &str,
        content: &str,
        ephemeral: bool,
    ) -> NodeResult<String> {
        let ids = self.protocol.send_group_message_with(
            group_id,
            content,
            GroupSendOptions {
                ephemeral,
                ..Default::default()
            },
        )?;
        let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
        serde_json::to_string(&ids).map_err(|e| NodeError(e.to_string()))
    }

    /// Starts, or advances, a session with `peer`, and says where it stands:
    ///
    /// - [`SessionStart::Created`]: this call built it and sent the Welcome;
    /// - [`SessionStart::Existing`]: there already is one;
    /// - [`SessionStart::Pending`]: no key package from `peer` yet. On a node
    ///   with relays this call has asked them for the peer's published
    ///   packages, and `secure_session_established` follows once one arrives.
    ///
    /// Pending is not an error here, as it is in the engine: for a device met
    /// only by its address it is the ordinary first answer.
    pub fn establish_secure_session(&mut self, peer: &str) -> NodeResult<SessionStart> {
        self.reach_over_nostr(peer);
        match self.protocol.establish_secure_session(peer) {
            Ok(Some(_)) => Ok(SessionStart::Created),
            Ok(None) => Ok(SessionStart::Existing),
            Err(offline_protocol::Error::SessionNotReady(_)) => Ok(SessionStart::Pending),
            Err(error) => Err(error.into()),
        }
    }

    /// Resolves a username to every device claiming it. `true` started the
    /// lookup, `false` joined one in flight; either way exactly one
    /// `username_resolved` event follows.
    pub fn resolve_username(&mut self, username: &str) -> NodeResult<bool> {
        Ok(self.protocol.resolve_username(username)?)
    }

    /// An invite for this device: its address and key, an optional petname,
    /// and, when `sign` is set, a signature binding the petname to the key.
    pub fn create_invite(&self, petname: Option<&str>, sign: bool) -> NodeResult<String> {
        let manager = self
            .protocol
            .mls_manager()
            .ok_or_else(|| NodeError("initialize before creating an invite".to_string()))?;
        let guard = manager
            .read()
            .map_err(|_| NodeError("MLS manager lock poisoned".to_string()))?;
        let mls = |e: offline_protocol_mls::MlsError| NodeError(e.to_string());
        let public_key = guard.get_identity_public_key().map_err(mls)?;
        let address = offline_protocol_mls::MlsManager::derive_address(&public_key).map_err(mls)?;
        let signature = if sign {
            let payload = offline_protocol_mls::invite::invite_signing_payload(
                &address,
                &public_key,
                petname,
            )
            .map_err(mls)?;
            Some(guard.sign_data(&payload).map_err(mls)?)
        } else {
            None
        };
        offline_protocol_mls::invite::encode_invite(
            &address,
            &public_key,
            petname,
            signature.as_deref(),
        )
        .map_err(mls)
    }

    /// Asks the relays for `peer`'s published key packages when there is no
    /// session with it yet and no live link to it. Nothing else does: a
    /// session-less send only queues, and the engine's own key-package push
    /// reaches only the mesh. That is a core gap on every binding, filed as
    /// #633; once the engine asks for itself, this goes.
    ///
    /// A linked peer is skipped: the link carries the key-package exchange
    /// within a tick, and a relay query would only publish which address
    /// this node wants (its `#p` filter) for nothing.
    fn reach_over_nostr(&self, peer: &str) {
        if !self.nostr_enabled() || self.peers.contains_key(peer) || self.has_session(peer) {
            return;
        }
        self.request_nostr_key_packages(peer);
    }

    /// Whether `peer` is announced on a peer-stream link now.
    pub(crate) fn stream_linked(&self, peer: &str) -> bool {
        self.peers.contains_key(peer)
    }

    fn has_session(&self, peer: &str) -> bool {
        self.protocol
            .mls_manager()
            .and_then(|manager| manager.read().ok()?.has_session(peer).ok())
            .unwrap_or(false)
    }

    fn preamble(&self) -> NodeResult<Vec<u8>> {
        let manager = self
            .protocol
            .mls_manager()
            .ok_or_else(|| NodeError("initialize before opening a link".to_string()))?;
        let assertion = manager
            .read()
            .map_err(|_| NodeError("MLS manager lock poisoned".to_string()))?
            .identity_assertion(&[])
            .map_err(|e| NodeError(e.to_string()))?;
        frame(&assertion).map_err(|e| NodeError(e.to_string()))
    }

    /// Records the address `link` proved. Returns a reason to refuse it.
    fn announce(&mut self, link: LinkId, address: String) -> Option<String> {
        if self.protocol.local_address() == Some(address.as_str()) {
            return Some("the link proves this device's own address".to_string());
        }
        if self.peers.contains_key(&address) {
            return Some(format!("{address} is already announced on another link"));
        }
        if let Some(state) = self.links.get_mut(&link) {
            state.peer = Some(address.clone());
        }
        self.peers.insert(address.clone(), link);
        if let Some(transport) = self.wifi_direct() {
            transport.on_peer_connected(address.clone());
        }
        self.protocol.on_neighbor_discovered(&address);
        push_event(
            &self.events,
            &Event::NeighborDiscovered {
                peer_id: address,
                transport: "WiFiDirect".to_string(),
                rssi: None,
            },
        );
        None
    }

    fn deliver(&mut self, from: String, body: Vec<u8>) {
        let Some(transport) = self.slot() else {
            return;
        };
        if let Err(error) = transport.on_data_received_from(body, from.clone()) {
            warn!(%error, "peer stream: the transport refused an inbound frame");
            return;
        }
        // Drained before the reachability notice, as the FFI crate does, so a
        // handshake confirmation in this frame lands before the engine decides
        // whether to re-arm a Welcome.
        while self.protocol.receive_message().is_some() {}
        self.protocol.on_neighbor_discovered(&from);
    }

    fn slot(&self) -> Option<Arc<dyn Transport>> {
        self.protocol
            .transport_manager()
            .get_transport(TransportType::WiFiDirect)
    }

    fn wifi_direct(&self) -> Option<WifiDirect> {
        self.slot().map(WifiDirect)
    }
}

/// The slot's concrete transport, for the two link calls the trait lacks.
struct WifiDirect(Arc<dyn Transport>);

impl WifiDirect {
    fn get(&self) -> Option<&WifiDirectTransport> {
        self.0.as_any().downcast_ref::<WifiDirectTransport>()
    }

    fn on_peer_connected(&self, peer: String) {
        if let Some(transport) = self.get() {
            transport.on_peer_connected(peer);
        }
    }

    fn on_peer_disconnected(&self, peer: &str) {
        if let Some(transport) = self.get() {
            transport.on_peer_disconnected(peer);
        }
    }
}

pub(crate) fn push_event(queue: &Mutex<VecDeque<String>>, event: &Event) {
    let Ok(json) = event.to_json() else {
        return;
    };
    let Ok(mut queue) = queue.lock() else {
        return;
    };
    if queue.len() >= MAX_QUEUED_EVENTS {
        queue.pop_front();
    }
    queue.push_back(json);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_event_queue_keeps_the_newest_up_to_its_cap() {
        let queue = Mutex::new(VecDeque::new());
        for n in 0..MAX_QUEUED_EVENTS + 5 {
            push_event(
                &queue,
                &Event::NeighborLost {
                    peer_id: format!("peer-{n}"),
                },
            );
        }
        let queue = queue.lock().expect("queue");
        assert_eq!(queue.len(), MAX_QUEUED_EVENTS);
        assert!(queue.front().expect("oldest").contains("\"peer-5\""));
        let newest = format!("\"peer-{}\"", MAX_QUEUED_EVENTS + 4);
        assert!(queue.back().expect("newest").contains(&newest));
    }
}
