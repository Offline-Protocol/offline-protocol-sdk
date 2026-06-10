//! # Offline Protocol headless node
//!
//! A daemon that runs a full [`OfflineProtocol`](offline_protocol::OfflineProtocol)
//! mesh node — MLS identity, capability exchange, transports — and exposes a
//! **localhost HTTP control API** mirroring the Capability Exchange
//! `MeshBridge` interface. This is how off-device agents (the MCP server's
//! `node` mode) and scripts participate in the mesh from machines that are
//! not phones: servers, Raspberry Pis, laptops.
//!
//! The control API is a trusted local surface (it can spend the node's
//! prepaid mesh balance): it binds to `127.0.0.1` by default and supports a
//! bearer token. Spending guardrails for agents live in the MCP server, not
//! here — this daemon is the mesh participant, the MCP server is the wallet
//! gate in front of it.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod config;
pub mod server;
pub mod state;
pub mod storage;

pub use config::NodeConfig;
pub use state::{NodeEvent, NodeState, Waiters};
pub use storage::FileStorage;
