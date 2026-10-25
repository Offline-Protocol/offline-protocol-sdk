//! The Nostr transport, driven from the worker (W12).
//!
//! No binding opens a relay socket in Rust: Swift, Kotlin and Python each hold
//! their own and move text, and this module is the browser's copy of their
//! host half. The worker holds the sockets; everything that decides what an
//! event means happens here, through the one `NostrTransport` every binding
//! shares.
//!
//! # Invariants
//!
//! - **The relays are the application's.** Nothing here names one, and the
//!   worker refuses to enable Nostr without a list. A relay the SDK chose would
//!   become the relay every application uses.
//! - **A relay message is opened here, never in JavaScript.** The worker reads
//!   a message's type, its subscription id and an `OK`'s verdict, the same
//!   fields the mobile bridges read; an `EVENT` crosses whole and is decoded,
//!   unsealed and attributed by [`Node::nostr_relay_event`].
//! - **The receive path is the FFI's, step for step.** Unseal once, deliver
//!   without a transport peer id (a Nostr key is not a protocol identity),
//!   drain, advance the watermark only for bytes that decoded into a message,
//!   then the reachability notice under the message's own sender. A different
//!   order lets undecodable traffic to this device's public tag move the
//!   watermark past history a relay still owes it.
//! - **A resolution query belongs to the worker until it says so.** The query
//!   ids handed out by [`Node::nostr_next_query`] are remembered until
//!   [`Node::nostr_query_completed`], so an `EVENT` under one of them is a
//!   record to open, and any other `EVENT` is a message.

use std::collections::VecDeque;
use std::sync::Arc;

use base64::Engine as _;
use offline_protocol::Event;
use offline_protocol_transport::nostr::ResolvedRecord;
use offline_protocol_transport::{NostrTransport, Transport, TransportStatus, TransportType};
use serde_json::Value;
use tracing::warn;

use crate::node::{push_event, Node, NodeError, NodeResult};

/// Resolution queries remembered at once, at most. The transport bounds its
/// own queue at 64; this is the worker's copy of the ids it handed out, and a
/// worker that never completes one must not grow it without bound.
const MAX_OPEN_QUERIES: usize = 64;

/// The largest relay message opened. The transport refuses to publish an
/// event over 64 KiB and refuses a resolution record over the same size, so
/// this leaves room for the `["EVENT", "<subscription>", ...]` wrapper only.
const MAX_RELAY_MESSAGE_BYTES: usize = 64 * 1024 + 256;

/// What an application turns on when it gives the node relays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NostrOptions {
    /// Publish key packages and resolve peers' (cold first contact).
    pub cold_contact: bool,
    /// Publish this profile as a username, and resolve others'. Off unless
    /// asked for: it binds a readable name to an address in a public place.
    pub username_discovery: bool,
}

impl Default for NostrOptions {
    fn default() -> Self {
        Self {
            cold_contact: true,
            username_discovery: false,
        }
    }
}

/// The worker's side of the transport: whether a relay is live, and the
/// queries it holds.
#[derive(Debug, Default)]
pub(crate) struct NostrHost {
    pub(crate) live: bool,
    queries: VecDeque<String>,
}

/// One signed event for the worker to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundEvent {
    /// The engine's id for what this event carries. The worker reports the
    /// relays' verdict under it.
    pub message_id: String,
    /// The Nostr event id the relays' `OK` names.
    pub event_id: String,
    /// The complete `["EVENT", {...}]` text, sent verbatim.
    pub event_json: String,
}

/// One resolution query for the worker to send to every live relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundQuery {
    /// Also the subscription id the query runs under.
    pub query_id: String,
    /// The complete `["REQ", ...]` text.
    pub req_json: String,
}

impl Node {
    /// The `REQ` text that receives this device's messages, under
    /// `subscription_id`. The worker sends it on every connect: its `since`
    /// moves with the persisted receive watermark.
    pub fn nostr_subscription(&self, subscription_id: &str) -> NodeResult<String> {
        self.with_nostr(|nostr| nostr.create_subscription(subscription_id))?
            .map_err(|e| NodeError(e.to_string()))
    }

