//! The localhost HTTP control API.
//!
//! This is the surface the Capability Exchange MCP server's `node` mode
//! talks to (`NodeMeshBridge` in `capability-exchange/packages/mcp-server`).
//! The route shapes mirror the TypeScript `MeshBridge` interface exactly —
//! every JSON body serializes the same structs the exchange crate defines,
//! so the two sides can never drift independently of the wire types.
//!
//! Trust model: this API can spend the node's prepaid mesh balance, so it
//! binds to localhost by default and supports a bearer token. It is a
//! *local control plane* — never expose it to a network. Agent-side
//! guardrails (budgets, quotes, confirmation) live in the MCP server.

use crate::state::{NodeEvent, NodeState};
use offline_protocol::{ListingFilter, ListingKind, Terms};
use offline_protocol_core::{ServiceDescriptor, ServiceId};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server};
use tracing::{info, warn};

const MAX_BODY_BYTES: usize = 256 * 1024;
const DEFAULT_INVOKE_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 120_000;
/// After the response arrives, how long to wait for the signed receipt.
const RECEIPT_GRACE_MS: u64 = 3_000;

#[derive(Deserialize)]
struct DiscoverBody {
    service_id: Option<String>,
}

#[derive(Deserialize)]
struct InvokeBody {
    provider: String,
    service_id: String,
    method: String,
    body: String,
    #[serde(default)]
    max_units: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct PullBody {
    provider: String,
    service_id: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct CreditBody {
    currency: String,
    amount_minor: u64,
}

#[derive(Deserialize)]
struct MarkSettledBody {
    receipt_ids: Vec<String>,
}

#[derive(Deserialize)]
struct PublishBody {
    service_id: String,
    version: String,
    #[serde(default)]
    capabilities: HashMap<String, String>,
    kind: String,
    terms: Terms,
}

#[derive(Deserialize)]
struct PublishAdapterBody {
    service_id: String,
    version: String,
    #[serde(default)]
    capabilities: HashMap<String, String>,
    terms: Terms,
    base_model: String,
    base_model_version: String,
    artifact_path: String,
}

enum Reply {
    Json(u16, Value),
}

fn ok(value: Value) -> Reply {
    Reply::Json(200, value)
}

fn err(status: u16, message: impl Into<String>) -> Reply {
    Reply::Json(status, json!({ "error": message.into() }))
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|e| json!({ "error": format!("serialize: {e}") }))
}

fn clamp_timeout(requested: Option<u64>) -> Duration {
    Duration::from_millis(
        requested
            .unwrap_or(DEFAULT_INVOKE_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS),
    )
}

/// Runs the HTTP server forever on the given `tiny_http::Server`.
pub fn serve(server: Server, state: Arc<NodeState>) -> ! {
    info!(
        bind = %state.config.bind,
        port = state.config.port,
        auth = if state.config.api_token.is_some() { "bearer" } else { "DISABLED" },
        "node control API listening"
    );
    if state.config.api_token.is_none() {
        warn!("NODE_API_TOKEN is not set — the control API is unauthenticated (dev mode)");
    }
    loop {
        let request = match server.recv() {
            Ok(request) => request,
            Err(e) => {
                warn!(error = %e, "accept error");
                continue;
            }
        };
        let state = Arc::clone(&state);
        std::thread::spawn(move || handle_connection(request, state));
    }
}

fn handle_connection(mut request: Request, state: Arc<NodeState>) {
    let reply = route(&mut request, &state);
    let Reply::Json(status, value) = reply;
    let body = value.to_string();
    let response = Response::from_string(body)
        .with_status_code(status)
        .with_header(
            Header::from_bytes("content-type", "application/json").expect("static header"),
        );
    if let Err(e) = request.respond(response) {
        warn!(error = %e, "failed to write response");
    }
}

fn authorized(request: &Request, state: &NodeState) -> bool {
    let Some(expected) = &state.config.api_token else {
        return true; // dev mode
    };
    let header = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("authorization"))
        .map(|h| h.value.as_str().to_string());
    matches!(header, Some(value) if value == format!("Bearer {expected}"))
}

