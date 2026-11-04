//! A phone provisioning its paired leaf with application membership, and
//! the two then proving it to each other over the membership exchange
//! (ADR 0037, "Leaves"; step 9a of the shared network plan).
//!
//! Both ends are real: the engine with its send and receive paths, and a
//! [`LeafDevice`] handed frames the way firmware hands them. The harness is
//! the radio. What is pinned is what each end concludes, what the device
//! acknowledges, and what it refuses: provisioning outside the pair's
//! session, and provisioning a store could not keep.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use ed25519_dalek::{Signer, SigningKey};
use offline_protocol_core::{Address, Message};
use offline_protocol_leaf::store::KEY_TYPE_MEMBERSHIP;
use offline_protocol_leaf::{Handled, LeafDevice, LeafError, LeafEvent, LeafStore, MemoryStore};
use offline_protocol_sealed::membership::{MemberCert, MemberProvision, WIRE_APP_ID};
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::constants::ACK_FOR_KEY;
use crate::mls::InMemoryStorage;
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::tests::{create_test_config_for_user, id};
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};

/// The application both ends run. `K_app` is derived under it, so the leaf
/// is opened with the same string the engine is configured with.
const APP: &str = "com.example.lock";

fn publisher() -> SigningKey {
    SigningKey::from_bytes(&[42; 32])
}

fn unix_now() -> u64 {
    u64::try_from(Utc::now().timestamp()).expect("after 1970")
}

/// A certificate the publisher signs over `address`, current for a day.
fn certificate(address: &str) -> MemberCert {
    let address: Address = address.parse().expect("an address");
    let not_after = unix_now() + 86_400;
    let signature = publisher()
        .sign(&MemberCert::signing_payload(&address, not_after))
        .to_bytes();
    MemberCert {
        address,
        not_after,
        signature,
    }
}

/// A store that can be told to refuse writes of the membership record, the
/// way a flash part refuses a write it cannot complete.
struct FlakyStore {
    inner: MemoryStore,
    fail_membership: AtomicBool,
    membership_writes: Mutex<usize>,
}

impl LeafStore for FlakyStore {
    fn store(
        &self,
        key_type: &str,
        key_id: &str,
        data: &[u8],
    ) -> Result<(), offline_protocol_leaf::StoreError> {
        if key_type == KEY_TYPE_MEMBERSHIP {
            if self.fail_membership.load(Ordering::SeqCst) {
                return Err(offline_protocol_leaf::StoreError::Store(
                    "flash write failed".to_string(),
                ));
            }
            *self.membership_writes.lock().unwrap() += 1;
        }
        self.inner.store(key_type, key_id, data)
    }

    fn load(
        &self,
        key_type: &str,
        key_id: &str,
    ) -> Result<Option<Vec<u8>>, offline_protocol_leaf::StoreError> {
        self.inner.load(key_type, key_id)
    }

    fn delete(
        &self,
        key_type: &str,
        key_id: &str,
    ) -> Result<(), offline_protocol_leaf::StoreError> {
        self.inner.delete(key_type, key_id)
    }
}

struct Phone {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
}

impl Phone {
    /// A phone of the application: configured with the publisher key and
    /// holding a certificate of its own.
    fn member(label: &str) -> Self {
        let mut config = create_test_config_for_user(label);
        config.app_id = APP.to_string();
        config.encryption.enabled = true;
        config.encryption.auto_key_exchange = true;
        config.app_publisher_key = Some(publisher().verifying_key().to_bytes());
        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        protocol
            .initialize_mls(
                crate::test_identity::seeded_storage(label),
                Arc::new(TestProtocolStateStorage {
                    storage: Arc::new(InMemoryStorage::new()),
                }),
            )
            .expect("initialize_mls");
        let mock = MockTransport::new(TransportType::BLE);
        mock.start().expect("transport start");
        let transport = mock.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        protocol.start().expect("start");
        let address = id(label);
        protocol
            .set_app_membership(&certificate(&address).to_bytes())
            .expect("the phone's own certificate");
        Self {
            protocol,
            transport,
            address,
        }
    }

    /// Everything the phone put on any link since the last take: ordinary
    /// sends and the membership frames handed straight to a neighbour.
    fn take_sent(&mut self) -> Vec<Message> {
        let mut out = self.transport.sent_messages();
        self.transport.clear_sent_messages();
        out.extend(self.transport.peer_sends().into_iter().map(|(_, m)| m));
        self.transport.clear_peer_sends();
        out
    }
}

