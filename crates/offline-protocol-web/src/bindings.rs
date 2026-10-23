//! The JavaScript surface: [`WebEngine`] over [`Node`].
//!
//! Every method is synchronous and returns. Nothing here calls into
//! JavaScript, so the worker that owns the engine decides when events and
//! outbound frames are read (see the invariants in [`crate::node`]).

use std::sync::Arc;

use wasm_bindgen::prelude::*;

use crate::log_store::{Category, LogStats, LogStore, STORE_KEY_BYTES};
use crate::node::{LinkId, Node, NodeError};
use crate::opfs::{OpfsFile, SyncAccessHandle};
use crate::signal;

/// The folder an account's files live in, under the origin's OPFS root: the
/// namespace every binding derives for one application id and profile, so
/// two profiles never share a log.
#[wasm_bindgen(js_name = storageNamespace)]
pub fn storage_namespace(app_id: &str, profile: &str) -> String {
    offline_protocol::account_storage_namespace(app_id, profile)
}

/// A fresh room code for signalling through Nostr (W11): eighty bits, in
/// letters and digits a person can read out.
#[wasm_bindgen(js_name = signalNewRoom)]
pub fn signal_new_room() -> String {
    signal::new_room()
}

/// The relay `REQ` text that receives `room`'s signalling from `since`
/// (Unix seconds) on.
#[wasm_bindgen(js_name = signalSubscription)]
pub fn signal_subscription(
    room: &str,
    subscription_id: &str,
    since: f64,
) -> Result<String, JsError> {
    signal::subscription(room, subscription_id, since as i64).map_err(|e| JsError::new(&e))
}

/// The relay `EVENT` text carrying one connection code for `room`, sealed
/// under the room key and signed by a key used once. `role` is "offer" or
/// "answer"; `session` is the joiner's 32 hex characters.
#[wasm_bindgen(js_name = signalEvent)]
pub fn signal_event(room: &str, role: &str, session: &str, code: &str) -> Result<String, JsError> {
    let signal = signal::Signal {
        role: signal::Role::parse(role).map_err(|e| JsError::new(&e))?,
        session: session.to_string(),
        code: code.to_string(),
    };
    signal::event(room, &signal).map_err(|e| JsError::new(&e))
}

/// Opens one message a relay sent: `{role, session, code}` as JSON when it
/// is an authentic, fresh signal for `room`, otherwise `undefined`.
#[wasm_bindgen(js_name = signalOpen)]
pub fn signal_open(room: &str, relay_message: &str) -> Option<String> {
    signal::open(room, relay_message, signal::now_unix_secs()).map(|s| s.to_json())
}

/// This clock in Unix seconds, for a subscription's `since`.
#[wasm_bindgen(js_name = signalNow)]
pub fn signal_now() -> f64 {
    signal::now_unix_secs() as f64
}

type DurableStores = (Arc<LogStore<OpfsFile>>, Arc<LogStore<OpfsFile>>);

fn js(error: NodeError) -> JsError {
    JsError::new(&error.0)
}

/// One frame for the page to put on the carrier it opened for `link`.
#[wasm_bindgen]
pub struct OutboundFrame {
    link: LinkId,
    data: Vec<u8>,
}

#[wasm_bindgen]
impl OutboundFrame {
    /// The carrier this frame goes on.
    #[wasm_bindgen(getter)]
    pub fn link(&self) -> LinkId {
        self.link
    }

    /// The framed bytes, sent as one carrier message.
    #[wasm_bindgen(getter)]
    pub fn data(&self) -> Vec<u8> {
        self.data.clone()
    }
}

/// The Offline Protocol engine, one per worker.
#[wasm_bindgen]
pub struct WebEngine {
    node: Node,
    durable: Option<DurableStores>,
}

#[wasm_bindgen]
impl WebEngine {
    /// A stopped engine. `profile` names the local account; it is not the
    /// identity, which [`WebEngine::initialize`] mints.
    #[wasm_bindgen(constructor)]
    pub fn new(app_id: &str, profile: &str) -> Result<WebEngine, JsError> {
        Ok(Self {
            node: Node::new(app_id, profile).map_err(js)?,
            durable: None,
        })
    }

    /// Mints a new identity held in memory and returns its `off1…` address.
    /// The identity ends with the worker.
    pub fn initialize(&mut self) -> Result<String, JsError> {
        self.node.initialize().map_err(js)
    }