fn read_body(request: &mut Request) -> Result<Vec<u8>, Reply> {
    if request
        .body_length()
        .is_some_and(|len| len > MAX_BODY_BYTES)
    {
        return Err(err(413, "request body too large"));
    }
    let mut buf = Vec::new();
    let mut reader = request.as_reader().take(MAX_BODY_BYTES as u64 + 1);
    if reader.read_to_end(&mut buf).is_err() {
        return Err(err(400, "failed to read request body"));
    }
    if buf.len() > MAX_BODY_BYTES {
        return Err(err(413, "request body too large"));
    }
    Ok(buf)
}

fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, Reply> {
    serde_json::from_slice(bytes).map_err(|e| err(400, format!("invalid request body: {e}")))
}

fn route(request: &mut Request, state: &Arc<NodeState>) -> Reply {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let method = request.method().clone();

    if path == "/healthz" {
        return healthz(state);
    }
    if !authorized(request, state) {
        return err(401, "missing or invalid bearer token");
    }

    match (method, path.as_str()) {
        (Method::Post, "/v1/discover") => {
            let body = match read_body(request) {
                Ok(b) => b,
                Err(reply) => return reply,
            };
            let parsed: DiscoverBody = if body.is_empty() {
                DiscoverBody { service_id: None }
            } else {
                match parse(&body) {
                    Ok(p) => p,
                    Err(reply) => return reply,
                }
            };
            discover(state, parsed)
        }
        (Method::Get, "/v1/listings") => listings(state),
        (Method::Post, "/v1/invoke") => match read_body(request).and_then(|b| parse(&b)) {
            Ok(parsed) => invoke(state, parsed),
            Err(reply) => reply,
        },
        (Method::Post, "/v1/adapters/pull") => match read_body(request).and_then(|b| parse(&b)) {
            Ok(parsed) => pull_adapter(state, parsed),
            Err(reply) => reply,
        },
        (Method::Get, "/v1/balance") => balance(state, &url),
        (Method::Post, "/v1/balance/credit") => match read_body(request).and_then(|b| parse(&b)) {
            Ok(parsed) => credit(state, parsed),
            Err(reply) => reply,
        },
        (Method::Get, "/v1/receipts/pending") => pending_receipts(state),
        (Method::Post, "/v1/receipts/mark-settled") => {
            match read_body(request).and_then(|b| parse(&b)) {
                Ok(parsed) => mark_settled(state, parsed),
                Err(reply) => reply,
            }
        }
        (Method::Post, "/v1/listings/publish") => {
            match read_body(request).and_then(|b| parse(&b)) {
                Ok(parsed) => publish(state, parsed),
                Err(reply) => reply,
            }
        }
        (Method::Post, "/v1/adapters/publish") => {
            match read_body(request).and_then(|b| parse(&b)) {
                Ok(parsed) => publish_adapter(state, parsed),
                Err(reply) => reply,
            }
        }
        _ => err(404, "no such route"),
    }
}

fn healthz(state: &NodeState) -> Reply {
    let Ok(protocol) = state.protocol.lock() else {
        return err(500, "protocol mutex poisoned");
    };
    ok(json!({
        "ok": true,
        "user_id": state.config.user_id,
        "state": format!("{:?}", protocol.state()),
    }))
}

fn discover(state: &NodeState, body: DiscoverBody) -> Reply {
    let Ok(mut protocol) = state.protocol.lock() else {
        return err(500, "protocol mutex poisoned");
    };
    match protocol.discover_listings(body.service_id.as_deref()) {
        Ok(query_id) => ok(json!({ "query_id": query_id })),
        Err(e) => err(502, e.to_string()),
    }
}

fn listings(state: &NodeState) -> Reply {
    let Ok(protocol) = state.protocol.lock() else {
        return err(500, "protocol mutex poisoned");
    };
    ok(to_json(
        &protocol.discovered_listings(&ListingFilter::default()),
    ))
}

