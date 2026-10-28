//! The JavaScript surface: [`WebEngine`] over [`Node`].
//!
//! Every method is synchronous and returns. Nothing here calls into
//! JavaScript, so the worker that owns the engine decides when events and
//! outbound frames are read (see the invariants in [`crate::node`]).

use std::sync::Arc;

use wasm_bindgen::prelude::*;

use crate::log_store::{Category, LogStats, LogStore, STORE_KEY_BYTES};
use crate::node::{LinkId, Node, NodeError, NodeOptions};
use crate::nostr_host::NostrOptions;
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

/// The relay `REQ` text that receives `room`'s signalling.
///
/// It asks from as far back as [`signal::open`] accepts an event, so a peer
/// whose clock runs behind this one by anything inside that window is
/// forwarded and then accepted, rather than filtered out by the relay first.
#[wasm_bindgen(js_name = signalSubscription)]
pub fn signal_subscription(room: &str, subscription_id: &str) -> Result<String, JsError> {
    let since = signal::now_unix_secs().saturating_sub(signal::MAX_EVENT_AGE_SECS);
    signal::subscription(room, subscription_id, since).map_err(|e| JsError::new(&e))
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

/// Opens one message a relay sent: `{role, session, code, freshUntilMs}` as
/// JSON when it is an authentic, fresh signal for `room`, otherwise
/// `undefined`. `freshUntilMs` is when this clock stops accepting the event
/// ([`signal::Opened::fresh_until`]).
#[wasm_bindgen(js_name = signalOpen)]
pub fn signal_open(room: &str, relay_message: &str) -> Option<String> {
    signal::open(room, relay_message, signal::now_unix_secs()).map(|o| o.to_json())
}

/// Opens an invite someone shared: `{address, publicKey, petname, signed}` as
/// JSON, the address already checked to derive from the key. Refuses a
/// malformed blob, an address that is not the key's, and a signature that
/// does not verify; each means the invite must not be shown at all.
#[wasm_bindgen(js_name = parseInvite)]
pub fn parse_invite(blob: &str) -> Result<String, JsError> {
    use base64::Engine as _;
    let invite = offline_protocol_mls::invite::parse_invite(blob)
        .map_err(|e| JsError::new(&e.to_string()))?;
    Ok(serde_json::json!({
        "address": invite.address.to_string(),
        "publicKey": base64::engine::general_purpose::STANDARD.encode(&invite.public_key),
        "petname": invite.petname,
        "signed": invite.signed,
    })
    .to_string())
}

/// One device's place in a nearby room (W13): the room behind one network
/// address for one application, and the key this device announces there.
#[wasm_bindgen(js_name = NearbyRoomKeys)]
pub struct WebNearby {
    inner: crate::nearby::Nearby,
}

#[wasm_bindgen(js_class = NearbyRoomKeys)]
impl WebNearby {
    /// Joins the room for `app_id` behind `network` (the address a STUN
    /// server reports: IPv4, or IPv6 taken to its /64), under `label`.
    #[wasm_bindgen(constructor)]
    pub fn new(app_id: &str, network: &str, label: &str) -> Result<WebNearby, JsError> {
        crate::nearby::Nearby::new(app_id, network, label)
            .map(|inner| Self { inner })
            .map_err(|e| JsError::new(&e))
    }

    /// The key this device announces.
    #[wasm_bindgen(getter)]
    pub fn key(&self) -> String {
        self.inner.key().to_string()
    }

    /// The `REQ` text receiving the room, from as far back as an event is
    /// accepted.
    pub fn subscription(&self, subscription_id: &str) -> Result<String, JsError> {
        let since = signal::now_unix_secs().saturating_sub(signal::MAX_EVENT_AGE_SECS);
        self.inner
            .subscription(subscription_id, since)
            .map_err(|e| JsError::new(&e))
    }

    /// The `EVENT` text announcing this device.
    pub fn presence(&self) -> Result<String, JsError> {
        self.inner.presence().map_err(|e| JsError::new(&e))
    }

    /// The `EVENT` text offering `code` to the device that announced `to`.
    pub fn offer(&self, to: &str, session: &str, code: &str) -> Result<String, JsError> {
        self.inner
            .offer(to, session, code)
            .map_err(|e| JsError::new(&e))
    }

    /// The `EVENT` text answering the joiner whose presence key is `to`.
    pub fn answer(&self, to: &str, session: &str, code: &str) -> Result<String, JsError> {
        self.inner
            .answer(to, session, code)
            .map_err(|e| JsError::new(&e))
    }

    /// Opens a relay message: `{kind, peer, label, session, code,
    /// createdAtMs, freshUntilMs}` as JSON for a presence from another
    /// device or an offer or answer sealed to this one, else `undefined`.
    pub fn open(&self, relay_message: &str) -> Option<String> {
        self.inner
            .open(relay_message, signal::now_unix_secs())
            .map(|opened| opened.to_json())
    }
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

/// One fragment for the page to write to a peripheral's Message
/// characteristic.
#[wasm_bindgen]
pub struct BleFragment {
    peer: String,
    data: Vec<u8>,
}

#[wasm_bindgen]
impl BleFragment {
    /// The address whose link it goes on.
    #[wasm_bindgen(getter)]
    pub fn peer(&self) -> String {
        self.peer.clone()
    }

    /// The fragment, written as one value.
    #[wasm_bindgen(getter)]
    pub fn data(&self) -> Vec<u8> {
        self.data.clone()
    }
}

/// This application's App tag (8 bytes), which a central compares when a
/// peripheral presents one instance of the service per SDK application.
#[wasm_bindgen(js_name = bleAppTag)]
pub fn ble_app_tag(app_id: &str) -> Vec<u8> {
    crate::ble_host::app_tag(app_id).to_vec()
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

    /// A stopped engine with the slots the application turned on. `nostr`
    /// adds the Nostr slot (W12), which the worker drives over relays the
    /// application named: `coldContact` publishes key packages and resolves
    /// peers', `usernameDiscovery` publishes `profile` as a username and
    /// resolves others'. `ble` adds the Bluetooth LE slot (W14), driven from
    /// the page over Web Bluetooth.
    #[wasm_bindgen(js_name = withOptions)]
    pub fn with_options(
        app_id: &str,
        profile: &str,
        nostr: bool,
        cold_contact: bool,
        username_discovery: bool,
        ble: bool,
    ) -> Result<WebEngine, JsError> {
        let options = NodeOptions {
            nostr: nostr.then_some(NostrOptions {
                cold_contact,
                username_discovery,
            }),
            ble,
        };
        Ok(Self {
            node: Node::with_options(app_id, profile, options).map_err(js)?,
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

    /// Removes a member from a group this device administers.
    #[wasm_bindgen(js_name = removeFromGroup)]
    pub fn remove_from_group(&mut self, group_id: &str, member: &str) -> Result<(), JsError> {
        self.node.remove_from_group(group_id, member).map_err(js)
    }

    /// A group's members and epoch as JSON, or nothing for a group this
    /// device does not hold.
    #[wasm_bindgen(js_name = groupInfo)]
    pub fn group_info(&self, group_id: &str) -> Result<Option<String>, JsError> {
        self.node.group_info(group_id).map_err(js)
    }

    /// Starts a session with a peer: "created" when this call sent the
    /// Welcome, "existing" when one already exists, "pending" while the
    /// peer's key package has not arrived (a node with Nostr has asked the
    /// relays for it).
    #[wasm_bindgen(js_name = establishSecureSession)]
    pub fn establish_secure_session(&mut self, peer: &str) -> Result<String, JsError> {
        self.node
            .establish_secure_session(peer)
            .map(|start| start.as_str().to_string())
            .map_err(js)
    }

    /// Resolves a username; exactly one `username_resolved` event follows.
    #[wasm_bindgen(js_name = resolveUsername)]
    pub fn resolve_username(&mut self, username: &str) -> Result<bool, JsError> {
        self.node.resolve_username(username).map_err(js)
    }

    /// An invite for this device, base64url, optionally with a petname and
    /// a signature binding it.
    #[wasm_bindgen(js_name = createInvite)]
    pub fn create_invite(&self, petname: Option<String>, sign: bool) -> Result<String, JsError> {
        self.node
            .create_invite(petname.as_deref(), sign)
            .map_err(js)
    }

    /// The `REQ` text that receives this device's messages (W12). Sent on
    /// every relay connect.
    #[wasm_bindgen(js_name = nostrSubscription)]
    pub fn nostr_subscription(&self, subscription_id: &str) -> Result<String, JsError> {
        self.node.nostr_subscription(subscription_id).map_err(js)
    }

    /// The worker has a live relay, or none.
    #[wasm_bindgen(js_name = nostrStatusChanged)]
    pub fn nostr_status_changed(&mut self, live: bool) -> Result<(), JsError> {
        self.node.nostr_status_changed(live).map_err(js)
    }

    /// The next event to publish as `{messageId, eventId, eventJson}` JSON,
    /// or `undefined`. Nothing while no relay is live.
    #[wasm_bindgen(js_name = nostrNextEvent)]
    pub fn nostr_next_event(&mut self) -> Option<String> {
        self.node.nostr_next_event().map(|event| {
            serde_json::json!({
                "messageId": event.message_id,
                "eventId": event.event_id,
                "eventJson": event.event_json,
            })
            .to_string()
        })
    }

    /// A relay accepted the event carrying `messageId`.
    #[wasm_bindgen(js_name = nostrConfirmSent)]
    pub fn nostr_confirm_sent(&mut self, message_id: &str) {
        self.node.nostr_confirm_sent(message_id);
    }

    /// No relay took the event carrying `messageId`.
    #[wasm_bindgen(js_name = nostrSendFailed)]
    pub fn nostr_send_failed(&mut self, message_id: &str, reason: Option<String>) {
        self.node.nostr_send_failed(message_id, reason);
    }

    /// The next resolution query as `{queryId, reqJson}` JSON, or
    /// `undefined`. Sent to every live relay under `queryId`.
    #[wasm_bindgen(js_name = nostrNextQuery)]
    pub fn nostr_next_query(&mut self) -> Option<String> {
        self.node.nostr_next_query().map(|query| {
            serde_json::json!({ "queryId": query.query_id, "reqJson": query.req_json }).to_string()
        })
    }

    /// Every relay the query went to has answered, or the worker stopped
    /// waiting.
    #[wasm_bindgen(js_name = nostrQueryCompleted)]
    pub fn nostr_query_completed(&mut self, query_id: &str) {
        self.node.nostr_query_completed(query_id);
    }

    /// One `EVENT` message a relay sent, whole.
    #[wasm_bindgen(js_name = nostrRelayEvent)]
    pub fn nostr_relay_event(&mut self, relay_message: &str) -> Result<(), JsError> {
        self.node.nostr_relay_event(relay_message).map_err(js)
    }

    /// This device's identity assertion, which the page writes to a
    /// peripheral's Hello characteristic (W14).
    #[wasm_bindgen(js_name = bleHello)]
    pub fn ble_hello(&self) -> Result<Vec<u8>, JsError> {
        self.node.ble_hello().map_err(js)
    }

    /// Checks a peripheral's Device id and Identity values; returns the
    /// address the identity proves, which must equal the Device id.
    #[wasm_bindgen(js_name = bleVerify)]
    pub fn ble_verify(&mut self, device_id: &[u8], identity: &[u8]) -> Result<String, JsError> {
        self.node.ble_verify(device_id, identity).map_err(js)
    }

    /// A verified peripheral's link is subscribed and has said hello; its
    /// write payload is `maxPayload` bytes.
    #[wasm_bindgen(js_name = bleLinkUp)]
    pub fn ble_link_up(&mut self, peer: &str, max_payload: u32) -> Result<(), JsError> {
        self.node.ble_link_up(peer, max_payload).map_err(js)
    }

    /// One fragment a peripheral notified on `peer`'s link.
    #[wasm_bindgen(js_name = bleFragmentReceived)]
    pub fn ble_fragment_received(&mut self, peer: &str, fragment: &[u8]) -> Result<(), JsError> {
        self.node.ble_fragment_received(peer, fragment).map_err(js)
    }

    /// The next fragment to write, or `undefined`.
    #[wasm_bindgen(js_name = bleNextFragment)]
    pub fn ble_next_fragment(&mut self) -> Option<BleFragment> {
        self.node
            .ble_next_fragment()
            .map(|(peer, data)| BleFragment { peer, data })
    }

    /// The link to `peer` is gone.
    #[wasm_bindgen(js_name = bleLinkDown)]
    pub fn ble_link_down(&mut self, peer: &str) {
        self.node.ble_link_down(peer);
    }

    // ------------------------------------------------------------------
    // Services and subscriptions (ADR 0035)
    // ------------------------------------------------------------------

    /// Registers a service; `capabilities_json` is a JSON object of strings.
    #[wasm_bindgen(js_name = registerService)]
    pub fn register_service(
        &mut self,
        service_id: &str,
        version: &str,
        capabilities_json: &str,
    ) -> Result<(), JsError> {
        self.node
            .register_service(service_id, version, capabilities_json)
            .map_err(js)
    }

    /// Unregisters a service; `false` when it was not registered.
    #[wasm_bindgen(js_name = unregisterService)]
    pub fn unregister_service(&mut self, service_id: &str) -> Result<bool, JsError> {
        self.node.unregister_service(service_id).map_err(js)
    }

    /// Subscribes to a provider's topic; returns the subscription id.
    #[wasm_bindgen(js_name = subscribeService)]
    pub fn subscribe_service(
        &mut self,
        provider: &str,
        service_id: &str,
        topic: &str,
        mode: &str,
        params: &str,
    ) -> Result<String, JsError> {
        self.node
            .subscribe_service(provider, service_id, topic, mode, params)
            .map_err(js)
    }

    /// Ends a subscription this device holds.
    #[wasm_bindgen(js_name = unsubscribeService)]
    pub fn unsubscribe_service(&mut self, sub_id: &str) -> Result<bool, JsError> {
        self.node.unsubscribe_service(sub_id).map_err(js)
    }

    /// Asks a subscription's provider for the whole state again.
    #[wasm_bindgen(js_name = resyncServiceSubscription)]
    pub fn resync_service_subscription(&mut self, sub_id: &str) -> Result<(), JsError> {
        self.node.resync_service_subscription(sub_id).map_err(js)
    }

    /// The subscriptions this device holds, as JSON.
    #[wasm_bindgen(js_name = serviceSubscriptions)]
    pub fn service_subscriptions(&self) -> Result<String, JsError> {
        self.node.service_subscriptions().map_err(js)
    }

    /// Sets a service's admission policy from JSON.
    #[wasm_bindgen(js_name = setServiceSubscriptionPolicy)]
    pub fn set_service_subscription_policy(
        &mut self,
        service_id: &str,
        policy_json: &str,
    ) -> Result<(), JsError> {
        self.node
            .set_service_subscription_policy(service_id, policy_json)
            .map_err(js)
    }

    /// Accepts a subscriber the application was asked about.
    #[wasm_bindgen(js_name = acceptServiceSubscription)]
    pub fn accept_service_subscription(&mut self, sub_id: &str) -> Result<(), JsError> {
        self.node.accept_service_subscription(sub_id).map_err(js)
    }

    /// Rejects a subscriber the application was asked about.
    #[wasm_bindgen(js_name = rejectServiceSubscription)]
    pub fn reject_service_subscription(
        &mut self,
        sub_id: &str,
        reason: &str,
    ) -> Result<(), JsError> {
        self.node
            .reject_service_subscription(sub_id, reason)
            .map_err(js)
    }

    /// Ends a subscription another device holds on this one.
    #[wasm_bindgen(js_name = endServiceSubscription)]
    pub fn end_service_subscription(&mut self, sub_id: &str, reason: &str) -> Result<(), JsError> {
        self.node
            .end_service_subscription(sub_id, reason)
            .map_err(js)
    }

    /// Publishes to a topic; returns the report as JSON.
    #[wasm_bindgen(js_name = publishServiceUpdate)]
    pub fn publish_service_update(
        &mut self,
        service_id: &str,
        topic: &str,
        key: Option<String>,
        body: &str,
    ) -> Result<String, JsError> {
        self.node
            .publish_service_update(service_id, topic, key, body)
            .map_err(js)
    }

    /// Sets a `log` topic's checkpoint.
    #[wasm_bindgen(js_name = setServiceTopicCheckpoint)]
    pub fn set_service_topic_checkpoint(
        &mut self,
        service_id: &str,
        topic: &str,
        body: &str,
    ) -> Result<(), JsError> {
        self.node
            .set_service_topic_checkpoint(service_id, topic, body)
            .map_err(js)
    }

    /// Starts a topic over.
    #[wasm_bindgen(js_name = clearServiceTopic)]
    pub fn clear_service_topic(&mut self, service_id: &str, topic: &str) -> Result<(), JsError> {
        self.node.clear_service_topic(service_id, topic).map_err(js)
    }

    /// The subscriptions other devices hold here, as JSON.
    #[wasm_bindgen(js_name = serviceSubscribers)]
    pub fn service_subscribers(&self, service_id: Option<String>) -> Result<String, JsError> {
        self.node.service_subscribers(service_id).map_err(js)
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