    /// The worker has a live relay (`true`) or none (`false`). The first
    /// relay up flushes the outbox, as the FFI does.
    pub fn nostr_status_changed(&mut self, live: bool) -> NodeResult<()> {
        // The slot first: a node with options but no address yet has no
        // transport, and must not record a liveness nothing acted on.
        let transport = self.nostr_slot()?;
        let host = self
            .nostr_host
            .as_mut()
            .ok_or_else(|| NodeError("Nostr is not enabled on this node".to_string()))?;
        let was_live = host.live;
        host.live = live;
        transport.on_status_changed(if live {
            TransportStatus::Available
        } else {
            TransportStatus::Disconnected
        });
        if live && !was_live {
            self.protocol.flush_outbox_all();
        }
        if live != was_live {
            push_event(
                &self.events,
                &if live {
                    Event::TransportSwitched {
                        from: None,
                        to: "Nostr".to_string(),
                        reason: "Connected to Nostr relays".to_string(),
                    }
                } else {
                    Event::TransportSwitched {
                        from: Some("Nostr".to_string()),
                        to: "None".to_string(),
                        reason: "Disconnected from Nostr relays".to_string(),
                    }
                },
            );
        }
        Ok(())
    }

    /// The next event to publish, while a relay is live. Key-package and
    /// discovery publications come first, then messages, in the transport's
    /// order.
    pub fn nostr_next_event(&mut self) -> Option<OutboundEvent> {
        if !self.nostr_host.as_ref()?.live {
            return None;
        }
        let transport = self.nostr_slot().ok()?;
        let nostr = transport.as_any().downcast_ref::<NostrTransport>()?;
        match nostr.get_next_signed_event() {
            Ok(Some(signed)) => Some(OutboundEvent {
                message_id: signed.message_id,
                event_id: signed.event_id,
                event_json: signed.event_json,
            }),
            Ok(None) => None,
            Err(error) => {
                warn!(%error, "nostr: could not sign an outbound event");
                None
            }
        }
    }

    /// A relay accepted the event carrying `message_id`.
    pub fn nostr_confirm_sent(&mut self, message_id: &str) {
        if let Err(error) = self.protocol.on_transport_send_confirmed(message_id) {
            warn!(%message_id, %error, "nostr: could not apply a send confirmation");
        }
        if let Ok(transport) = self.nostr_slot() {
            transport.confirm_sent(message_id);
        }
    }

    /// Every relay the event carrying `message_id` went to refused it, or
    /// none could take it.
    pub fn nostr_send_failed(&mut self, message_id: &str, reason: Option<String>) {
        let reason = reason.or_else(|| Some("Nostr transport send failed".to_string()));
        if let Err(error) = self.protocol.on_transport_send_failed_via(
            message_id,
            reason,
            Some(TransportType::Nostr),
        ) {
            warn!(%message_id, %error, "nostr: could not apply a send failure");
        }
        if let Ok(transport) = self.nostr_slot() {
            transport.report_send_failure(message_id);
        }
    }

    /// The next resolution query, while a relay is live. A username query
    /// opens its accumulator here, at mint time, so a name nobody claims
    /// still answers.
    pub fn nostr_next_query(&mut self) -> Option<OutboundQuery> {
        if !self.nostr_host.as_ref()?.live {
            return None;
        }
        let transport = self.nostr_slot().ok()?;
        let nostr = transport.as_any().downcast_ref::<NostrTransport>()?;
        let query = match nostr.next_query() {
            Ok(Some(query)) => query,
            Ok(None) => return None,
            Err(error) => {
                warn!(%error, "nostr: could not build a resolution query");
                return None;
            }
        };
        if let Some(username) = query.discovery_username.clone() {
            self.protocol
                .begin_username_resolution(query.query_id.clone(), username);
        }
        let host = self.nostr_host.as_mut()?;
        if host.queries.len() >= MAX_OPEN_QUERIES {
            host.queries.pop_front();
        }
        host.queries.push_back(query.query_id.clone());
        Some(OutboundQuery {
            query_id: query.query_id,
            req_json: query.req_json,
        })
    }

    /// Every relay the query went to has answered (or the worker stopped
    /// waiting). A username resolution emits its one `username_resolved`
    /// event here.
    pub fn nostr_query_completed(&mut self, query_id: &str) {
        if let Some(host) = self.nostr_host.as_mut() {
            host.queries.retain(|open| open != query_id);
        }
        let Ok(transport) = self.nostr_slot() else {
            return;
        };
        let Some(nostr) = transport.as_any().downcast_ref::<NostrTransport>() else {
            return;
        };
        if nostr.complete_query(query_id).is_some() {
            self.protocol.flush_username_resolution(query_id);
        }
    }

