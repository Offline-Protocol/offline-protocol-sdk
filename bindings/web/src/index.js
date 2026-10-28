// The page's handle on a node running in its own worker.

export { WebRtcCarrier } from "./webrtc.js";
export { NostrSignaller } from "./nostr-signal.js";
export { NearbyRoom, publicAddress } from "./nearby.js";
export { BluetoothCarrier } from "./bluetooth.js";
import { checkRelays } from "./nostr-relays.js";

export class OfflineNode extends EventTarget {
  #worker;
  #next = 1;
  #pending = new Map();
  #failed = null;
  #appId;
  #instance;
  #tabs;
  // link -> { send(bytes), close() }: the page's carriers.
  #links = new Map();
  // { write(peer, bytes), close() }: where Bluetooth fragments go (W14).
  #bleSink = null;
  #onPageHide = () => this.#worker.postMessage({ id: 0, op: "stop" });
  // A page restored from the back-forward cache had its node stopped on
  // pagehide; say so once rather than leave a node that answers nothing.
  #onPageShow = (event) => {
    if (event.persisted) this.#fail("the page was restored after its node stopped");
  };

  constructor(worker, { appId, instance, tabs }) {
    super();
    this.#worker = worker;
    this.#appId = appId;
    this.#instance = instance;
    this.#tabs = tabs;
    this.#worker.onmessage = ({ data }) => this.#receive(data);
    // A worker that fails to load or throws outside a call ends here. Without
    // it every call in flight would wait for a reply that never comes.
    this.#worker.onerror = (event) => {
      event.preventDefault?.();
      this.#fail(event.message || "the node's worker failed");
    };
    this.#worker.onmessageerror = () => this.#fail("the node's worker sent an unreadable message");
    this.address = null;
  }

  /// Starts a node in a new module worker and resolves once it has an address.
  ///
  /// `storage` is "durable" (the default: the identity and groups outlive the
  /// worker, in sealed origin-private files) or "memory" (a new device every
  /// start). `tabs: false` leaves out the carrier between this origin's tabs,
  /// for a node that links only through carriers the page attaches.
  ///
  /// `nostr: { relays, coldContact, usernameDiscovery }` adds the Nostr
  /// transport over the relays the application names (W12). The SDK names
  /// none, so `relays` is required. `coldContact` (default on) publishes key
  /// packages so a device that knows only this node's address can reach it;
  /// `usernameDiscovery` (default off) publishes `profile` as a username.
  ///
  /// `ble: true` adds the Bluetooth LE slot, which a BluetoothCarrier on this
  /// page drives over Web Bluetooth (W14): Chromium only, as a central.
  ///
  /// `signal` aborts a start that has not finished: its worker ends, and with
  /// it the account's lock or its place in the queue for one (W8), and the
  /// start rejects with the signal's reason. A start left to finish would
  /// hold the account at least until it had replayed the logs.
  static async start({
    appId,
    profile = "default",
    storage = "durable",
    tabs = true,
    nostr,
    ble = false,
    workerUrl,
    wasm,
    signal,
  } = {}) {
    if (!appId) throw new Error("appId is required");
    signal?.throwIfAborted();
    if (nostr !== undefined) checkRelays(nostr?.relays);
    if (storage === "durable") {
      // Best effort: without it the browser may evict the origin's files
      // under storage pressure, identity included. Never awaited: Firefox
      // answers by asking the user, and the promise stays pending until they
      // do, so a node that waited for it would not start while the prompt
      // is ignored (or ever, in a headless browser).
      navigator.storage?.persist?.().catch(() => false);
    }
    const url = workerUrl ?? new URL("./worker.js", import.meta.url);
    const instance = crypto.randomUUID();
    const node = new OfflineNode(new Worker(url, { type: "module" }), { appId, instance, tabs });
    const abort = () => node.#fail("the start was aborted");
    signal?.addEventListener("abort", abort, { once: true });
    try {
      node.address = await node.#call("start", {
        appId,
        profile,
        storage,
        tabs,
        instance,
        wasm,
        ble: Boolean(ble),
        // The worker applies the defaults; it is the one place that does.
        nostr:
          nostr === undefined
            ? undefined
            : {
                relays: [...nostr.relays],
                coldContact: nostr.coldContact,
                usernameDiscovery: nostr.usernameDiscovery,
              },
      });
    } catch (error) {
      // The worker kept nothing (no engine, lock or handle), but it is still
      // a running worker with a module in it.
      node.#worker.terminate();
      throw signal?.aborted ? signal.reason : error;
    } finally {
      signal?.removeEventListener("abort", abort);
    }
    addEventListener("pagehide", node.#onPageHide);
    addEventListener("pageshow", node.#onPageShow);
    return node;
  }

  /// `ephemeral: true` sends once over the live link and never retries:
  /// for state the next send replaces, such as a game's positions. It fails
  /// at once with no link or no session (docs/bridges/web.md, W10).
  sendMessage(to, content, { ephemeral = false } = {}) {
    return this.#call("sendMessage", { to, content, ephemeral });
  }

  createGroup(name) {
    return this.#call("createGroup", { name });
  }

  inviteToGroup(groupId, member) {
    return this.#call("inviteToGroup", { groupId, member });
  }

  /// Removes a member from a group this device administers. The others move
  /// to a new epoch the removed member holds no keys for.
  removeFromGroup(groupId, member) {
    return this.#call("removeFromGroup", { groupId, member });
  }

  /// The group's members and epoch, or null for a group this device does
  /// not hold.
  groupInfo(groupId) {
    return this.#call("groupInfo", { groupId });
  }

  /// `ephemeral` as for sendMessage, to every member holding a live link.
  sendGroupMessage(groupId, content, { ephemeral = false } = {}) {
    return this.#call("sendGroupMessage", { groupId, content, ephemeral });
  }

  // Services and subscriptions (ADR 0035). Every subscription frame is
  // sealed in the 1:1 session, with no plaintext fallback. Updates arrive as
  // `service_update_received`, state changes as
  // `service_subscription_changed`, and a subscriber waiting for this page's
  // decision as `service_subscribe_received`.

  registerService(serviceId, { version = "", capabilities = {} } = {}) {
    return this.#call("registerService", { serviceId, version, capabilities });
  }

  unregisterService(serviceId) {
    return this.#call("unregisterService", { serviceId });
  }

  /// `mode` is "latest" (state, one current value per key) or "log" (every
  /// event, in order). `topic` may be a pattern, "prefix*" or "*": every
  /// matching topic in the mode, each update naming its own. `keys` limits a
  /// "latest" subscription to those keys (exact, or a prefix ending in "*").
  /// Resolves to the subscription id its events carry.
  subscribeService(provider, serviceId, topic, { mode = "latest", params = "", keys = [] } = {}) {
    return this.#call("subscribeService", { provider, serviceId, topic, mode, params, keys });
  }

  unsubscribeService(subId) {
    return this.#call("unsubscribeService", { subId });
  }

  /// Asks the provider for the whole state again, now.
  resyncServiceSubscription(subId) {
    return this.#call("resyncServiceSubscription", { subId });
  }

  serviceSubscriptions() {
    return this.#call("serviceSubscriptions");
  }

  /// `{ kind: "restricted" }` (the default: this page is asked),
  /// `{ kind: "accept_all" }`, or `{ kind: "rules", allowPeers, allowGroups,
  /// fallback: "ask" | "reject" }`.
  setServiceSubscriptionPolicy(serviceId, policy) {
    return this.#call("setServiceSubscriptionPolicy", { serviceId, policy });
  }

  acceptServiceSubscription(subId) {
    return this.#call("acceptServiceSubscription", { subId });
  }

  rejectServiceSubscription(subId, reason = "") {
    return this.#call("rejectServiceSubscription", { subId, reason });
  }

  endServiceSubscription(subId, reason = "") {
    return this.#call("endServiceSubscription", { subId, reason });
  }

  /// With a `key` the topic is `latest`; without one it is `log`.
  publishServiceUpdate(serviceId, topic, body, { key = null } = {}) {
    return this.#call("publishServiceUpdate", { serviceId, topic, key, body });
  }

  setServiceTopicCheckpoint(serviceId, topic, body) {
    return this.#call("setServiceTopicCheckpoint", { serviceId, topic, body });
  }

  clearServiceTopic(serviceId, topic) {
    return this.#call("clearServiceTopic", { serviceId, topic });
  }

  serviceSubscribers(serviceId = null) {
    return this.#call("serviceSubscribers", { serviceId });
  }

  /// Starts a session with `address`: "created" (the Welcome went out now),
  /// "existing", or "pending" while its key package has not arrived. On a
  /// node with relays, the engine has asked them for the packages the device
  /// published, and `secure_session_established` follows. A message sent
  /// meanwhile waits for the session, so calling this first is optional.
  establishSecureSession(address) {
    return this.#call("establishSecureSession", { address });
  }

  /// Looks a username up in the directory the relays hold (W12). Resolves
  /// once the lookup is under way; the answer is one `username_resolved`
  /// event carrying every device that claims the name. A name is a claim,
  /// not an identity: show the person the whole set, and keep the address
  /// they choose.
  resolveUsername(username) {
    return this.#call("resolveUsername", { username });
  }

  /// An invite naming this device (base64url): its address and key, and an
  /// optional petname that `sign: true` binds to the key.
  createInvite({ petname, sign = false } = {}) {
    return this.#call("createInvite", { petname, sign });
  }

  /// Opens an invite: `{ address, publicKey, petname, signed }`, the address
  /// checked to derive from the key. Rejects an invite that does not verify.
  parseInvite(blob) {
    return this.#call("parseInvite", { blob });
  }

  /// A fresh room code for NostrSignaller: eighty bits a person can read out.
  newRoom() {
    return this.#call("signalNewRoom");
  }

  /// NostrSignaller's half in the worker (W11): the relay REQ text for a
  /// room, a sealed and signed EVENT text, and the signal a relay message
  /// carries (`{ role, session, code, freshUntilMs }`, where `freshUntilMs` is
  /// when the offer stops being fresh, or null for anything else).
  signalSubscription(room, subscriptionId) {
    return this.#call("signalSubscription", { room, subscriptionId });
  }

  signalEvent(room, role, session, code) {
    return this.#call("signalEvent", { room, role, session, code });
  }

  signalOpen(room, message) {
    return this.#call("signalOpen", { room, message });
  }

  /// NearbyRoom's half in the worker (W13): open a room for this network's
  /// address under a label (`{ id, key }`), its REQ text, sealed EVENT texts,
  /// and what a relay message carries (`{ kind, peer, label, session, code,
  /// createdAtMs, freshUntilMs }`, or null).
  nearbyOpen(network, label) {
    return this.#call("nearbyOpen", { network, label });
  }

  nearbySubscription(id, subscriptionId) {
    return this.#call("nearbySubscription", { id, subscriptionId });
  }

  nearbyPresence(id) {
    return this.#call("nearbyPresence", { id });
  }

  nearbyOffer(id, to, session, code) {
    return this.#call("nearbyOffer", { id, to, session, code });
  }

  nearbyAnswer(id, to, session, code) {
    return this.#call("nearbyAnswer", { id, to, session, code });
  }

  nearbyOpenMessage(id, message) {
    return this.#call("nearbyOpenMessage", { id, message });
  }

  nearbyClose(id) {
    return this.#call("nearbyClose", { id });
  }

  /// The Bluetooth slot's half in the worker (W14), for BluetoothCarrier:
  /// this device's hello, this application's App tag, a peripheral's Device
  /// id and Identity checked (resolving to the address proved), a verified
  /// link announced with its write payload, a notified fragment, a link gone.
  bleHello() {
    return this.#call("bleHello");
  }

  bleAppTag() {
    return this.#call("bleAppTag");
  }

  bleVerify(deviceId, identity) {
    return this.#call("bleVerify", { deviceId, identity });
  }

  bleLinkUp(peer, maxPayload) {
    return this.#call("bleLinkUp", { peer, maxPayload });
  }

  bleFragmentReceived(peer, bytes) {
    return this.#call("bleFragmentReceived", { peer, bytes });
  }

  bleLinkDown(peer) {
    return this.#call("bleLinkDown", { peer }).catch(() => {});
  }

  /// Where the engine's outbound Bluetooth fragments go:
  /// `{ write(peer, bytes), close() }`, `close` called once when the node
  /// stops or fails. One sink per node, which BluetoothCarrier takes:
  /// replacing a sink another carrier holds throws, since that carrier's
  /// links would go deaf with no error. `null` gives it back.
  setBluetoothSink(sink) {
    if (sink !== null && this.#bleSink !== null) {
      throw new Error("another BluetoothCarrier holds this node's Bluetooth; release() it first");
    }
    this.#bleSink = sink;
  }

  /// What the durable store has cost since the node started (flushes, log
  /// and live bytes, compactions per log), or null for a node in memory.
  stats() {
    return this.#call("stats");
  }

  /// Registers a carrier the page opened toward one other node. `send` puts
  /// bytes on it, in order; `close` closes it. Resolves to the link id the
  /// carrier reports its bytes and its close under.
  async openLink({ send, close }) {
    const link = await this.#call("openLink");
    this.#links.set(link, { send, close });
    return link;
  }

  /// Bytes a page carrier delivered, in order. Resolves to the reason the
  /// engine closed the link, or undefined; on a reason the carrier has
  /// already been closed.
  async linkReceived(link, bytes) {
    const reason = await this.#call("linkReceived", { link, bytes });
    if (reason !== undefined && reason !== null) this.#closeCarrier(link);
    return reason ?? undefined;
  }

  /// A page carrier closed.
  closeLink(link) {
    this.#links.delete(link);
    return this.#call("closeLink", { link }).catch(() => {});
  }

  /// Stops the engine and ends its worker. Every later call rejects: the
  /// worker is gone, so a call that waited for its reply would wait for ever.
  /// No `fatal` event, as nothing failed.
  async stop() {
    removeEventListener("pagehide", this.#onPageHide);
    removeEventListener("pageshow", this.#onPageShow);
    try {
      await this.#call("stop");
    } finally {
      this.#failed ??= new Error("the node was stopped");
      this.#worker.terminate();
      this.#closeAllCarriers();
    }
  }

  /// Ends the worker at once, without stopping the engine: what a crash or a
  /// killed tab does. Everything the engine had acknowledged is durable, so
  /// starting a node again with the same `appId` and `profile` is the same
  /// device. The tab carrier's other ends are told now, as the worker can no
  /// longer tell them.
  terminate() {
    this.#fail("the node was terminated");
  }

  /// Ends the node: rejects every call in flight and every later one, and
  /// dispatches one `fatal` event. The engine traps on a panic (W5); the
  /// recovery is to start the node again, which a durable store makes the
  /// same device.
  #fail(reason) {
    if (this.#failed) return;
    this.#failed = new Error(reason);
    removeEventListener("pagehide", this.#onPageHide);
    removeEventListener("pageshow", this.#onPageShow);
    for (const { reject } of this.#pending.values()) reject(this.#failed);
    this.#pending.clear();
    this.#worker.terminate();
    this.#sayByeForWorker();
    this.#closeAllCarriers();
    this.dispatchEvent(new CustomEvent("fatal", { detail: reason }));
  }

  // The tab carrier's `bye`, sent from the worker's carrier address.
  #sayByeForWorker() {
    if (!this.#tabs) return;
    try {
      const channel = new BroadcastChannel(`offline-protocol/${this.#appId}`);
      channel.postMessage({ type: "bye", from: this.#instance });
      channel.close();
    } catch {
      // Without a channel the other ends wait out their silence instead.
    }
  }

  #closeCarrier(link) {
    const carrier = this.#links.get(link);
    this.#links.delete(link);
    try {
      carrier?.close();
    } catch {
      // A carrier that throws on close is closed.
    }
  }

  #closeAllCarriers() {
    for (const link of [...this.#links.keys()]) this.#closeCarrier(link);
    const ble = this.#bleSink;
    this.#bleSink = null;
    try {
      ble?.close();
    } catch {
      // A carrier that throws on close is closed.
    }
  }

  #call(op, args) {
    if (this.#failed) return Promise.reject(this.#failed);
    const id = this.#next++;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      this.#worker.postMessage({ id, op, args });
    });
  }

  #receive(message) {
    if (message.kind === "reply") {
      const pending = this.#pending.get(message.id);
      this.#pending.delete(message.id);
      if (message.error !== undefined) pending?.reject(new Error(message.error));
      else pending?.resolve(message.result);
    } else if (message.kind === "bleFragment") {
      this.#bleSink?.write(message.peer, new Uint8Array(message.data));
    } else if (message.kind === "frame") {
      this.#links.get(message.link)?.send(new Uint8Array(message.data));
    } else if (message.kind === "linkClosed") {
      // The engine closed a page link on its own (a preamble that never came).
      this.#closeCarrier(message.link);
    } else if (message.kind === "event") {
      // One DOM event per engine event, named by its `type`
      // (message_received, neighbor_discovered, ...), with the engine's JSON
      // as `detail`.
      this.dispatchEvent(new CustomEvent(message.event.type, { detail: message.event }));
      this.dispatchEvent(new CustomEvent("event", { detail: message.event }));
    } else if (message.kind === "error") {
      this.dispatchEvent(new CustomEvent("error", { detail: message.message }));
    } else if (message.kind === "fatal") {
      this.#fail(message.message);
    }
  }
}
