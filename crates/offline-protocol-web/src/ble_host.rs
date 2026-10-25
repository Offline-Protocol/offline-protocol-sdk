//! The Bluetooth LE slot, driven from the page over Web Bluetooth (W14).
//!
//! A page can be a GATT central and nothing else: it cannot advertise, so
//! nothing ever discovers it, and it serves no GATT, so a peripheral can never
//! dial it back. What it can do, in Chromium, is connect to a phone or a board
//! running this SDK as a peripheral once its person picks the device in the
//! browser's chooser. The page owns that connection and moves bytes; this
//! module owns what they mean, through the `BleTransport` every binding
//! shares, so the fragment codec is the one the conformance vectors check.
//!
//! # Invariants
//!
//! - **A peer is announced only under the address its identity proves.** The
//!   page hands over the Device id and Identity values it read;
//!   [`Node::ble_verify`] runs the one verifier, compares the derived address
//!   with the Device id exactly, and remembers it. [`Node::ble_link_up`]
//!   announces only an address that verification produced, so a page cannot
//!   name a peer the engine did not check.
//! - **The per-peer MTU reaches the transport before the peer does,** as in
//!   every binding: a fragmenting send between the two would fall back to the
//!   floor. A value under the 185-byte floor is the transport's to refuse.
//! - **A link is lost once.** A peer linked over a peer stream as well as
//!   Bluetooth is lost only when the last of them goes, as the FFI's
//!   `ble_peer_lost` does.
//! - **The central says hello.** [`Node::ble_hello`] is this device's
//!   identity assertion, which the page writes to the peripheral's Hello
//!   characteristic: without it a peripheral has no way to name a central it
//!   cannot dial back, and nothing the page writes is attributed.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use offline_protocol::Event;
use offline_protocol_core::time::SystemTime;
use offline_protocol_transport::ble::PeerDevice;
use offline_protocol_transport::{BleTransport, Transport, TransportType};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::node::{push_event, Node, NodeError, NodeResult, MAX_LINKS};

/// Addresses verified and waiting to be linked, at most. A page verifies a
/// device and links it moments later; this bounds a page that only verifies.
const MAX_VERIFIED: usize = 16;

/// The App tag domain separator, the iOS and Android copies' own.
const APP_TAG_DOMAIN: &str = "offline-protocol-ble-app-tag-v1";

/// Bytes in an App tag.
pub const APP_TAG_BYTES: usize = 8;

/// This application's App tag: `SHA-256(domain ‖ 0x00 ‖ app_id)[0..8]`, the
/// value a central compares when a peripheral presents several instances of
/// the service, one per SDK application on the phone.
pub fn app_tag(app_id: &str) -> [u8; APP_TAG_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(APP_TAG_DOMAIN.as_bytes());
    hasher.update([0u8]);
    hasher.update(app_id.as_bytes());
    let digest = hasher.finalize();
    let mut tag = [0u8; APP_TAG_BYTES];
    tag.copy_from_slice(&digest[..APP_TAG_BYTES]);
    tag
}

/// The worker's side of the Bluetooth slot.
#[derive(Debug, Default)]
pub(crate) struct BleHost {
    /// Addresses `ble_verify` proved, not linked yet.
    verified: VecDeque<String>,
    /// Addresses linked over Bluetooth now.
    pub(crate) linked: BTreeSet<String>,
}

impl Node {
    /// This device's identity assertion, for the page to write to a
    /// peripheral's Hello characteristic.
    pub fn ble_hello(&self) -> NodeResult<Vec<u8>> {
        let manager = self
            .protocol
            .mls_manager()
            .ok_or_else(|| NodeError("initialize before linking over Bluetooth".to_string()))?;
        let assertion = manager
            .read()
            .map_err(|_| NodeError("MLS manager lock poisoned".to_string()))?
            .identity_assertion(&[])
            .map_err(|e| NodeError(e.to_string()))?;
        Ok(assertion)
    }

    /// Checks what a peripheral served: its Device id value and its Identity
    /// value. Returns the address the identity proves, which must equal the
    /// Device id exactly, and remembers it for [`Self::ble_link_up`].
    pub fn ble_verify(&mut self, device_id: &[u8], identity: &[u8]) -> NodeResult<String> {
        self.ble_host_mut()?;
        let claimed = std::str::from_utf8(device_id)
            .map_err(|_| NodeError("the peripheral's Device id is not UTF-8".to_string()))?;
        let derived = offline_protocol_mls::verify_identity_assertion(identity)
            .map_err(|e| NodeError(format!("the peripheral's identity did not verify: {e}")))?
            .to_string();
        if derived != claimed {
            return Err(NodeError(format!(
                "the peripheral serves Device id {claimed} but proves {derived}"
            )));
        }
        if self.protocol.local_address() == Some(derived.as_str()) {
            return Err(NodeError(
                "the peripheral proves this device's own address".to_string(),
            ));
        }
        let host = self.ble_host_mut()?;
        host.verified.retain(|held| held != &derived);
        if host.verified.len() >= MAX_VERIFIED {
            host.verified.pop_front();
        }
        host.verified.push_back(derived.clone());
        Ok(derived)
    }

