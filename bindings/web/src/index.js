// The page's handle on a node running in its own worker.

export { WebRtcCarrier } from "./webrtc.js";

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
  static async start({
    appId,
    profile = "default",
    storage = "durable",
    tabs = true,
    workerUrl,
  } = {}) {
    if (!appId) throw new Error("appId is required");
    if (storage === "durable") {
      // Best effort: without it the browser may evict the origin's files
      // under storage pressure, identity included.
      await navigator.storage?.persist?.().catch(() => false);
    }
    const url = workerUrl ?? new URL("./worker.js", import.meta.url);
    const instance = crypto.randomUUID();
    const node = new OfflineNode(new Worker(url, { type: "module" }), { appId, instance, tabs });
    try {
      node.address = await node.#call("start", { appId, profile, storage, tabs, instance });
    } catch (error) {
      // The worker kept nothing (no engine, lock or handle), but it is still
      // a running worker with a module in it.
      node.#worker.terminate();
      throw error;
    }
    addEventListener("pagehide", node.#onPageHide);
    addEventListener("pageshow", node.#onPageShow);
    return node;
  }

  sendMessage(to, content) {
    return this.#call("sendMessage", { to, content });
  }

  createGroup(name) {
    return this.#call("createGroup", { name });
  }

  inviteToGroup(groupId, member) {
    return this.#call("inviteToGroup", { groupId, member });
  }

  sendGroupMessage(groupId, content) {
    return this.#call("sendGroupMessage", { groupId, content });
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

  async stop() {
    removeEventListener("pagehide", this.#onPageHide);
    removeEventListener("pageshow", this.#onPageShow);
    await this.#call("stop");
    this.#worker.terminate();
    this.#closeAllCarriers();
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
