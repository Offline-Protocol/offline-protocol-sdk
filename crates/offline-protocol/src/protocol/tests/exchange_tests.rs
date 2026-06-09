//! Capability exchange integration tests — the v1 definition-of-done loop,
//! exercised over the in-memory transport with two full protocol instances:
//!
//! 1. A node publishes an attested priced listing; another discovers it with
//!    price, attestation status, and reputation visible.
//! 2. A node publishes an attested adapter; another pulls it over the media
//!    path, with a hash mismatch rejecting the artifact.
//! 3. A consumer funds a prepaid balance, invokes a priced service, a signed
//!    receipt is produced, the balance debits, and settlement clears through
//!    the mock backend with the protocol fee applied.
//! 4. A priced invocation refuses to run without a confirmed MLS session.

use super::{create_test_config_for_user, *};
use offline_protocol_core::ServiceId;
use offline_protocol_exchange::{
    AttestationStatus, BillingUnit, ListingFilter, ListingKind, MockClearing, Price,
    ReputationLevel, Terms,
};
use std::collections::HashMap;

struct TestNode {
    protocol: OfflineProtocol,
    transport: MockTransport,
    user_id: String,
    events: Arc<Mutex<Vec<Event>>>,
}

impl TestNode {
    fn new(user_id: &str, auto_key_exchange: bool) -> Self {
        let mut config = create_test_config_for_user(user_id);
        config.encryption.auto_key_exchange = auto_key_exchange;
        let mut protocol = OfflineProtocol::new(config).unwrap();

        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(transport.clone()));

        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        protocol.on_event(move |event| sink.lock().unwrap().push(event));

        protocol
            .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();
        protocol.start().unwrap();

        Self {
            protocol,
            transport,
            user_id: user_id.to_string(),
            events,
        }
    }

    fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }

    fn find_event<T>(&self, f: impl Fn(&Event) -> Option<T>) -> Option<T> {
        self.events.lock().unwrap().iter().find_map(f)
    }
}

/// Shuttles in-flight messages between the two nodes until the mesh goes
/// quiet (or a round cap is hit), driving receive loops and periodic
/// processing so handshakes, ACK windows, and media transfers complete.
fn pump(a: &mut TestNode, b: &mut TestNode) {
    for _ in 0..40 {
        let mut moved = false;
        for msg in a.transport.sent_messages() {
            if msg.recipient.as_str() == b.user_id {
                b.transport.queue_message_from(msg, a.user_id.clone());
                moved = true;
            }
        }
        a.transport.clear_sent_messages();
        for msg in b.transport.sent_messages() {
            if msg.recipient.as_str() == a.user_id {
                a.transport.queue_message_from(msg, b.user_id.clone());
                moved = true;
            }
        }
        b.transport.clear_sent_messages();

        while a.protocol.receive_message().is_some() {}
        while b.protocol.receive_message().is_some() {}
        let _ = a.protocol.process();
        let _ = b.protocol.process();

        if !moved {
            break;
        }
    }
}

/// Creates two nodes, introduces them, and pumps until both sides hold a
/// confirmed MLS session.
fn paired_nodes() -> (TestNode, TestNode) {
    let mut alice = TestNode::new("alice", true);
    let mut bob = TestNode::new("bob", true);
    alice.protocol.on_neighbor_discovered("bob");
    bob.protocol.on_neighbor_discovered("alice");
    pump(&mut alice, &mut bob);
    assert_eq!(
        alice.protocol.get_establishment_state("bob").unwrap(),
        EstablishmentState::SessionConfirmed,
        "alice must have a confirmed session with bob"
    );
    assert_eq!(
        bob.protocol.get_establishment_state("alice").unwrap(),
        EstablishmentState::SessionConfirmed,
        "bob must have a confirmed session with alice"
    );
    (alice, bob)
}

fn descriptor(id: &str) -> ServiceDescriptor {
    ServiceDescriptor {
        service_id: ServiceId::new(id).unwrap(),
        version: "1.0".to_string(),
        capabilities: HashMap::new(),
    }
}

fn priced_terms(amount_minor: u64) -> Terms {
    Terms {
        price: Some(Price { amount_minor }),
        unit: BillingUnit::PerCall,
        currency: "USD".to_string(),
        max_payload_kb: 64,
    }
}

// ============================================================================
// DoD 1 — attested publish + discovery with price/attestation/reputation
// ============================================================================

