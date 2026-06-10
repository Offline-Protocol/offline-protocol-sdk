//! Node runtime: the protocol instance, the receive/process pump, and the
//! event-waiter registry that turns the protocol's async events into
//! synchronous HTTP responses.
//!
//! Locking discipline: HTTP handlers take the protocol mutex only for the
//! duration of a protocol call, never while waiting on events. The event
//! callback runs inside protocol calls (under the mutex), so it must only
//! touch the waiter registry — never the protocol itself.

use crate::config::NodeConfig;
use offline_protocol::{Error as ProtocolError, Event, OfflineProtocol};
use offline_protocol_exchange::UsageReceipt;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Events routed from the protocol callback to a waiting HTTP handler,
/// keyed by request id.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// `ServiceResponseReceived` for the awaited request.
    Response {
        /// Application-defined status (`ok`, `error`, `not_found`, ...).
        status: String,
        /// Response body.
        body: String,
    },
    /// The consumer-side signed receipt was issued for the awaited request.
    ReceiptIssued(Box<UsageReceipt>),
    /// The tracked invocation failed (timeout/error); hold released.
    InvocationFailed(String),
    /// A verified adapter artifact arrived for the awaited pull.
    PullCompleted {
        /// Verified SHA-256 content hash (lowercase hex).
        content_hash: String,
        /// Artifact size in bytes.
        size_bytes: u64,
        /// Base64-encoded verified artifact bytes.
        data_base64: String,
    },
    /// The adapter artifact failed verification and was discarded.
    PullRejected(String),
}

/// Registry of in-flight HTTP requests waiting on protocol events.
#[derive(Default)]
pub struct Waiters {
    by_request: Mutex<HashMap<String, Sender<NodeEvent>>>,
}

impl Waiters {
    /// Registers a waiter channel for a request id.
    pub fn register(&self, request_id: &str) -> Receiver<NodeEvent> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut map) = self.by_request.lock() {
            map.insert(request_id.to_string(), tx);
        }
        rx
    }

    /// Removes the waiter for a request id.
    pub fn remove(&self, request_id: &str) {
        if let Ok(mut map) = self.by_request.lock() {
            map.remove(request_id);
        }
    }

    fn notify(&self, request_id: &str, event: NodeEvent) {
        let sender = match self.by_request.lock() {
            Ok(map) => map.get(request_id).cloned(),
            Err(_) => None,
        };
        if let Some(sender) = sender {
            // A dropped receiver (handler timed out) is fine to ignore.
            let _ = sender.send(event);
        }
    }
}

/// Shared node state handed to the HTTP server.
pub struct NodeState {
    /// The protocol instance. See the module docs for locking discipline.
    pub protocol: Mutex<OfflineProtocol>,
    /// In-flight request waiters.
    pub waiters: Arc<Waiters>,
    /// Resolved configuration.
    pub config: NodeConfig,
}

impl NodeState {
    /// Wires the event callback that feeds the waiter registry. Call once,
    /// before the protocol starts handling traffic.
    pub fn install_event_routing(protocol: &mut OfflineProtocol, waiters: Arc<Waiters>) {
        protocol.on_event(move |event| match event {
            Event::ServiceResponseReceived {
                request_id,
                status,
                body,
                ..
            } => {
                waiters.notify(&request_id, NodeEvent::Response { status, body });
            }
            Event::ExchangeReceiptIssued { receipt } => {
                let request_id = receipt.request_id.clone();
                waiters.notify(&request_id, NodeEvent::ReceiptIssued(Box::new(receipt)));
            }
            Event::ExchangeInvocationFailed { request_id, reason } => {
                waiters.notify(&request_id, NodeEvent::InvocationFailed(reason));
            }
            Event::AdapterPullCompleted {
                request_id,
                content_hash,
                size_bytes,
                data,
                ..
            } => {
                waiters.notify(
                    &request_id,
                    NodeEvent::PullCompleted {
                        content_hash,
                        size_bytes,
                        data_base64: data,
                    },
                );
            }
            Event::AdapterPullRejected {
                request_id, reason, ..
            } => {
                waiters.notify(&request_id, NodeEvent::PullRejected(reason));
            }
            _ => {}
        });
    }

    /// Drives the protocol: drains inbound messages and runs periodic
    /// processing. Spawn on a dedicated thread.
    pub fn pump_forever(state: Arc<NodeState>, tick: Duration) -> ! {
        loop {
            let started = Instant::now();
            {
                let Ok(mut protocol) = state.protocol.lock() else {
                    warn!("protocol mutex poisoned in pump; exiting");
                    std::process::exit(1);
                };
                // Drain everything currently queued on the transports.
                while protocol.receive_message().is_some() {}
                if let Err(e) = protocol.process() {
                    if !matches!(e, ProtocolError::NotStarted) {
                        debug!(error = %e, "process() tick error");
                    }
                }
            }
            let elapsed = started.elapsed();
            if elapsed < tick {
                std::thread::sleep(tick - elapsed);
            }
        }
    }
}