struct Leaf {
    device: LeafDevice,
    store: Arc<FlakyStore>,
    address: String,
    events: Vec<LeafEvent>,
}

impl Leaf {
    fn new() -> Self {
        let store = Arc::new(FlakyStore {
            inner: MemoryStore::new(),
            fail_membership: AtomicBool::new(false),
            membership_writes: Mutex::new(0),
        });
        let shared: Arc<dyn LeafStore> = store.clone();
        let device = LeafDevice::provision(shared, APP).expect("provision");
        let address = device.address().to_string();
        Self {
            device,
            store,
            address,
            events: Vec::new(),
        }
    }

    fn provisioned_by(&self, peer: &str) -> bool {
        self.events.iter().any(
            |event| matches!(event, LeafEvent::MembershipProvisioned { peer: p, .. } if p == peer),
        )
    }
}

/// Phones and a leaf in range of each other, with the harness as the radio.
struct Radio {
    phones: Vec<Phone>,
    leaf: Leaf,
    now: u64,
    leaf_outbound: Vec<Message>,
    /// Every frame the leaf handed back, kept for assertions.
    leaf_sent: Vec<Message>,
}

impl Radio {
    fn new(labels: &[&str]) -> Self {
        Self {
            phones: labels.iter().map(|label| Phone::member(label)).collect(),
            leaf: Leaf::new(),
            now: unix_now(),
            leaf_outbound: Vec::new(),
            leaf_sent: Vec::new(),
        }
    }

    fn phone(&mut self, index: usize) -> &mut Phone {
        &mut self.phones[index]
    }

    /// Puts the leaf on phone `index`'s link, as a carrier would report it.
    fn link(&mut self, index: usize) {
        let leaf = self.leaf.address.clone();
        let phone = &mut self.phones[index];
        phone.transport.add_connected_peer(leaf.clone(), -50);
        phone.protocol.neighbor_reachable(&leaf, "BLE");
    }

    fn feed_leaf(&mut self, message: &Message) -> Result<(), LeafError> {
        let Handled { outbound, events } = self.leaf.device.handle(message, self.now)?;
        self.leaf.events.extend(events);
        self.leaf_sent.extend(outbound.iter().cloned());
        self.leaf_outbound.extend(outbound);
        Ok(())
    }

    /// Carries every frame both ways until nothing moves.
    fn settle(&mut self) {
        for _ in 0..16 {
            let mut moved = 0;
            for index in 0..self.phones.len() {
                for message in self.phones[index].take_sent() {
                    if message.recipient.as_str() == self.leaf.address {
                        moved += 1;
                        self.feed_leaf(&message)
                            .expect("the device refused a frame this test did not expect");
                    }
                }
            }
            let outbound = std::mem::take(&mut self.leaf_outbound);
            for message in outbound {
                moved += 1;
                let leaf = self.leaf.address.clone();
                if let Some(phone) = self
                    .phones
                    .iter_mut()
                    .find(|p| p.address == message.recipient.as_str())
                {
                    phone.transport.queue_message_from(message, leaf);
                    while phone.protocol.receive_message().is_some() {}
                }
            }
            if moved == 0 {
                return;
            }
        }
        panic!("the radio never went quiet");
    }

    /// Pairs phone `index` with the leaf from the device's key package.
    fn pair(&mut self, index: usize) {
        let phone = self.phones[index].address.clone();
        let frame = self
            .leaf
            .device
            .key_package_frame(&phone, self.now)
            .expect("mint");
        self.leaf_outbound.push(frame);
        self.settle();
        assert!(
            self.leaf.device.has_session(&phone).unwrap(),
            "the pairing this test rests on did not complete"
        );
    }

    fn leaf_cert(&self) -> Vec<u8> {
        certificate(&self.leaf.address).to_bytes().to_vec()
    }
}