#[test]
fn dod1_publish_and_discover_priced_listing() {
    let (mut alice, mut bob) = paired_nodes();

    bob.protocol
        .publish_listing(
            descriptor("weather.v1"),
            ListingKind::Service,
            priced_terms(40),
            None,
        )
        .unwrap();

    let query_id = alice.protocol.discover_listings(None).unwrap();
    pump(&mut alice, &mut bob);

    let found = alice
        .find_event(|e| match e {
            Event::ListingDiscovered {
                query_id: q,
                listing,
                attestation_status,
                reputation,
                ..
            } if *q == query_id => Some((
                listing.clone(),
                attestation_status.clone(),
                reputation.clone(),
            )),
            _ => None,
        })
        .expect("alice must receive a ListingDiscovered event");

    let (listing, attestation_status, reputation) = found;
    assert_eq!(listing.service_id(), "weather.v1");
    assert_eq!(listing.publisher, "bob");
    assert_eq!(listing.terms.unit_price_minor(), 40);
    assert_eq!(attestation_status, "verified");
    assert_eq!(reputation.level, ReputationLevel::New);

    // The listing cache supports filtered queries.
    let paid = alice.protocol.discovered_listings(&ListingFilter {
        free: Some(false),
        ..Default::default()
    });
    assert_eq!(paid.len(), 1);
    assert_eq!(paid[0].attestation_status, AttestationStatus::Verified);

    // Exchange-unaware consumers still see the plain service event.
    assert!(alice.events().iter().any(
        |e| matches!(e, Event::ServiceDiscovered { service_id, .. } if service_id == "weather.v1")
    ));
}

// ============================================================================
// DoD 3 — fund, invoke, receipt, debit, settle with fee
// ============================================================================

