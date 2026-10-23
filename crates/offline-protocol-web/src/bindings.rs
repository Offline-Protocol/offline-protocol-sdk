//! The JavaScript surface: [`WebEngine`] over [`Node`].
//!
//! Every method is synchronous and returns. Nothing here calls into
//! JavaScript, so the worker that owns the engine decides when events and
//! outbound frames are read (see the invariants in [`crate::node`]).

use wasm_bindgen::prelude::*;

use crate::node::{LinkId, Node, NodeError};

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
}

#[wasm_bindgen]
impl WebEngine {
    /// A stopped engine. `profile` names the local account; it is not the
    /// identity, which [`WebEngine::initialize`] mints.
    #[wasm_bindgen(constructor)]
    pub fn new(app_id: &str, profile: &str) -> Result<WebEngine, JsError> {
        Ok(Self {
            node: Node::new(app_id, profile).map_err(js)?,
        })
    }

    /// Mints this device's identity and returns its `off1…` address.
    pub fn initialize(&mut self) -> Result<String, JsError> {
        self.node.initialize().map_err(js)
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

    /// Sends a direct message; returns its id.
    #[wasm_bindgen(js_name = sendMessage)]
    pub fn send_message(&mut self, recipient: &str, content: &str) -> Result<String, JsError> {
        self.node.send_message(recipient, content).map_err(js)
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
    #[wasm_bindgen(js_name = sendGroupMessage)]
    pub fn send_group_message(&mut self, group_id: &str, content: &str) -> Result<String, JsError> {
        self.node.send_group_message(group_id, content).map_err(js)
    }
}
