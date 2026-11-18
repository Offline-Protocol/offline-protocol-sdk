//! A full engine behind the peer-stream slot, for the example and its test.
//!
//! The slot's methods are the FFI crate's (`wifi_direct_*` in
//! `offline-protocol-uniffi`), which a Rust program cannot link: that crate
//! builds no Rust library. So this file does what those methods do to the
//! engine, the way the browser node (`offline-protocol-web`, `node.rs`) does
//! for its links: the link goes into the slot's transport, the engine hears
//! the peer is reachable (which pushes the key package and forms the
//! session), and a loss is reported once, only for a link the transport
//! held. One difference, on purpose: `neighbor_discovered` is emitted when a
//! stream is announced, not again on every body as the FFI does, so the
//! example's output says when a link appeared.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use offline_protocol::mls::InMemoryStorage;
use offline_protocol::{
    Event, OfflineProtocol, ProtocolConfig, ProtocolStateError, ProtocolStateResult,
    ProtocolStateStorage,
};
use offline_protocol_transport::{Transport, TransportStatus, TransportType, WifiDirectTransport};

use crate::bridge::Slot;

/// Inbound messages one tick hands the engine, as the mobile tickers do.
const MAX_RECEIVES_PER_TICK: usize = 100;

/// One engine with a fresh identity over in-memory stores, carrying the
/// peer-stream slot only.
pub struct Engine {
    protocol: OfflineProtocol,
    events: Arc<Mutex<VecDeque<Event>>>,
    address: String,
    layer_up: bool,
}

impl Engine {
    /// A started engine: a new device every time.
    pub fn new(app_id: &str, profile: &str) -> Result<Self, String> {
        let mut config = ProtocolConfig::new(app_id, profile);
        config.transport.ble_enabled = false;
        config.transport.internet_enabled = false;
        config.transport.reticulum_enabled = false;
        config.transport.nostr_enabled = false;
        config.transport.wifi_direct_enabled = true;

        let mut protocol = OfflineProtocol::new(config).map_err(|e| e.to_string())?;
        // A placeholder id until the identity exists, as the FFI crate does;
        // the transport is rebuilt on the address below.
        protocol.transport_manager_mut().add_transport(
            TransportType::WiFiDirect,
            Box::new(WifiDirectTransport::new(profile)),
        );
        protocol
            .initialize_mls(
                Arc::new(InMemoryStorage::new()),
                Arc::new(MemoryStateStorage::default()),
            )
            .map_err(|e| e.to_string())?;
        let address = protocol
            .local_address()
            .ok_or("the engine has no address after initialising")?
            .to_string();
        protocol.transport_manager_mut().add_transport(
            TransportType::WiFiDirect,
            Box::new(WifiDirectTransport::new(address.as_str())),
        );

        let events = Arc::new(Mutex::new(VecDeque::new()));
        let sink = Arc::clone(&events);
        protocol.on_event(move |event| {
            if let Ok(mut queue) = sink.lock() {
                queue.push_back(event);
            }
        });
        protocol.start().map_err(|e| e.to_string())?;
        Ok(Self {
            protocol,
            events,
            address,
            layer_up: false,
        })
    }

    /// This device's canonical address.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// This device's identity assertion, the stream preamble's body.
    pub fn identity_assertion(&self) -> Result<Vec<u8>, String> {
        let manager = self.protocol.mls_manager().ok_or("no MLS manager")?;
        let guard = manager.read().map_err(|_| "MLS manager lock poisoned")?;
        guard.identity_assertion(&[]).map_err(|e| e.to_string())
    }

    /// One tick: the engine's timers, then up to 100 queued inbound
    /// messages. The bridges call this every 100 ms.
    pub fn tick(&mut self) -> Result<(), String> {
        self.protocol.process().map_err(|e| e.to_string())?;
        for _ in 0..MAX_RECEIVES_PER_TICK {
            if self.protocol.receive_message().is_none() {
                break;
            }
        }
        Ok(())
    }

    /// Sends a direct message. Returns its id.
    pub fn send_text(&mut self, to: &str, text: &str) -> Result<String, String> {
        self.protocol
            .send_message(to, text, None, None::<String>)
            .map(|id| id.as_str())
            .map_err(|e| e.to_string())
    }

    /// Accepts a connection request, with a key package so the requester
    /// can open the session at once.
    pub fn accept_connection_request(&mut self, from: &str, name: &str) -> Result<(), String> {
        let key_package = {
            let manager = self.protocol.mls_manager().ok_or("no MLS manager")?;
            let guard = manager.read().map_err(|_| "MLS manager lock poisoned")?;
            guard
                .generate_key_package()
                .map_err(|e| e.to_string())?
                .key_package_data
        };
        self.protocol
            .accept_connection_request(from, name, Some(key_package))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// The next engine event.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.lock().ok()?.pop_front()
    }

