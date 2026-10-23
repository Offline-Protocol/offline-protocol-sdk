// Links between nodes in other tabs and workers of this origin, over one
// BroadcastChannel.
//
// A BroadcastChannel reaches every context of the origin, so this file makes
// point-to-point carriers out of it: every message names its sender and its
// recipient by a random instance id, and each remote instance this node hears
// from gets exactly one link. The instance id is a carrier address and proves
// nothing; who is on the other end is what the engine's preamble proves.
//
// A link ends in one of three ways, and the other end always learns of it:
// - "bye": this node is stopping. Every link to it closes.
// - "close": the engine refused or timed out the link (a preamble that did
//   not verify, an address already announced on another link, or no preamble
//   at all). The engine has already closed its side; the other end closes its
//   side, so it stops reporting a peer this node will never hear. This is the
//   stream-framing chapter's rule that the close is the refused side's signal.
// - silence for LOSS_MS: a tab that crashed or was killed cannot say bye.
//
// After a close, both ends ignore each other for REFUSAL_MS, so a heartbeat
// does not re-open the link at once, and then try again: what caused the
// refusal (a second tab holding the same identity, say) may have gone.

const HEARTBEAT_MS = 2000;
const LOSS_MS = 7000;
const REFUSAL_MS = 30000;
// Instances held as refused, at most. Every one expires after REFUSAL_MS, so
// this bounds only a burst of fresh instance ids.
const MAX_REFUSED = 256;

export class TabCarrier {
  constructor(appId, engine) {
    this.engine = engine;
    this.self = crypto.randomUUID();
    this.links = new Map(); // remote instance -> { link, heard }
    this.instances = new Map(); // link -> remote instance
    this.refused = new Map(); // remote instance -> ignored until (ms)
    this.channel = new BroadcastChannel(`offline-protocol/${appId}`);
    this.channel.onmessage = ({ data }) => this.#receive(data);
    this.#say({ type: "hello" });
    this.heartbeat = setInterval(() => this.#beat(), HEARTBEAT_MS);
  }

  send(link, bytes) {
    const to = this.instances.get(link);
    if (to) this.#say({ type: "data", to, bytes });
  }

  /// The engine closed `link` on its own (WebEngine.nextClosed). Its side is
  /// already gone; close the carrier and tell the other end.
  engineClosed(link) {
    const instance = this.instances.get(link);
    if (instance !== undefined) this.#refuse(instance);
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
    for (const [instance, until] of this.refused) {
      if (now >= until) this.refused.delete(instance);
    }
  }

  #linkFor(instance) {
    let entry = this.links.get(instance);
    if (!entry) {
      let link;
      try {
        link = this.engine.open();
      } catch {
        // The engine's link ceiling. Ignore the instance for now; its next
        // heartbeat asks again, by which time a link may have closed.
        return undefined;
      }
      entry = { link, heard: Date.now() };
      this.links.set(instance, entry);
      this.instances.set(entry.link, instance);
      // Answer at once, so the other side learns of this instance without
      // waiting for a heartbeat.
      this.#say({ type: "hello" });
    }
    entry.heard = Date.now();
    return entry.link;
  }

  // The other end is gone or has closed its side: close ours in the engine.
  #drop(instance) {
    const entry = this.links.get(instance);
    if (!entry) return;
    this.links.delete(instance);
    this.instances.delete(entry.link);
    this.engine.closed(entry.link);
  }

  // The engine closed our side: forget the link, tell the other end, and
  // ignore it for a while.
  #refuse(instance) {
    const entry = this.links.get(instance);
    if (entry) {
      this.links.delete(instance);
      this.instances.delete(entry.link);
    }
    this.#say({ type: "close", to: instance });
    this.#hold(instance);
  }

  #hold(instance) {
    if (!this.refused.has(instance) && this.refused.size >= MAX_REFUSED) {
      this.refused.delete(this.refused.keys().next().value);
    }
    this.refused.set(instance, Date.now() + REFUSAL_MS);
  }

  #receive(message) {
    const { type, from, to } = message ?? {};
    if (!from || from === this.self) return;
    if (to !== undefined && to !== this.self) return;
    if (type === "bye") {
      this.refused.delete(from);
      return this.#drop(from);
    }
    if (type === "close") {
      this.#drop(from);
      return this.#hold(from);
    }
    const until = this.refused.get(from);
    if (until !== undefined) {
      if (Date.now() < until) return;
      this.refused.delete(from);
    }
    const link = this.#linkFor(from);
    if (link === undefined || type !== "data") return;
    const reason = this.engine.received(link, new Uint8Array(message.bytes));
    // The engine has already closed the link and delivers nothing more on it.
    if (reason !== undefined) this.#refuse(from);
  }
}