fn invoke(state: &Arc<NodeState>, body: InvokeBody) -> Reply {
    let timeout = clamp_timeout(body.timeout_ms);
    let max_units = body.max_units.unwrap_or(1).max(1);

    // Start the invocation and learn whether a receipt is expected, all
    // under one short lock.
    let (request_id, priced, rx) = {
        let Ok(mut protocol) = state.protocol.lock() else {
            return err(500, "protocol mutex poisoned");
        };
        let priced = protocol
            .discovered_listing(&body.provider, &body.service_id)
            .map(|d| {
                d.listing
                    .terms
                    .price
                    .map(|p| p.amount_minor > 0)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        let request_id = match protocol.invoke_listing(
            &body.provider,
            &body.service_id,
            &body.method,
            &body.body,
            max_units,
        ) {
            Ok(id) => id,
            Err(e) => return err(422, e.to_string()),
        };
        let rx = state.waiters.register(&request_id);
        (request_id, priced, rx)
    };

    let outcome = wait_for_invocation(&rx, timeout, priced);
    state.waiters.remove(&request_id);

    match outcome {
        InvokeOutcome::Done {
            status,
            body,
            receipt,
        } => {
            let mut payload = json!({
                "request_id": request_id,
                "status": status,
                "body": body,
            });
            if let Some(receipt) = receipt {
                payload["receipt"] = to_json(&receipt);
            }
            ok(payload)
        }
        InvokeOutcome::Failed(reason) => err(502, reason),
        InvokeOutcome::TimedOut => err(
            504,
            format!(
                "no response within {} ms (request {request_id})",
                timeout.as_millis()
            ),
        ),
    }
}

enum InvokeOutcome {
    Done {
        status: String,
        body: String,
        // Boxed: a receipt is ~10 strings and would dominate the enum size.
        receipt: Option<Box<offline_protocol_exchange::UsageReceipt>>,
    },
    Failed(String),
    TimedOut,
}

/// Collects events for one invocation. Receipt and response can arrive in
/// either order (the receipt is emitted during the same dispatch as the
/// response); for priced listings we wait a short grace period for the
/// receipt after the response lands.
fn wait_for_invocation(rx: &Receiver<NodeEvent>, timeout: Duration, priced: bool) -> InvokeOutcome {
    let deadline = Instant::now() + timeout;
    let mut response: Option<(String, String)> = None;
    let mut receipt: Option<Box<offline_protocol_exchange::UsageReceipt>> = None;
    let mut receipt_deadline: Option<Instant> = None;

    loop {
        let now = Instant::now();
        let until = match (response.is_some(), receipt_deadline) {
            (true, Some(rd)) => rd,
            _ => deadline,
        };
        if now >= until {
            break;
        }
        match rx.recv_timeout(until - now) {
            Ok(NodeEvent::Response { status, body }) => {
                response = Some((status, body));
                if !priced || receipt.is_some() {
                    break;
                }
                receipt_deadline = Some(Instant::now() + Duration::from_millis(RECEIPT_GRACE_MS));
            }
            Ok(NodeEvent::ReceiptIssued(r)) => {
                receipt = Some(r);
                if response.is_some() {
                    break;
                }
            }
            Ok(NodeEvent::InvocationFailed(reason)) => return InvokeOutcome::Failed(reason),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    match response {
        Some((status, body)) => InvokeOutcome::Done {
            status,
            body,
            receipt,
        },
        None => InvokeOutcome::TimedOut,
    }
}

fn pull_adapter(state: &Arc<NodeState>, body: PullBody) -> Reply {
    let timeout = clamp_timeout(body.timeout_ms);
    let (request_id, rx) = {
        let Ok(mut protocol) = state.protocol.lock() else {
            return err(500, "protocol mutex poisoned");
        };
        let request_id = match protocol.pull_adapter(&body.provider, &body.service_id) {
            Ok(id) => id,
            Err(e) => return err(422, e.to_string()),
        };
        let rx = state.waiters.register(&request_id);
        (request_id, rx)
    };

    let outcome = rx.recv_timeout(timeout);
    state.waiters.remove(&request_id);
    match outcome {
        Ok(NodeEvent::PullCompleted {
            content_hash,
            size_bytes,
            data_base64,
        }) => ok(json!({
            "request_id": request_id,
            "service_id": body.service_id,
            "content_hash": content_hash,
            "size_bytes": size_bytes,
            "data_base64": data_base64,
        })),
        Ok(NodeEvent::PullRejected(reason)) => err(502, format!("adapter rejected: {reason}")),
        Ok(_) => err(502, "unexpected event for adapter pull"),
        Err(_) => err(
            504,
            format!(
                "no artifact within {} ms (request {request_id})",
                timeout.as_millis()
            ),
        ),
    }
}

fn balance(state: &NodeState, url: &str) -> Reply {
    let currency = url
        .split('?')
        .nth(1)
        .and_then(|qs| {
            qs.split('&')
                .find_map(|pair| pair.strip_prefix("currency="))
        })
        .unwrap_or("USD");
    let Ok(protocol) = state.protocol.lock() else {
        return err(500, "protocol mutex poisoned");
    };
    ok(to_json(&protocol.exchange_balance(currency)))
}

fn credit(state: &NodeState, body: CreditBody) -> Reply {
    let Ok(mut protocol) = state.protocol.lock() else {
        return err(500, "protocol mutex poisoned");
    };
    match protocol.credit_exchange_balance(&body.currency, body.amount_minor) {
        Ok(balance) => ok(to_json(&balance)),
        Err(e) => err(422, e.to_string()),
    }
}

fn pending_receipts(state: &NodeState) -> Reply {
    let Ok(protocol) = state.protocol.lock() else {
        return err(500, "protocol mutex poisoned");
    };
    ok(to_json(&protocol.pending_exchange_receipts()))
}

fn mark_settled(state: &NodeState, body: MarkSettledBody) -> Reply {
    let Ok(mut protocol) = state.protocol.lock() else {
        return err(500, "protocol mutex poisoned");
    };
    protocol.mark_exchange_receipts_settled(&body.receipt_ids);
    ok(json!({ "marked": body.receipt_ids.len() }))
}

fn parse_kind(kind: &str) -> Result<ListingKind, Reply> {
    match kind {
        "service" => Ok(ListingKind::Service),
        "adapter" => Ok(ListingKind::Adapter),
        other => Err(err(400, format!("invalid kind '{other}'"))),
    }
}

fn descriptor(
    service_id: &str,
    version: String,
    capabilities: HashMap<String, String>,
) -> Result<ServiceDescriptor, Reply> {
    let service_id =
        ServiceId::new(service_id).map_err(|e| err(400, format!("invalid service id: {e}")))?;
    Ok(ServiceDescriptor {
        service_id,
        version,
        capabilities,
    })
}

fn publish(state: &NodeState, body: PublishBody) -> Reply {
    let kind = match parse_kind(&body.kind) {
        Ok(kind) => kind,
        Err(reply) => return reply,
    };
    let descriptor = match descriptor(&body.service_id, body.version, body.capabilities) {
        Ok(d) => d,
        Err(reply) => return reply,
    };
    let Ok(mut protocol) = state.protocol.lock() else {
        return err(500, "protocol mutex poisoned");
    };
    match protocol.publish_listing(descriptor, kind, body.terms, None) {
        Ok(listing) => ok(to_json(&listing)),
        Err(e) => err(422, e.to_string()),
    }
}

fn publish_adapter(state: &NodeState, body: PublishAdapterBody) -> Reply {
    let descriptor = match descriptor(&body.service_id, body.version, body.capabilities) {
        Ok(d) => d,
        Err(reply) => return reply,
    };
    let Ok(mut protocol) = state.protocol.lock() else {
        return err(500, "protocol mutex poisoned");
    };
    match protocol.publish_adapter_listing(
        descriptor,
        body.terms,
        &body.base_model,
        &body.base_model_version,
        &body.artifact_path,
    ) {
        Ok(listing) => ok(to_json(&listing)),
        Err(e) => err(422, e.to_string()),
    }
}