/// The scenario the plan names: a phone pairs a leaf, provisions it, and
/// the two then see each other as members. The provisioning is
/// acknowledged, and from then on the device names no application on the
/// wire.
#[test]
fn a_provisioned_leaf_and_its_configured_phone_see_each_other() {
    let mut radio = Radio::new(&["alice"]);
    let phone = radio.phones[0].address.clone();
    let leaf = radio.leaf.address.clone();
    radio.link(0);
    radio.pair(0);
    assert!(
        !radio.phones[0].protocol.is_member_visible(&leaf),
        "an unprovisioned leaf is no member"
    );

    let cert = radio.leaf_cert();
    radio.leaf_sent.clear();
    let id = radio
        .phone(0)
        .protocol
        .provision_leaf_membership(&leaf, &cert)
        .expect("provision");
    radio.settle();

    assert!(radio.leaf.provisioned_by(&phone), "{:?}", radio.leaf.events);
    assert!(
        radio.leaf_sent.iter().any(|m| m
            .metadata
            .get(ACK_FOR_KEY)
            .is_some_and(|acked| *acked == id.to_string())),
        "the device never acknowledged the provisioning"
    );
    assert!(
        radio.leaf.device.is_member_visible(&phone, radio.now),
        "the leaf does not see its phone: {:?}",
        radio.leaf.events
    );
    assert!(
        radio.phones[0].protocol.is_member_visible(&leaf),
        "the phone does not see its leaf"
    );
    assert!(radio
        .leaf
        .events
        .iter()
        .any(|e| matches!(e, LeafEvent::MemberVisible { peer, .. } if *peer == phone)));
    assert!(
        radio.leaf.device.membership_not_after(radio.now).is_some(),
        "the leaf reports no certificate of its own"
    );
    let stamped: Vec<_> = radio
        .leaf_sent
        .iter()
        .map(|m| m.app_id.as_str().to_string())
        .collect();
    assert!(
        stamped.iter().all(|app| app == WIRE_APP_ID),
        "a provisioned leaf named an application on the wire: {stamped:?}"
    );
}

/// A phone of the same application that never paired with the device sees
/// it, and is seen by it. That phone signs its frames to a stranger under
/// the older control payload, which the device admits for the exchange.
/// Across a power cut the provisioning survives, so the device runs the
/// exchange from its first frame.
#[test]
fn a_member_that_never_paired_sees_a_provisioned_leaf_across_a_power_cut() {
    let mut radio = Radio::new(&["alice", "bob"]);
    let bob = radio.phones[1].address.clone();
    let leaf = radio.leaf.address.clone();
    radio.link(0);
    radio.pair(0);
    let cert = radio.leaf_cert();
    radio
        .phone(0)
        .protocol
        .provision_leaf_membership(&leaf, &cert)
        .expect("provision");
    radio.settle();

    let shared: Arc<dyn LeafStore> = radio.leaf.store.clone();
    radio.leaf.device = LeafDevice::open(shared, APP).expect("reopen");
    assert!(radio.leaf.device.membership_not_after(radio.now).is_some());

    radio.link(1);
    let hello = radio
        .leaf
        .device
        .neighbor_discovered(&bob, radio.now)
        .expect("hello");
    radio.leaf_outbound.extend(hello.outbound);
    radio.settle();

    assert!(radio.phones[1].protocol.is_member_visible(&leaf));
    assert!(radio.leaf.device.is_member_visible(&bob, radio.now));
}

/// Provisioning outside the pair's session is not provisioning. The prefix
/// sent as a bare frame, even one the phone signed, provisions nothing, and
/// the application cannot send it as text through the session either.
#[test]
fn a_provisioning_frame_outside_the_session_is_refused() {
    let mut radio = Radio::new(&["alice"]);
    let leaf = radio.leaf.address.clone();
    radio.link(0);
    radio.pair(0);

    let body = MemberProvision {
        publisher_key: publisher().verifying_key().to_bytes(),
        cert: certificate(&leaf),
        supplied_time: radio.now,
    }
    .to_body();
    let content = format!("{}{}", internal_prefixes::MEMBER_PROVISION, body);

    let refused =
        radio
            .phone(0)
            .protocol
            .send_message(&leaf, content.clone(), None, None::<String>);
    assert!(
        refused.is_err(),
        "the application sent the reserved provisioning prefix as text"
    );

    let mut bare = radio
        .phone(0)
        .protocol
        .create_message(&leaf, content, None, None)
        .expect("a frame");
    radio
        .phone(0)
        .protocol
        .sign_control_message(&mut bare)
        .expect("signed");
    radio.feed_leaf(&bare).expect("ignored, not an error");

    assert!(radio.leaf.device.membership_not_after(radio.now).is_none());
    assert!(!radio.leaf.provisioned_by(&radio.phones[0].address.clone()));
    assert_eq!(*radio.leaf.store.membership_writes.lock().unwrap(), 0);
    assert!(radio.leaf_outbound.is_empty(), "a bare frame was answered");
}

