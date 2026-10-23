//! One engine and the links that carry it.
//!
//! A browser has no Bluetooth peripheral and no Wi-Fi Direct, and what it does
//! have (a `BroadcastChannel` between tabs, a WebRTC data channel between
//! devices) delivers whole messages between two endpoints the page set up.
//! Each one is a peer stream in the sense of `docs/spec/stream-framing.md`,
//! carried in the engine's one host slot (`wifi_direct`, named for history).
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
//! - **Nothing calls back into the page.** Events and outbound frames wait in
//!   queues the page drains. The engine fires its own callbacks while it holds
//!   its state lock, and a page that answered one by calling back in would
//!   re-enter that lock on the only thread there is.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use offline_protocol::mls::InMemoryStorage;
use offline_protocol::{Event, GroupInfo, OfflineProtocol, ProtocolConfig};
use offline_protocol_transport::{
    frame, PeerStreamReader, StreamEvent, Transport, TransportStatus, TransportType,
    WifiDirectTransport,
};
use tracing::warn;

use crate::memory_store::MemoryProtocolStateStorage;

/// The page's handle for one carrier.
pub type LinkId = u32;

/// Events held for the page, at most. The engine emits continuously and a
/// page that stops polling must not grow the worker without bound; the oldest
/// go first, as in the mobile bridges' polling queue.
const MAX_QUEUED_EVENTS: usize = 1024;

/// Inbound messages one [`Node::process`] call hands to the engine. Matches
/// the mobile tickers' drain cap, so one tick's cost is bounded the same way.
const MAX_RECEIVES_PER_TICK: usize = 100;

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

type NodeResult<T> = Result<T, NodeError>;

struct Link {
    reader: PeerStreamReader,
    /// The address this link's preamble proved, once it has.
    peer: Option<String>,
}

/// One engine, its links, and the queues the page drains.
pub struct Node {
    protocol: OfflineProtocol,
    events: Arc<Mutex<VecDeque<String>>>,
    links: BTreeMap<LinkId, Link>,
    /// The one live link for each announced address.
    peers: HashMap<String, LinkId>,
    /// Frames this module produced itself (preambles), ahead of the engine's.
    outbound: VecDeque<(LinkId, Vec<u8>)>,
    next_link: LinkId,
}

impl Node {
    /// A stopped engine with no identity yet.
    ///
    /// Only the peer-stream slot is registered: a browser has no Bluetooth
    /// peripheral, and the relay and Nostr sockets are not wired yet.
    pub fn new(app_id: &str, profile: &str) -> NodeResult<Self> {
        let mut config = ProtocolConfig::new(app_id, profile);
        config.transport.ble_enabled = false;
        config.transport.internet_enabled = false;
        config.transport.reticulum_enabled = false;
        config.transport.nostr_enabled = false;
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
            links: BTreeMap::new(),
            peers: HashMap::new(),
            outbound: VecDeque::new(),
            next_link: 1,
        })
    }

    /// Mints this device's identity over in-memory stores and returns its
    /// address. Must run before [`Self::start`]: the address replaces the
    /// placeholder the transport was built on.
    pub fn initialize(&mut self) -> NodeResult<String> {
        if let Some(address) = self.protocol.local_address() {
            return Ok(address.to_string());
        }
        self.protocol.initialize_mls(
            Arc::new(InMemoryStorage::new()),
            Arc::new(MemoryProtocolStateStorage::new()),
        )?;
        let address = self
            .protocol
            .local_address()
            .ok_or_else(|| NodeError("MLS initialized without an address".to_string()))?
            .to_string();
        self.protocol.transport_manager_mut().add_transport(
            TransportType::WiFiDirect,
            Box::new(WifiDirectTransport::new(address.as_str())),
        );
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

    /// One tick: the engine's timers, then up to 100 queued inbound
    /// messages. The page calls this
    /// every 100 ms, as both mobile bridges do.
    pub fn process(&mut self) -> NodeResult<()> {
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
    /// returned id.
    pub fn open_link(&mut self) -> NodeResult<LinkId> {
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
        self.protocol.on_neighbor_lost(&peer);
        push_event(&self.events, &Event::NeighborLost { peer_id: peer });
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
    pub fn send_message(&mut self, recipient: &str, content: &str) -> NodeResult<String> {
        let id = self
            .protocol
            .send_message(recipient, content, None, None::<String>)?;
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

    /// Sends to every member of `group_id`; returns the per-member ids as a
    /// JSON array.
    pub fn send_group_message(&mut self, group_id: &str, content: &str) -> NodeResult<String> {
        let ids = self
            .protocol
            .send_group_message(group_id, content, None, None)?;
        let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
        serde_json::to_string(&ids).map_err(|e| NodeError(e.to_string()))
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

fn push_event(queue: &Mutex<VecDeque<String>>, event: &Event) {
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
