// Links between nodes in other tabs and workers of this origin, over one
// BroadcastChannel.
//
// A BroadcastChannel reaches every context of the origin, so this file makes
// point-to-point carriers out of it: every message names its sender and its
// recipient by a random instance id, and each remote instance this node hears
// from gets exactly one link. The instance id is a carrier address and proves
// nothing; who is on the other end is what the engine's preamble proves.
//
// A tab that closes cannot always say so (a crash, a killed process). A node
// that stops hearing from an instance for LOSS_MS reports its link closed.

const HEARTBEAT_MS = 2000;
const LOSS_MS = 7000;

export class TabCarrier {
  constructor(appId, engine) {
    this.engine = engine;
    this.self = crypto.randomUUID();
    this.links = new Map(); // remote instance -> { link, heard }
    this.instances = new Map(); // link -> remote instance
    // Instances whose link the engine refused (a preamble that did not
    // verify, or an address already announced on another link). Ignored
    // until they say goodbye, so a heartbeat does not re-link them.
    this.refused = new Set();
    this.channel = new BroadcastChannel(`offline-protocol/${appId}`);
    this.channel.onmessage = ({ data }) => this.#receive(data);
    this.#say({ type: "hello" });
    this.heartbeat = setInterval(() => this.#beat(), HEARTBEAT_MS);
  }

  send(link, bytes) {
    const to = this.instances.get(link);
    if (to) this.#say({ type: "data", to, bytes });
  }

  close() {
    clearInterval(this.heartbeat);
    this.#say({ type: "bye" });
    this.channel.close();
  }

  #say(message) {
    this.channel.postMessage({ ...message, from: this.self });
  }

  #beat() {
    this.#say({ type: "hello" });
    const now = Date.now();
    for (const [instance, entry] of this.links) {
      if (now - entry.heard > LOSS_MS) this.#drop(instance);
    }
  }

  #linkFor(instance) {
    let entry = this.links.get(instance);
    if (!entry) {
      entry = { link: this.engine.open(), heard: Date.now() };
      this.links.set(instance, entry);
      this.instances.set(entry.link, instance);
      // Answer at once, so the other side learns of this instance without
      // waiting for a heartbeat.
      this.#say({ type: "hello" });
    }
    entry.heard = Date.now();
    return entry.link;
  }

  #drop(instance) {
    const entry = this.links.get(instance);
    if (!entry) return;
    this.links.delete(instance);
    this.instances.delete(entry.link);
    this.engine.closed(entry.link);
  }

  #receive(message) {
    const { type, from, to } = message ?? {};
    if (!from || from === this.self) return;
    if (to !== undefined && to !== this.self) return;
    if (type === "bye") {
      this.refused.delete(from);
      return this.#drop(from);
    }
    if (this.refused.has(from)) return;
    const link = this.#linkFor(from);
    if (type !== "data") return;
    const reason = this.engine.received(link, new Uint8Array(message.bytes));
    if (reason !== undefined) {
      // The engine has already closed the link and delivers nothing more on
      // it; forget the instance's link and stop listening to it.
      this.links.delete(from);
      this.instances.delete(link);
      this.refused.add(from);
    }
  }
}