    /// The addresses the slot's transport holds a link to now.
    #[allow(dead_code)] // Read by tests/example_engine.rs.
    pub fn linked_peers(&self) -> Vec<String> {
        self.slot()
            .map(|t| t.connected_peers().into_iter().map(|l| l.peer_id).collect())
            .unwrap_or_default()
    }

    fn slot(&self) -> Option<Arc<dyn Transport>> {
        self.protocol
            .transport_manager()
            .get_transport(TransportType::WiFiDirect)
    }

    fn with_wifi_direct<T>(&self, f: impl FnOnce(&WifiDirectTransport) -> T) -> Option<T> {
        let slot = self.slot()?;
        slot.as_any().downcast_ref::<WifiDirectTransport>().map(f)
    }

    fn emit(&self, event: Event) {
        if let Ok(mut queue) = self.events.lock() {
            queue.push_back(event);
        }
    }

    fn lost(&mut self, peer: String) {
        self.protocol.on_neighbor_lost(&peer);
        self.emit(Event::NeighborLost { peer_id: peer });
    }
}

impl Slot for Engine {
    fn wifi_direct_status_changed(&mut self, is_connected: bool) {
        let Some(slot) = self.slot() else {
            return;
        };
        let before = slot.connected_peers();
        slot.on_status_changed(if is_connected {
            TransportStatus::Available
        } else {
            TransportStatus::Disconnected
        });
        self.layer_up = is_connected;
        if is_connected {
            // A link proved while the layer was down is revealed now, and
            // its key package push is retried.
            for link in slot.connected_peers() {
                if !before.iter().any(|b| b.peer_id == link.peer_id) {
                    self.protocol.on_neighbor_discovered(&link.peer_id);
                }
            }
        } else {
            // Links the layer's end cleared are lost here, once.
            for link in before {
                self.lost(link.peer_id);
            }
        }
    }

    fn wifi_direct_peer_connected(&mut self, peer_id: &str) {
        self.with_wifi_direct(|t| t.on_peer_connected(peer_id.to_string()));
        self.protocol.on_neighbor_discovered(peer_id);
        self.emit(Event::NeighborDiscovered {
            peer_id: peer_id.to_string(),
            transport: "WiFiDirect".to_string(),
            rssi: None,
        });
    }

    fn wifi_direct_message_received(&mut self, sender_id: &str, data: Vec<u8>) {
        let Some(slot) = self.slot() else {
            return;
        };
        if let Err(error) = slot.on_data_received_from(data, sender_id.to_string()) {
            eprintln!("peer stream: the transport refused a frame from {sender_id}: {error}");
            return;
        }
        // Drained before the reachability notice, as the FFI does, so a
        // handshake confirmation in this frame lands before the engine
        // decides whether to re-arm a Welcome.
        while self.protocol.receive_message().is_some() {}
        self.protocol.on_neighbor_discovered(sender_id);
    }

    fn wifi_direct_get_next_message(&mut self) -> Option<(String, Vec<u8>)> {
        if !self.layer_up {
            return None;
        }
        let slot = self.slot()?;
        loop {
            match slot.get_next_message() {
                Ok(next) => return next,
                Err(error) => eprintln!("peer stream: an outbound message failed: {error}"),
            }
        }
    }

    fn wifi_direct_peer_disconnected(&mut self, peer_id: &str) {
        let held = self
            .with_wifi_direct(|t| t.on_peer_disconnected(peer_id))
            .unwrap_or(false);
        // Reported once, and only for a link the transport held.
        if held {
            self.lost(peer_id.to_string());
        }
    }
}

type Records = BTreeMap<(String, String), Vec<u8>>;

/// Protocol state in a map, keyed by `(key_type, key_id)` as a pair so no
/// separator can make two categories collide. Gone when the process ends.
#[derive(Debug, Default)]
pub struct MemoryStateStorage {
    records: Mutex<Records>,
}

impl MemoryStateStorage {
    fn records(&self) -> ProtocolStateResult<std::sync::MutexGuard<'_, Records>> {
        self.records
            .lock()
            .map_err(|_| ProtocolStateError::LoadFailed("memory store lock poisoned".to_string()))
    }
}

impl ProtocolStateStorage for MemoryStateStorage {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
        self.records()?
            .insert((key_type.to_string(), key_id.to_string()), data.to_vec());
        Ok(())
    }

    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
        Ok(self
            .records()?
            .get(&(key_type.to_string(), key_id.to_string()))
            .cloned())
    }

    fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
        self.records()?
            .remove(&(key_type.to_string(), key_id.to_string()));
        Ok(())
    }

    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
        let keys: BTreeSet<String> = self
            .records()?
            .keys()
            .filter(|(kind, _)| kind == key_type)
            .map(|(_, id)| id.clone())
            .collect();
        Ok(keys.into_iter().collect())
    }
}
