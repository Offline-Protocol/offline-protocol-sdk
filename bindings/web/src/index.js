// The page's handle on a node running in its own worker.

export class OfflineNode extends EventTarget {
  #worker;
  #next = 1;
  #pending = new Map();
  #failed = null;

  constructor(worker) {
    super();
    this.#worker = worker;
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
  static async start({ appId, profile = "default", workerUrl } = {}) {
    if (!appId) throw new Error("appId is required");
    const url = workerUrl ?? new URL("./worker.js", import.meta.url);
    const node = new OfflineNode(new Worker(url, { type: "module" }));
    node.address = await node.#call("start", { appId, profile });
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

  async stop() {
    await this.#call("stop");
    this.#worker.terminate();
  }

  /// Ends the node: rejects every call in flight and every later one, and
  /// dispatches one `fatal` event. The engine traps on a panic (W5), and
  /// with in-memory storage the recovery is a new node, which is a new device.
  #fail(reason) {
    if (this.#failed) return;
    this.#failed = new Error(reason);
    for (const { reject } of this.#pending.values()) reject(this.#failed);
    this.#pending.clear();
    this.#worker.terminate();
    this.dispatchEvent(new CustomEvent("fatal", { detail: reason }));
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
