// The Nostr transport's sockets, in the worker beside the engine (W12).
//
// Invariants (docs/bridges/web.md):
// - The relays are the application's. This file is handed a list and dials
//   exactly that list; it names no relay of its own.
// - It moves text. It reads what every binding's driver reads (a message's
//   type, its subscription id, an OK's event id and verdict) and hands every
//   EVENT to the engine whole, which decodes, unseals and attributes it.
// - A relay is live once it has confirmed this device's subscription with
//   EOSE, and stays live while it keeps confirming: every KEEP_ALIVE_MS the
//   subscription is sent again under the same id (a NIP-01 replace), and a
//   relay that has not answered within PROBE_MS is dropped and redialled. A
//   socket the network lost without closing it would otherwise count as a
//   relay for ever. The engine hears "live" when the first relay is and "not
//   live" when the last one stops.
// - An event is confirmed by the first relay that accepts it and fails only
//   when every relay it went to refused it or dropped. The transport expires
//   a confirmation nobody reported, so a silent relay costs a retry, not a
//   lost message.
// - A resolution query is complete when every relay it went to has sent its
//   own EOSE (or dropped), never on the first: one fast, empty or hostile
//   relay must not decide the answer (docs/spec/username-discovery.md). It is
//   closed on each relay as that relay finishes, and completed after
//   COMPLETION_TIMEOUT_MS at the latest, below the engine's 30 s sweep, so the
//   driver that knows which relays answered is the one that ends it.

const RETRY_MIN_MS = 1000;
const RETRY_MAX_MS = 30000;
const KEEP_ALIVE_MS = 60000;
const PROBE_MS = 10000;
const COMPLETION_TIMEOUT_MS = 10_000;
// Events awaiting a verdict, at most. The transport bounds its own queue;
// this bounds the driver's copy if the relays never answer.
const MAX_PENDING_EVENTS = 1024;

function randomHex(bytes) {
  const buffer = new Uint8Array(bytes);
  crypto.getRandomValues(buffer);
  return Array.from(buffer, (b) => b.toString(16).padStart(2, "0")).join("");
}

/// Checks an application's relay list: a non-empty array of ws:// or wss://
/// URLs. Throws with the reason otherwise.
export function checkRelays(relays) {
  if (!Array.isArray(relays) || relays.length === 0) {
    throw new Error(
      "nostr.relays must list the relays to use (ws:// or wss:// URLs); the SDK names none",
    );
  }
  for (const relay of relays) {
    let url;
    try {
      url = new URL(relay);
    } catch {
      throw new Error(`nostr.relays: ${relay} is not a URL`);
    }
    if (url.protocol !== "wss:" && url.protocol !== "ws:") {
      throw new Error(`nostr.relays: ${relay} is not a ws:// or wss:// URL`);
    }
  }
  return [...new Set(relays)];
}

export class NostrRelays {
  #engine;
  #slots = [];
  #subscriptionId = `inbox-${randomHex(8)}`;
  // eventId -> { messageId, waiting: Set<url>, reason }
  #pending = new Map();
  // queryId -> { waiting: Set<url>, deadline }
  #queries = new Map();
  #keepAlive = null;
  #live = false;
  #closed = false;
  #wake = () => this.#redialNow();

  /// `engine` is what the worker hands over: `subscription(id)` returns the
  /// REQ text, `event(text)` takes a relay's EVENT, `status(live)`,
  /// `confirmed(messageId)`, `failed(messageId, reason)` and
  /// `completed(queryId)` report back.
  constructor(relays, engine) {
    this.#engine = engine;
    for (const url of checkRelays(relays)) {
      this.#slots.push({ url, socket: null, live: false, confirm: null, retry: null, backoff: RETRY_MIN_MS });
    }
  }

  get live() {
    return this.#live;
  }

