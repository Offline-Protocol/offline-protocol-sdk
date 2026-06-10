//! Control-API integration tests: a real node (MockTransport + in-memory
//! MLS storage) served over real HTTP, exercised with a real client.
//! The full two-node exchange loop is covered by the protocol crate's
//! integration tests; these verify the HTTP wiring, auth, and error shapes
//! the MCP server's `node` mode depends on.

use offline_protocol::{OfflineProtocol, ProtocolConfig};
use offline_protocol_mls::storage::InMemoryStorage;
use offline_protocol_node::{NodeConfig, NodeState, Waiters};
use offline_protocol_transport::{mock::MockTransport, Transport, TransportType};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn boot(api_token: Option<&str>) -> (String, Arc<NodeState>) {
    let mut protocol_config = ProtocolConfig::new("test-app", "node-alice");
    // The config validator requires at least one transport flag; the test
    // wires a MockTransport in as BLE below.
    protocol_config.transport.ble_enabled = true;
    protocol_config.transport.wifi_direct_enabled = false;
    protocol_config.transport.internet_enabled = false;
    let mut protocol = OfflineProtocol::new(protocol_config).unwrap();

    let mut transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol
        .initialize_mls(Arc::new(InMemoryStorage::new()))
        .unwrap();
    protocol.start().unwrap();

    let waiters = Arc::new(Waiters::default());
    NodeState::install_event_routing(&mut protocol, Arc::clone(&waiters));

    let port = free_port();
    let config = NodeConfig {
        user_id: "node-alice".into(),
        app_id: "test-app".into(),
        data_dir: "/tmp/unused".into(),
        bind: "127.0.0.1".into(),
        port,
        api_token: api_token.map(|t| t.to_string()),
        internet_enabled: false,
        internet_server: None,
    };
    let state = Arc::new(NodeState {
        protocol: Mutex::new(protocol),
        waiters,
        config,
    });

    let server = tiny_http::Server::http(format!("127.0.0.1:{port}")).unwrap();
    let serve_state = Arc::clone(&state);
    std::thread::spawn(move || offline_protocol_node::server::serve(server, serve_state));
    // Give the acceptor a beat.
    std::thread::sleep(Duration::from_millis(100));
    (format!("http://127.0.0.1:{port}"), state)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build()
}

#[test]
fn healthz_and_listings_and_balance_roundtrip() {
    let (base, _state) = boot(None);
    let client = agent();

    let health: serde_json::Value = client
        .get(&format!("{base}/healthz"))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(health["ok"], true);
    assert_eq!(health["user_id"], "node-alice");

    let listings: serde_json::Value = client
        .get(&format!("{base}/v1/listings"))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(listings, serde_json::json!([]));

    // Credit then read the mesh ledger.
    let credited: serde_json::Value = client
        .post(&format!("{base}/v1/balance/credit"))
        .send_json(serde_json::json!({ "currency": "USD", "amount_minor": 500 }))
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(credited["available_minor"], 500);
    assert_eq!(credited["held_minor"], 0);

    let balance: serde_json::Value = client
        .get(&format!("{base}/v1/balance?currency=USD"))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(balance["available_minor"], 500);

    let pending: serde_json::Value = client
        .get(&format!("{base}/v1/receipts/pending"))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(pending, serde_json::json!([]));
}

#[test]
fn bearer_auth_gates_everything_but_healthz() {
    let (base, _state) = boot(Some("sekrit"));
    let client = agent();

    // healthz stays open for liveness probes.
    assert_eq!(
        client
            .get(&format!("{base}/healthz"))
            .call()
            .unwrap()
            .status(),
        200
    );

    // Everything else requires the token.
    let denied = client.get(&format!("{base}/v1/listings")).call();
    assert_eq!(denied.unwrap_err().into_response().unwrap().status(), 401);

    let allowed = client
        .get(&format!("{base}/v1/listings"))
        .set("authorization", "Bearer sekrit")
        .call()
        .unwrap();
    assert_eq!(allowed.status(), 200);

    let wrong = client
        .get(&format!("{base}/v1/listings"))
        .set("authorization", "Bearer wrong")
        .call();
    assert_eq!(wrong.unwrap_err().into_response().unwrap().status(), 401);
}

#[test]
fn publish_and_discover_endpoints_work() {
    let (base, _state) = boot(None);
    let client = agent();

    let listing: serde_json::Value = client
        .post(&format!("{base}/v1/listings/publish"))
        .send_json(serde_json::json!({
            "service_id": "weather.v1",
            "version": "1.0",
            "capabilities": { "format": "json" },
            "kind": "service",
            "terms": { "price": { "amount_minor": 5 }, "unit": "per_call", "currency": "USD", "max_payload_kb": 64 }
        }))
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(listing["publisher"], "node-alice");
    assert_eq!(listing["kind"], "service");
    // The attestation was produced with the node's real MLS identity key.
    assert!(listing["attestation"]["signature"].as_str().unwrap().len() > 40);

    // Discovery broadcasts return a query id even with no peers around.
    let discover: serde_json::Value = client
        .post(&format!("{base}/v1/discover"))
        .send_json(serde_json::json!({}))
        .unwrap()
        .into_json()
        .unwrap();
    assert!(discover["query_id"].as_str().unwrap().len() > 10);
}

#[test]
fn invoke_and_pull_fail_cleanly_for_unknown_listings() {
    let (base, _state) = boot(None);
    let client = agent();

    let invoke = client
        .post(&format!("{base}/v1/invoke"))
        .send_json(serde_json::json!({
            "provider": "ghost",
            "service_id": "nope.v1",
            "method": "go",
            "body": "{}"
        }));
    let response = invoke.unwrap_err().into_response().unwrap();
    assert_eq!(response.status(), 422);
    let body: serde_json::Value = response.into_json().unwrap();
    assert!(body["error"].as_str().unwrap().contains("not discovered"));

    let pull = client
        .post(&format!("{base}/v1/adapters/pull"))
        .send_json(serde_json::json!({ "provider": "ghost", "service_id": "nope.adapter" }));
    assert_eq!(pull.unwrap_err().into_response().unwrap().status(), 422);
}

#[test]
fn malformed_bodies_get_400s_not_crashes() {
    let (base, _state) = boot(None);
    let client = agent();
    let response = client
        .post(&format!("{base}/v1/invoke"))
        .set("content-type", "application/json")
        .send_string("{not json")
        .unwrap_err()
        .into_response()
        .unwrap();
    assert_eq!(response.status(), 400);

    let missing = client.get(&format!("{base}/v1/nope")).call();
    assert_eq!(missing.unwrap_err().into_response().unwrap().status(), 404);
}