#[test]
fn dod3_paid_invocation_receipt_and_settlement() {
    let (mut alice, mut bob) = paired_nodes();

    bob.protocol
        .publish_listing(
            descriptor("weather.v1"),
            ListingKind::Service,
            priced_terms(40),
            None,
        )
        .unwrap();
    alice.protocol.discover_listings(None).unwrap();
    pump(&mut alice, &mut bob);

    // Fund the prepaid balance.
    let balance = alice.protocol.credit_exchange_balance("USD", 100).unwrap();
    assert_eq!(balance.available_minor, 100);

    // Invoke: hold placed for the worst case (1 unit × 40).
    let request_id = alice
        .protocol
        .invoke_listing("bob", "weather.v1", "get_forecast", r#"{"city":"NYC"}"#, 1)
        .unwrap();
    assert_eq!(alice.protocol.exchange_balance("USD").available_minor, 60);
    assert_eq!(alice.protocol.exchange_balance("USD").held_minor, 40);
    pump(&mut alice, &mut bob);

    // Provider app handles the request and responds.
    let (req_id, sender) = bob
        .find_event(|e| match e {
            Event::ServiceRequestReceived {
                request_id, sender, ..
            } => Some((request_id.clone(), sender.clone())),
            _ => None,
        })
        .expect("bob must receive the service request");
    assert_eq!(req_id, request_id);
    bob.protocol
        .respond_to_service_request(&req_id, &sender, "weather.v1", "ok", r#"{"temp":72}"#)
        .unwrap();
    pump(&mut alice, &mut bob);

    // Consumer issued a signed receipt and the hold was debited.
    let receipt = alice
        .find_event(|e| match e {
            Event::ExchangeReceiptIssued { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .expect("alice must issue a usage receipt");
    assert_eq!(receipt.request_id, request_id);
    assert_eq!(receipt.total_minor, 40);
    assert_eq!(receipt.consumer_id, "alice");
    assert_eq!(receipt.provider_id, "bob");
    assert!(!receipt.consumer_signature.is_empty());
    assert_eq!(alice.protocol.exchange_balance("USD").available_minor, 60);
    assert_eq!(alice.protocol.exchange_balance("USD").held_minor, 0);

    // Provider verified and counter-signed; consumer holds the dual signature.
    let provider_receipt = bob
        .find_event(|e| match e {
            Event::ExchangeReceiptReceived { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .expect("bob must receive and counter-sign the receipt");
    assert!(!provider_receipt.provider_signature.is_empty());
    assert!(alice
        .events()
        .iter()
        .any(|e| matches!(e, Event::ExchangeReceiptAcknowledged { .. })));

    // Settlement clears on reconnect with the protocol fee applied (2.5%).
    let clearing = MockClearing::new(250);
    let report = alice.protocol.reconcile_exchange(&clearing).unwrap();
    assert_eq!(report.settled.len(), 1);
    let report = bob.protocol.reconcile_exchange(&clearing).unwrap();
    assert_eq!(report.settled.len(), 1); // same receipt id — idempotent
    assert_eq!(clearing.collected_fees("USD"), 1); // ceil(40 × 2.5%)
    assert_eq!(clearing.account_balance("bob", "USD"), 39);

    // Settled history establishes the publisher locally.
    assert_eq!(
        alice.protocol.publisher_reputation("bob").level,
        ReputationLevel::Established
    );
}

// ============================================================================
// DoD 4 — priced invocations refuse the plaintext path
// ============================================================================

#[test]
fn dod4_priced_invocation_requires_confirmed_mls_session() {
    // No auto key exchange: discovery works over the plaintext control path,
    // but no MLS session is ever confirmed.
    let mut alice = TestNode::new("alice", false);
    let mut bob = TestNode::new("bob", false);
    alice.protocol.on_neighbor_discovered("bob");
    bob.protocol.on_neighbor_discovered("alice");
    pump(&mut alice, &mut bob);

    bob.protocol
        .publish_listing(
            descriptor("weather.v1"),
            ListingKind::Service,
            priced_terms(40),
            None,
        )
        .unwrap();
    alice.protocol.discover_listings(None).unwrap();
    pump(&mut alice, &mut bob);

    // Discovery itself succeeded over plaintext...
    assert!(!alice
        .protocol
        .discovered_listings(&ListingFilter::default())
        .is_empty());

    // ...but the priced invocation refuses to start.
    alice.protocol.credit_exchange_balance("USD", 100).unwrap();
    let err = alice
        .protocol
        .invoke_listing("bob", "weather.v1", "get", "{}", 1)
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Exchange(offline_protocol_exchange::ExchangeError::EncryptionRequired(_))
        ),
        "expected EncryptionRequired, got: {err:?}"
    );
    // Nothing was held or sent.
    assert_eq!(alice.protocol.exchange_balance("USD").available_minor, 100);
    assert_eq!(alice.protocol.exchange_balance("USD").held_minor, 0);
}

// ============================================================================
// DoD 2 — adapter publish, pull, verify; hash mismatch rejects
// ============================================================================

#[test]
fn dod2_adapter_pull_verifies_and_rejects_mismatch() {
    let (mut alice, mut bob) = paired_nodes();

    // Bob publishes an adapter backed by a real artifact file.
    let dir = std::env::temp_dir().join(format!("op-exchange-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let artifact_path = dir.join("adapter.bin");
    let artifact_bytes: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&artifact_path, &artifact_bytes).unwrap();

    bob.protocol
        .publish_adapter_listing(
            descriptor("adapter.medical"),
            Terms::free(),
            "gemma-3-1b",
            "1.0",
            artifact_path.to_str().unwrap(),
        )
        .unwrap();
    alice.protocol.discover_listings(None).unwrap();
    pump(&mut alice, &mut bob);

    let adapters = alice.protocol.discovered_listings(&ListingFilter {
        kind: Some(ListingKind::Adapter),
        ..Default::default()
    });
    assert_eq!(adapters.len(), 1);
    assert_eq!(adapters[0].attestation_status, AttestationStatus::Verified);

    // Pull: the artifact moves over the media path and verifies on arrival.
    let pull_id = alice
        .protocol
        .pull_adapter("bob", "adapter.medical")
        .unwrap();
    pump(&mut alice, &mut bob);

    let (request_id, size, data_b64) = alice
        .find_event(|e| match e {
            Event::AdapterPullCompleted {
                request_id,
                size_bytes,
                data,
                ..
            } => Some((request_id.clone(), *size_bytes, data.clone())),
            _ => None,
        })
        .expect("alice must receive the verified adapter artifact");
    assert_eq!(request_id, pull_id);
    assert_eq!(size, artifact_bytes.len() as u64);
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;
    assert_eq!(BASE64.decode(&data_b64).unwrap(), artifact_bytes);
    // The artifact never surfaces as a plain FileReceived event.
    assert!(!alice
        .events()
        .iter()
        .any(|e| matches!(e, Event::FileReceived { .. })));

    // Tamper with the artifact on disk: the next pull must be REJECTED
    // (the listing's attested hash no longer matches what bob serves).
    let mut tampered = artifact_bytes.clone();
    tampered[0] ^= 0x01;
    std::fs::write(&artifact_path, &tampered).unwrap();

    let pull2 = alice
        .protocol
        .pull_adapter("bob", "adapter.medical")
        .unwrap();
    pump(&mut alice, &mut bob);

    let (rejected_id, reason) = alice
        .find_event(|e| match e {
            Event::AdapterPullRejected {
                request_id, reason, ..
            } => Some((request_id.clone(), reason.clone())),
            _ => None,
        })
        .expect("tampered artifact must be rejected");
    assert_eq!(rejected_id, pull2);
    assert!(reason.contains("hash mismatch"), "reason: {reason}");

    std::fs::remove_dir_all(&dir).ok();
}

// ============================================================================
// Free listings keep working without balances or sessions
// ============================================================================

#[test]
fn free_listing_invocation_has_no_payment_machinery() {
    let (mut alice, mut bob) = paired_nodes();

    bob.protocol
        .publish_listing(
            descriptor("wiki.first-aid"),
            ListingKind::Service,
            Terms::free(),
            None,
        )
        .unwrap();
    alice
        .protocol
        .discover_listings(Some("wiki.first-aid"))
        .unwrap();
    pump(&mut alice, &mut bob);

    let request_id = alice
        .protocol
        .invoke_listing("bob", "wiki.first-aid", "lookup", r#"{"q":"burns"}"#, 1)
        .unwrap();
    pump(&mut alice, &mut bob);

    let (req_id, sender) = bob
        .find_event(|e| match e {
            Event::ServiceRequestReceived {
                request_id, sender, ..
            } => Some((request_id.clone(), sender.clone())),
            _ => None,
        })
        .expect("bob must receive the free request");
    bob.protocol
        .respond_to_service_request(&req_id, &sender, "wiki.first-aid", "ok", "cool water")
        .unwrap();
    pump(&mut alice, &mut bob);

    // Response arrives; no receipts, no balance changes.
    assert!(alice.events().iter().any(|e| matches!(
        e,
        Event::ServiceResponseReceived { request_id: r, status, .. }
            if *r == request_id && status == "ok"
    )));
    assert!(!alice
        .events()
        .iter()
        .any(|e| matches!(e, Event::ExchangeReceiptIssued { .. })));
    assert!(alice.protocol.pending_exchange_receipts().is_empty());
}

// ============================================================================
// Metered billing across two real nodes
// ============================================================================

#[test]
fn metered_invocation_bills_declared_units() {
    let (mut alice, mut bob) = paired_nodes();

    bob.protocol
        .publish_listing(
            descriptor("llm.summarize"),
            ListingKind::Service,
            Terms {
                price: Some(Price { amount_minor: 10 }),
                unit: BillingUnit::PerToken,
                currency: "USD".to_string(),
                max_payload_kb: 64,
            },
            None,
        )
        .unwrap();
    alice.protocol.discover_listings(None).unwrap();
    pump(&mut alice, &mut bob);

    alice.protocol.credit_exchange_balance("USD", 1000).unwrap();
    let request_id = alice
        .protocol
        .invoke_listing("bob", "llm.summarize", "summarize", "long text", 5)
        .unwrap();
    // Worst case held: 5 × 10.
    assert_eq!(alice.protocol.exchange_balance("USD").held_minor, 50);
    pump(&mut alice, &mut bob);

    let (req_id, sender) = bob
        .find_event(|e| match e {
            Event::ServiceRequestReceived {
                request_id, sender, ..
            } => Some((request_id.clone(), sender.clone())),
            _ => None,
        })
        .unwrap();
    // Provider declares actual usage before responding.
    bob.protocol.declare_invocation_usage(&req_id, 3).unwrap();
    bob.protocol
        .respond_to_service_request(&req_id, &sender, "llm.summarize", "ok", "summary")
        .unwrap();
    pump(&mut alice, &mut bob);

    let receipt = alice
        .find_event(|e| match e {
            Event::ExchangeReceiptIssued { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .expect("metered receipt must be issued");
    assert_eq!(receipt.request_id, request_id);
    assert_eq!(receipt.unit_count, 3);
    assert_eq!(receipt.total_minor, 30);
    // 30 debited, the unused 20 of the hold returned.
    assert_eq!(alice.protocol.exchange_balance("USD").available_minor, 970);
    assert_eq!(alice.protocol.exchange_balance("USD").held_minor, 0);

    // Provider accepted and counter-signed.
    assert!(bob
        .events()
        .iter()
        .any(|e| matches!(e, Event::ExchangeReceiptReceived { .. })));
}
