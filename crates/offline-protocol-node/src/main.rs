//! Headless node entrypoint. Configuration is environment-driven; see
//! `docs/headless-node.md`.

use offline_protocol::{OfflineProtocol, ProtocolConfig};
use offline_protocol_node::{FileStorage, NodeConfig, NodeState, Waiters};
use offline_protocol_transport::{InternetConfig, InternetTransport, Transport, TransportType};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

fn main() {
    let level = match std::env::var("NODE_LOG").as_deref() {
        Ok("debug") => tracing::Level::DEBUG,
        Ok("warn") => tracing::Level::WARN,
        Ok("error") => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };
    tracing_subscriber::fmt().with_max_level(level).init();

    if let Err(e) = run() {
        eprintln!("[offline-protocol-node] fatal: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let config = NodeConfig::from_env()?;
    info!(user_id = %config.user_id, data_dir = %config.data_dir, "starting headless node");

    let mut protocol_config = ProtocolConfig::new(&config.app_id, &config.user_id);
    protocol_config.transport.ble_enabled = false;
    protocol_config.transport.wifi_direct_enabled = false;
    // The validator requires at least one enabled transport flag. Transports
    // are added explicitly below; with internet disabled the node still runs
    // (useful as a local provider / for testing) but cannot reach peers.
    protocol_config.transport.internet_enabled = true;

    let mut protocol =
        OfflineProtocol::new(protocol_config).map_err(|e| format!("protocol init: {e}"))?;

    if config.internet_enabled {
        let mut internet_config = InternetConfig::default();
        if let Some(server) = &config.internet_server {
            internet_config.server_address = server.clone();
        }
        let mut transport = InternetTransport::with_config(config.user_id.clone(), internet_config);
        transport
            .start()
            .map_err(|e| format!("internet transport start: {e}"))?;
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::Internet, Box::new(transport));
        info!(server = ?config.internet_server, "internet transport enabled");
    } else {
        warn!(
            "no transports enabled — the node will run but cannot reach peers; \
             set NODE_INTERNET_ENABLED=true and NODE_INTERNET_SERVER to join a mesh"
        );
    }

    // MLS identity + exchange state persist across restarts in the data dir.
    let storage =
        Arc::new(FileStorage::new(&config.data_dir).map_err(|e| format!("storage init: {e}"))?);
    protocol
        .initialize_mls(storage)
        .map_err(|e| format!("mls init: {e}"))?;
    protocol.start().map_err(|e| format!("start: {e}"))?;

    let waiters = Arc::new(Waiters::default());
    NodeState::install_event_routing(&mut protocol, Arc::clone(&waiters));

    let state = Arc::new(NodeState {
        protocol: Mutex::new(protocol),
        waiters,
        config: config.clone(),
    });

    // Receive/process pump.
    {
        let pump_state = Arc::clone(&state);
        std::thread::spawn(move || NodeState::pump_forever(pump_state, Duration::from_millis(100)));
    }

    let addr = format!("{}:{}", config.bind, config.port);
    let server = tiny_http::Server::http(&addr).map_err(|e| format!("bind {addr}: {e}"))?;
    offline_protocol_node::server::serve(server, state)
}