    /// The link to `peer`, which [`Self::ble_verify`] proved, is subscribed
    /// and has said hello: tells the transport its write payload, then the
    /// engine that the peer is here.
    pub fn ble_link_up(&mut self, peer: &str, max_payload: u32) -> NodeResult<()> {
        let host = self.ble_host_mut()?;
        let Some(at) = host.verified.iter().position(|held| held == peer) else {
            return Err(NodeError(format!(
                "{peer} was not verified on this link; verify its identity first"
            )));
        };
        if !host.linked.contains(peer) && host.linked.len() >= MAX_LINKS {
            return Err(NodeError(format!(
                "{MAX_LINKS} Bluetooth links are already open"
            )));
        }
        host.verified.remove(at);
        host.linked.insert(peer.to_string());
        let transport = self.ble_slot()?;
        // ORDERING: the payload before the peer, as every binding does.
        transport.set_peer_mtu(peer, max_payload as usize);
        if let Some(ble) = transport.as_any().downcast_ref::<BleTransport>() {
            ble.on_peer_discovered(PeerDevice {
                device_id: peer.to_string(),
                address: String::new(),
                rssi: 0,
                last_seen: SystemTime::now(),
                connected: true,
            });
        }
        let blocked = self.protocol.is_user_blocked(peer);
        self.protocol.on_neighbor_discovered(peer);
        if !blocked {
            push_event(
                &self.events,
                &Event::NeighborDiscovered {
                    peer_id: peer.to_string(),
                    transport: "BLE".to_string(),
                    rssi: None,
                },
            );
        }
        Ok(())
    }

    /// A notification or indication the peripheral sent on `peer`'s link:
    /// one fragment, reassembled in the transport.
    pub fn ble_fragment_received(&mut self, peer: &str, fragment: &[u8]) -> NodeResult<()> {
        if !self.ble_host_mut()?.linked.contains(peer) {
            return Err(NodeError(format!("{peer} has no Bluetooth link")));
        }
        let transport = self.ble_slot()?;
        transport
            .on_fragment_received_from(fragment.to_vec(), peer.to_string())
            .map_err(|e| NodeError(e.to_string()))?;
        while self.protocol.receive_message().is_some() {}
        self.protocol.on_neighbor_discovered(peer);
        Ok(())
    }

    /// The next fragment to write, as `(peer, bytes)`. A fragment for a peer
    /// whose link went down after it was queued is dropped.
    pub fn ble_next_fragment(&mut self) -> Option<(String, Vec<u8>)> {
        let transport = self.ble_slot().ok()?;
        loop {
            let (peer, fragment) = match transport.get_next_fragment() {
                Ok(Some(next)) => next,
                Ok(None) => return None,
                Err(error) => {
                    warn!(%error, "bluetooth: could not take an outbound fragment");
                    return None;
                }
            };
            if self.ble_host.as_ref()?.linked.contains(&peer) {
                return Some((peer, fragment));
            }
            warn!(%peer, "bluetooth: dropping a fragment for a link that closed");
        }
    }

    /// The link to `peer` is gone. The engine hears of the loss only when no
    /// peer stream still links it.
    pub fn ble_link_down(&mut self, peer: &str) {
        let Some(host) = self.ble_host.as_mut() else {
            return;
        };
        if !host.linked.remove(peer) {
            return;
        }
        if let Ok(transport) = self.ble_slot() {
            if let Some(ble) = transport.as_any().downcast_ref::<BleTransport>() {
                ble.on_peer_lost(peer);
            }
        }
        if self.stream_linked(peer) {
            return;
        }
        self.protocol.on_neighbor_lost(peer);
        push_event(
            &self.events,
            &Event::NeighborLost {
                peer_id: peer.to_string(),
            },
        );
    }

    /// Whether `peer` is linked over Bluetooth now.
    pub(crate) fn ble_linked(&self, peer: &str) -> bool {
        self.ble_host
            .as_ref()
            .is_some_and(|host| host.linked.contains(peer))
    }

    fn ble_host_mut(&mut self) -> NodeResult<&mut BleHost> {
        self.ble_host
            .as_mut()
            .ok_or_else(|| NodeError("Bluetooth is not enabled on this node".to_string()))
    }

    fn ble_slot(&self) -> NodeResult<Arc<dyn Transport>> {
        self.protocol
            .transport_manager()
            .get_transport(TransportType::BLE)
            .ok_or_else(|| NodeError("Bluetooth is not enabled on this node".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The vectors iOS and Android pin (`BleAppTagTests.swift`,
    /// `BleAppTagTest.kt`): only shared vectors prove the copies agree.
    #[test]
    fn the_app_tag_matches_the_native_vectors() {
        assert_eq!(hex(&app_tag("offline-messenger")), "2e2e6107567ff67b");
        assert_eq!(hex(&app_tag("offline-demo")), "3a88052657e598e1");
        assert_eq!(hex(&app_tag("")), "d5c9ed34fbf6503f");
    }
}