    /// One `["EVENT", <subscription>, {...}]` a relay sent, whole. Anything
    /// that does not open (another device's traffic, a squatter on a public
    /// tag, junk) is dropped: a public tag returns whatever a relay holds
    /// there, so none of that is an error.
    pub fn nostr_relay_event(&mut self, relay_message: &str) -> NodeResult<()> {
        if relay_message.len() > MAX_RELAY_MESSAGE_BYTES {
            warn!(
                len = relay_message.len(),
                "nostr: ignoring an oversized relay message"
            );
            return Ok(());
        }
        let Ok(Value::Array(parts)) = serde_json::from_str::<Value>(relay_message) else {
            return Ok(());
        };
        let (Some(Value::String(kind)), Some(Value::String(subscription)), Some(event)) =
            (parts.first(), parts.get(1), parts.get(2))
        else {
            return Ok(());
        };
        if kind != "EVENT" || !event.is_object() {
            return Ok(());
        }
        let is_query = self
            .nostr_host
            .as_ref()
            .is_some_and(|host| host.queries.iter().any(|open| open == subscription));
        if is_query {
            self.open_query_event(subscription, event)
        } else {
            self.receive_message_event(event)
        }
    }

    fn open_query_event(&mut self, query_id: &str, event: &Value) -> NodeResult<()> {
        let event_json = event.to_string();
        let opened = self
            .with_nostr(|nostr| nostr.open_query_event(query_id, &event_json))?
            .map_err(|e| NodeError(e.to_string()))?;
        match opened {
            Some(ResolvedRecord::KeyPackage { plaintext, .. }) => {
                // Anyone can publish a record that opens: its seal key derives
                // from the queried address. One that then does not decode is
                // junk on a public tag, dropped like any other, not an error
                // the page hears.
                if let Err(error) = self.protocol.handle_resolved_key_package(&plaintext) {
                    warn!(%error, "nostr: a resolved key-package record did not decode");
                    return Ok(());
                }
                while self.protocol.receive_message().is_some() {}
            }
            Some(ResolvedRecord::Discovery {
                username: _,
                author,
                plaintext,
            }) => {
                self.protocol
                    .handle_resolved_discovery_record(query_id, &author, &plaintext);
            }
            None => {}
        }
        Ok(())
    }

    fn receive_message_event(&mut self, event: &Value) -> NodeResult<()> {
        let (Some(sender), Some(content)) = (
            event.get("pubkey").and_then(Value::as_str),
            event.get("content").and_then(Value::as_str),
        ) else {
            return Ok(());
        };
        let created_at = event.get("created_at").and_then(Value::as_i64);
        let transport = self.nostr_slot()?;
        let Some(nostr) = transport.as_any().downcast_ref::<NostrTransport>() else {
            return Ok(());
        };
        // Only the legacy unsealed form can match: a gift wrap is signed by a
        // key used once, and nothing on it names its author.
        if sender == nostr.public_key_hex() {
            return Ok(());
        }
        let raw = base64::engine::general_purpose::STANDARD
            .decode(content)
            .unwrap_or_else(|_| content.as_bytes().to_vec());
        let data = nostr.unseal_event_payload(sender, &raw).into_owned();
        // The protocol sender is the identity; the Nostr key only unseals.
        let real_sender = transport
            .deserialize_message(&data)
            .ok()
            .map(|message| message.sender.as_str().to_string());
        transport
            .on_data_received(data)
            .map_err(|e| NodeError(format!("Failed to process nostr message: {e}")))?;
        while self.protocol.receive_message().is_some() {}
        let Some(peer) = real_sender else {
            warn!(nostr_pubkey = %sender, "nostr: undecodable frame; no discovery");
            return Ok(());
        };
        if let Some(created_at) = created_at {
            self.protocol.note_nostr_event_received(created_at);
        }
        if peer.is_empty() {
            return Ok(());
        }
        let blocked = self.protocol.is_user_blocked(&peer);
        self.protocol.on_neighbor_discovered(&peer);
        if !blocked {
            push_event(
                &self.events,
                &Event::NeighborDiscovered {
                    peer_id: peer,
                    transport: "Nostr".to_string(),
                    rssi: None,
                },
            );
        }
        Ok(())
    }

    fn nostr_slot(&self) -> NodeResult<Arc<dyn Transport>> {
        self.protocol
            .transport_manager()
            .get_transport(TransportType::Nostr)
            .ok_or_else(|| NodeError("Nostr is not enabled on this node".to_string()))
    }

    fn with_nostr<T>(&self, f: impl FnOnce(&NostrTransport) -> T) -> NodeResult<T> {
        let transport = self.nostr_slot()?;
        let nostr = transport
            .as_any()
            .downcast_ref::<NostrTransport>()
            .ok_or_else(|| NodeError("the Nostr slot holds another transport".to_string()))?;
        Ok(f(nostr))
    }
}
