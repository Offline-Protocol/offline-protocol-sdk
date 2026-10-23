// The page's handle on a node running in its own worker.

export class OfflineNode extends EventTarget {
  #worker;
  #next = 1;
  #pending = new Map();

  constructor(worker) {
    super();
    this.#worker = worker;
    this.#worker.onmessage = ({ data }) => this.#receive(data);
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

  #call(op, args) {
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
    }
  }
}