/// Durable before acknowledged: a store that cannot keep the provisioning
/// leaves the device unprovisioned, unacknowledged and invisible, and the
/// same frame retransmitted provisions it once the store recovers.
#[test]
fn a_provisioning_the_store_cannot_keep_is_never_acknowledged() {
    let mut radio = Radio::new(&["alice"]);
    let phone = radio.phones[0].address.clone();
    let leaf = radio.leaf.address.clone();
    radio.link(0);
    radio.pair(0);

    radio
        .leaf
        .store
        .fail_membership
        .store(true, Ordering::SeqCst);
    let cert = radio.leaf_cert();
    let id = radio
        .phone(0)
        .protocol
        .provision_leaf_membership(&leaf, &cert)
        .expect("provision");
    let sent = radio.phone(0).take_sent();
    let frame = sent
        .iter()
        .find(|m| m.id == id)
        .expect("the provisioning frame")
        .clone();

    let refused = radio.feed_leaf(&frame);
    assert!(matches!(refused, Err(LeafError::Storage(_))), "{refused:?}");
    assert!(radio.leaf_outbound.is_empty(), "something was acknowledged");
    assert!(radio.leaf.device.membership_not_after(radio.now).is_none());
    // The phone's fresh hello, carried now: the device still answers
    // nothing, and sees nobody.
    for message in sent.into_iter().filter(|m| m.id != id) {
        radio
            .feed_leaf(&message)
            .expect("a hello is never an error");
    }
    assert!(radio.leaf_outbound.is_empty());
    assert!(!radio.leaf.device.is_member_visible(&phone, radio.now));
    assert!(!radio.phones[0].protocol.is_member_visible(&leaf));

    // The retransmission is the same frame, and it still opens: the refused
    // attempt spent no generation.
    radio
        .leaf
        .store
        .fail_membership
        .store(false, Ordering::SeqCst);
    radio.feed_leaf(&frame).expect("the retransmission");
    assert!(radio.leaf.provisioned_by(&phone));
    assert!(radio.leaf_outbound.iter().any(|m| m
        .metadata
        .get(ACK_FOR_KEY)
        .is_some_and(|acked| *acked == id.to_string())));
    radio.settle();
    assert!(radio.leaf.device.is_member_visible(&phone, radio.now));
    assert!(radio.phones[0].protocol.is_member_visible(&leaf));
}

/// A leaf nobody provisioned behaves as it did before provisioning existed:
/// a membership hello is consumed and answered by nothing, nothing is
/// written, discovery starts nothing, and its frames carry its own app id.
#[test]
fn an_unprovisioned_leaf_is_unchanged() {
    let mut radio = Radio::new(&["alice"]);
    let phone = radio.phones[0].address.clone();
    radio.link(0);

    let hellos: Vec<Message> = radio
        .phone(0)
        .take_sent()
        .into_iter()
        .filter(|m| m.content.starts_with(internal_prefixes::MEMBER))
        .collect();
    assert_eq!(hellos.len(), 1, "the phone sends one hello to the leaf");
    let entries = radio.leaf.store.inner.len();
    radio.feed_leaf(&hellos[0]).expect("consumed");
    assert!(
        radio.leaf_outbound.is_empty(),
        "an unprovisioned leaf answered"
    );
    assert_eq!(
        radio.leaf.store.inner.len(),
        entries,
        "a hello wrote to flash"
    );
    assert!(matches!(
        radio.leaf.events.last(),
        Some(LeafEvent::Ignored { .. })
    ));
    let discovered = radio
        .leaf
        .device
        .neighbor_discovered(&phone, radio.now)
        .expect("discovery");
    assert!(discovered.outbound.is_empty() && discovered.events.is_empty());

    radio.pair(0);
    let stamped: HashMap<String, usize> =
        radio.leaf_sent.iter().fold(HashMap::new(), |mut acc, m| {
            *acc.entry(m.app_id.as_str().to_string()).or_default() += 1;
            acc
        });
    assert_eq!(
        stamped.keys().collect::<Vec<_>>(),
        vec![APP],
        "an unprovisioned leaf stamps its own app id"
    );
    assert!(!radio.leaf.device.is_member_visible(&phone, radio.now));
}