  open() {
    for (const slot of this.#slots) this.#connect(slot);
    this.#keepAlive = setInterval(() => this.#probe(), KEEP_ALIVE_MS);
    self.addEventListener?.("online", this.#wake);
  }

  /// Puts one signed event on every live relay.
  publish({ messageId, eventId, eventJson }) {
    const live = this.#slots.filter((slot) => slot.live);
    if (live.length === 0) return this.#engine.failed(messageId, "no relay is live");
    if (this.#pending.size >= MAX_PENDING_EVENTS) {
      const [oldest, entry] = this.#pending.entries().next().value;
      this.#pending.delete(oldest);
      this.#engine.failed(entry.messageId, "no relay answered");
    }
    this.#pending.set(eventId, { messageId, waiting: new Set(live.map((s) => s.url)), reason: null });
    for (const slot of live) slot.socket.send(eventJson);
  }

  /// Sends one resolution query to every live relay.
  query({ queryId, reqJson }) {
    const live = this.#slots.filter((slot) => slot.live);
    if (live.length === 0) return this.#engine.completed(queryId);
    const deadline = setTimeout(() => this.#finish(queryId), COMPLETION_TIMEOUT_MS);
    this.#queries.set(queryId, { waiting: new Set(live.map((s) => s.url)), deadline });
    for (const slot of live) slot.socket.send(reqJson);
  }

  close() {
    if (this.#closed) return;
    this.#closed = true;
    clearInterval(this.#keepAlive);
    self.removeEventListener?.("online", this.#wake);
    for (const queryId of [...this.#queries.keys()]) this.#finish(queryId);
    for (const slot of this.#slots) {
      clearTimeout(slot.retry);
      if (slot.socket?.readyState === WebSocket.OPEN) {
        slot.socket.send(JSON.stringify(["CLOSE", this.#subscriptionId]));
      }
      this.#drop(slot);
    }
    this.#setLive(false);
  }

  #connect(slot) {
    if (this.#closed) return;
    slot.retry = null;
    let socket;
    try {
      socket = new WebSocket(slot.url);
    } catch {
      // A URL the browser refuses to dial is refused for good.
      return;
    }
    slot.socket = socket;
    socket.onopen = () => this.#subscribe(slot);
    socket.onerror = () => this.#lost(slot, socket);
    socket.onclose = () => this.#lost(slot, socket);
    socket.onmessage = ({ data }) => {
      if (typeof data === "string") this.#message(slot, data);
    };
  }

  #subscribe(slot) {
    let request;
    try {
      request = this.#engine.subscription(this.#subscriptionId);
    } catch {
      return this.#lost(slot, slot.socket);
    }
    slot.socket.send(request);
    this.#await(slot);
  }

  // Arms the confirmation deadline for the subscription just sent.
  #await(slot) {
    clearTimeout(slot.confirm);
    const socket = slot.socket;
    slot.confirm = setTimeout(() => {
      if (slot.socket === socket) this.#lost(slot, socket);
    }, PROBE_MS);
  }

  #probe() {
    for (const slot of this.#slots) {
      if (slot.live && slot.socket?.readyState === WebSocket.OPEN && !slot.confirm) {
        this.#subscribe(slot);
      }
    }
  }

  #redialNow() {
    for (const slot of this.#slots) {
      if (slot.retry) {
        clearTimeout(slot.retry);
        this.#connect(slot);
      }
    }
  }

  #message(slot, text) {
    let parts;
    try {
      parts = JSON.parse(text);
    } catch {
      return;
    }
    if (!Array.isArray(parts)) return;
    const [type, id] = parts;
    if (type === "EVENT") {
      if (id === this.#subscriptionId || this.#queries.has(id)) this.#engine.event(text);
    } else if (type === "OK") {
      this.#verdict(slot, id, parts[2] === true, parts[3]);
    } else if (type === "EOSE") {
      if (id === this.#subscriptionId) {
        clearTimeout(slot.confirm);
        slot.confirm = null;
        slot.backoff = RETRY_MIN_MS;
        slot.live = true;
        this.#setLive(true);
      } else {
        this.#answered(slot, id);
      }
    } else if (type === "CLOSED") {
      // A relay that ends this device's subscription is not carrying it.
      if (id === this.#subscriptionId) this.#lost(slot, slot.socket);
      else this.#answered(slot, id);
    }
  }

  #verdict(slot, eventId, accepted, reason) {
    const entry = this.#pending.get(eventId);
    if (!entry) return;
    if (accepted) {
      this.#pending.delete(eventId);
      return this.#engine.confirmed(entry.messageId);
    }
    entry.reason = typeof reason === "string" && reason ? reason : "relay rejected event";
    this.#released(entry, eventId, slot.url);
  }

  // `url` will not answer for `eventId`; fail it once nobody else can.
  #released(entry, eventId, url) {
    entry.waiting.delete(url);
    if (entry.waiting.size > 0) return;
    this.#pending.delete(eventId);
    this.#engine.failed(entry.messageId, entry.reason ?? "every relay dropped");
  }

  #answered(slot, queryId) {
    const query = this.#queries.get(queryId);
    if (!query || !query.waiting.delete(slot.url)) return;
    if (slot.socket?.readyState === WebSocket.OPEN) {
      slot.socket.send(JSON.stringify(["CLOSE", queryId]));
    }
    if (query.waiting.size === 0) this.#finish(queryId);
  }

  #finish(queryId) {
    const query = this.#queries.get(queryId);
    if (!query) return;
    clearTimeout(query.deadline);
    this.#queries.delete(queryId);
    for (const slot of this.#slots) {
      if (query.waiting.has(slot.url) && slot.socket?.readyState === WebSocket.OPEN) {
        slot.socket.send(JSON.stringify(["CLOSE", queryId]));
      }
    }
    this.#engine.completed(queryId);
  }

  #lost(slot, socket) {
    if (slot.socket !== socket || socket === null) return;
    this.#drop(slot);
    for (const [eventId, entry] of [...this.#pending]) {
      if (entry.waiting.has(slot.url)) this.#released(entry, eventId, slot.url);
    }
    for (const queryId of [...this.#queries.keys()]) this.#answered(slot, queryId);
    this.#setLive(this.#slots.some((s) => s.live));
    if (this.#closed) return;
    slot.retry = setTimeout(() => this.#connect(slot), slot.backoff);
    slot.backoff = Math.min(slot.backoff * 2, RETRY_MAX_MS);
  }

  #drop(slot) {
    clearTimeout(slot.confirm);
    slot.confirm = null;
    slot.live = false;
    const socket = slot.socket;
    slot.socket = null;
    if (!socket) return;
    socket.onopen = socket.onerror = socket.onclose = socket.onmessage = null;
    try {
      socket.close();
    } catch {
      // Closing a socket that never opened throws in some engines.
    }
  }

  #setLive(live) {
    if (live === this.#live) return;
    this.#live = live;
    this.#engine.status(live);
  }
}