    /// Opens the account's sealed logs, loads its identity (minting it on the
    /// first run), and returns its `off1…` address.
    ///
    /// `store_key` is the 32-byte key the worker unwrapped; the four handles
    /// are the MLS log's two files and the protocol-state log's two. Refuses,
    /// and changes nothing, when a log holds records this key does not open.
    #[wasm_bindgen(js_name = initializeDurable)]
    pub fn initialize_durable(
        &mut self,
        store_key: &[u8],
        mls_first: SyncAccessHandle,
        mls_second: SyncAccessHandle,
        state_first: SyncAccessHandle,
        state_second: SyncAccessHandle,
    ) -> Result<String, JsError> {
        let key: &[u8; STORE_KEY_BYTES] = store_key
            .try_into()
            .map_err(|_| JsError::new("the store key must be 32 bytes"))?;
        let open = |category, first, second| {
            LogStore::open(key, category, OpfsFile::new(first), OpfsFile::new(second))
                .map(Arc::new)
                .map_err(|e| JsError::new(&e.to_string()))
        };
        let mls = open(Category::Mls, mls_first, mls_second)?;
        let state = open(Category::ProtocolState, state_first, state_second)?;
        let address = self
            .node
            .initialize_with(mls.clone(), state.clone())
            .map_err(js)?;
        self.durable = Some((mls, state));
        Ok(address)
    }

    /// What the durable logs have cost since they opened, as JSON
    /// (`{"mls": {...}, "protocol_state": {...}}`), or `undefined` for an
    /// engine held in memory.
    #[wasm_bindgen(js_name = storeStats)]
    pub fn store_stats(&self) -> Option<String> {
        let (mls, state) = self.durable.as_ref()?;
        let render = |stats: LogStats| {
            format!(
                r#"{{"flushes":{},"log_bytes":{},"live_bytes":{},"compactions":{}}}"#,
                stats.flushes, stats.log_bytes, stats.live_bytes, stats.compactions
            )
        };
        Some(format!(
            r#"{{"mls":{},"protocol_state":{}}}"#,
            render(mls.stats()),
            render(state.stats())
        ))
    }

    /// Starts the engine.
    pub fn start(&mut self) -> Result<(), JsError> {
        self.node.start().map_err(js)
    }

    /// Stops the engine, reporting every link lost first.
    pub fn stop(&mut self) -> Result<(), JsError> {
        self.node.stop().map_err(js)
    }

    /// This device's address, once initialized.
    #[wasm_bindgen(js_name = localAddress)]
    pub fn local_address(&self) -> Option<String> {
        self.node.local_address()
    }

    /// One tick. Call every 100 ms.
    pub fn process(&mut self) -> Result<(), JsError> {
        self.node.process().map_err(js)
    }

    /// The next event as the engine's JSON, or `undefined`.
    #[wasm_bindgen(js_name = pollEvent)]
    pub fn poll_event(&self) -> Option<String> {
        self.node.poll_event()
    }

    /// Registers a carrier the page opened; returns its link id.
    #[wasm_bindgen(js_name = openLink)]
    pub fn open_link(&mut self) -> Result<LinkId, JsError> {
        self.node.open_link().map_err(js)
    }

    /// Bytes a carrier delivered. Returns the reason to close it, or
    /// `undefined` to keep it open.
    #[wasm_bindgen(js_name = linkReceived)]
    pub fn link_received(&mut self, link: LinkId, bytes: &[u8]) -> Option<String> {
        self.node.link_received(link, bytes)
    }

    /// A carrier closed.
    #[wasm_bindgen(js_name = closeLink)]
    pub fn close_link(&mut self, link: LinkId) {
        self.node.close_link(link);
    }

    /// The next frame to send, or `undefined`.
    #[wasm_bindgen(js_name = nextOutbound)]
    pub fn next_outbound(&mut self) -> Option<OutboundFrame> {
        self.node
            .next_outbound()
            .map(|(link, data)| OutboundFrame { link, data })
    }

    /// The next link the engine closed on its own (a preamble that never
    /// came), or `undefined`. The page closes that carrier and tells the
    /// other end, as for a link `linkReceived` refused.
    #[wasm_bindgen(js_name = nextClosed)]
    pub fn next_closed(&mut self) -> Option<LinkId> {
        self.node.next_closed()
    }

    /// Sends a direct message; returns its id. `ephemeral` sends it once
    /// over the live link, never acknowledged or retried (W10).
    #[wasm_bindgen(js_name = sendMessage)]
    pub fn send_message(
        &mut self,
        recipient: &str,
        content: &str,
        ephemeral: bool,
    ) -> Result<String, JsError> {
        self.node
            .send_message(recipient, content, ephemeral)
            .map_err(js)
    }

    /// Creates a group; returns its info as JSON.
    #[wasm_bindgen(js_name = createGroup)]
    pub fn create_group(&mut self, name: &str) -> Result<String, JsError> {
        self.node.create_group(name).map_err(js)
    }

    /// Adds a member to a group this device administers.
    #[wasm_bindgen(js_name = inviteToGroup)]
    pub fn invite_to_group(&mut self, group_id: &str, member: &str) -> Result<(), JsError> {
        self.node.invite_to_group(group_id, member).map_err(js)
    }

    /// Sends to a group; returns the per-member message ids as JSON.
    /// `ephemeral` as for [`WebEngine::send_message`].
    #[wasm_bindgen(js_name = sendGroupMessage)]
    pub fn send_group_message(
        &mut self,
        group_id: &str,
        content: &str,
        ephemeral: bool,
    ) -> Result<String, JsError> {
        self.node
            .send_group_message(group_id, content, ephemeral)
            .map_err(js)
    }
}
