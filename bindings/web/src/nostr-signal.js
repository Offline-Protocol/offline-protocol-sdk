// WebRTC signalling through Nostr relays, by room code (docs/bridges/web.md, W11).
//
// Invariants:
// - This file moves text. The REQ it sends, the events it publishes and what
//   a relay's message means are built and opened in Rust (signal.rs, in the
//   node's worker): the room's topic and key, the seal, the signature and
//   every check on what a relay sent. The page only reads a relay's EOSE, to
//   know its subscription is live before it publishes.
// - A room code is a shared secret, not an identity. Its holder can open a
//   carrier toward the host; who is on that carrier is what the engine's
//   preamble and MLS session prove, as for a pasted code.
// - The host answers each joiner's session once, and again only when the
//   same session asks again (its first answer may have been lost). It holds
//   at most MAX_SESSIONS sessions, so a room code that leaked cannot make the
//   host open connections without bound; the engine's own link ceiling
//   stands behind that.
// - The host remembers a session until the newest offer it has seen for it
//   stops being fresh: the `freshUntilMs` the worker reports (signal.rs,
//   Opened::fresh_until), moved later by every repeat. An offer the worker
//   still accepts is one the host still remembers, so a relay that re-sends
//   an offer late reaches the session it belongs to rather than opening a
//   second connection. A fixed hold from first sight is too short twice
//   over: the joiner republishes for up to JOIN_TIMEOUT_MS, and a joiner
//   whose clock runs ahead sends offers that stay fresh longer.
// - A hosted room stays reachable for as long as it is open. A relay socket
//   that closes is dialled again, with backoff, and a live one is asked to
//   confirm the subscription every KEEP_ALIVE_MS and dropped if it does not,
//   because a socket the network lost without a word still reads as open.
//   Coming back online or into view probes at once. A room that dropped its
//   sockets and never redialled answered nobody, while host() had long
//   since resolved and the page had no way to tell.
// - A channel that fails to open closes every socket it opened. A relay that
//   connects but never confirms the subscription would otherwise stay open,
//   and keep delivering to a room nobody is answering.
// - A joiner publishes its offer until an answer for its session arrives or
//   its deadline passes, with a new event each time: relays drop an event id
//   they have seen, and the event is ephemeral, so a host that subscribed late
//   only sees one published after it did.

const OPEN_TIMEOUT_MS = 10000;
const REPUBLISH_MS = 2000;
const JOIN_TIMEOUT_MS = 30000;
const MAX_SESSIONS = 16;
// A hosted room redials a relay that dropped after RETRY_MIN_MS, doubling to
// RETRY_MAX_MS, and asks each live relay to confirm its subscription every
// KEEP_ALIVE_MS, dropping one that has not within PROBE_MS (or the keep-alive
// period, if that is shorter, so probes never overlap).
const RETRY_MIN_MS = 1000;
const RETRY_MAX_MS = 30000;
const KEEP_ALIVE_MS = 60000;
const PROBE_MS = 10000;

function randomHex(bytes) {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (b) =>
    b.toString(16).padStart(2, "0"),
  ).join("");
}

/// One subscription to a room on every relay. `onSignal` gets each signal a
/// relay delivers that opens for this room. A channel that `stays` (a hosted
/// room) keeps every relay subscribed for as long as it is open; one that
/// does not (a join, which lasts seconds) lets a relay that drops go.
class RoomChannel {
  #node;
  #relays;
  #room;
  #onSignal;
  #keepAliveMs;
  // One per relay: { url, socket, request, live, answered, retry, backoff, refused }.
  #slots = [];
  #subscriptionId = randomHex(8);
  #closed = false;
  // While open() waits: { live(), refused() }.
  #waiting = null;
  #keepAlive = null;
  #wake = () => this.#probe({ now: true });
  #shown = () => document.visibilityState === "visible" && this.#probe({ now: true });

  /// `keepAliveMs` makes the channel stay, probing each relay that often.
  constructor(node, relays, room, onSignal, { keepAliveMs = null } = {}) {
    this.#node = node;
    this.#relays = relays;
    this.#room = room;
    this.#onSignal = onSignal;
    this.#keepAliveMs = keepAliveMs;
  }

  get #stays() {
    return this.#keepAliveMs !== null;
  }

  /// Resolves once at least one relay has the subscription live, and rejects
  /// when none does in time, with every socket it opened closed.
  async open() {
    try {
      await new Promise((resolve, reject) => {
        let refused = 0;
        const timer = setTimeout(() => reject(new Error("no Nostr relay answered")), OPEN_TIMEOUT_MS);
        this.#waiting = {
          live: () => {
            clearTimeout(timer);
            resolve();
          },
          refused: () => {
            if (++refused < this.#relays.length) return;
            clearTimeout(timer);
            reject(new Error("every Nostr relay refused the connection"));
          },
        };
        for (const url of this.#relays) {
          const slot = { url, socket: null, request: null, live: false, answered: false, retry: null, backoff: RETRY_MIN_MS, refused: false };
          this.#slots.push(slot);
          this.#connect(slot);
        }
      });
    } catch (error) {
      this.close();
      throw error;
    } finally {
      this.#waiting = null;
    }
    if (this.#stays && !this.#closed) {
      this.#keepAlive = setInterval(() => this.#probe(), this.#keepAliveMs);
      globalThis.addEventListener?.("online", this.#wake);
      globalThis.document?.addEventListener("visibilitychange", this.#shown);
    }
  }

  async #connect(slot) {
    let socket;
    try {
      // Built again for every socket, so a resubscription asks from now on.
      slot.request = await this.#node.signalSubscription(this.#room, this.#subscriptionId);
      if (this.#closed) return;
      socket = new WebSocket(slot.url);
    } catch {
      return this.#lost(slot, null);
    }
    slot.socket = socket;
    slot.live = false;
    // A socket that fails fires both error and close; #lost acts on the first.
    socket.onerror = () => this.#lost(slot, socket);
    socket.onclose = () => this.#lost(slot, socket);
    socket.onopen = () => socket.send(slot.request);
    socket.onmessage = ({ data }) => {
      if (typeof data !== "string") return;
      if (this.#isEose(data)) {
        slot.answered = true;
        if (!slot.live) {
          slot.live = true;
          slot.backoff = RETRY_MIN_MS;
          this.#waiting?.live();
        }
        return;
      }
      this.#node
        .signalOpen(this.#room, data)
        .then((signal) => signal && !this.#closed && this.#onSignal(signal))
        .catch(() => {});
    };
  }

  // The relay behind `slot` is gone: counted once against open(), and dialled
  // again after a backoff if the channel stays. A socket that closes after
  // the subscription went live used to be left closed, and a hosted room
  // whose sockets the network had dropped (a phone that slept, Wi-Fi that
  // came back) went on looking open while nobody could reach it.
  #lost(slot, socket) {
    if (slot.socket !== socket) return;
    slot.socket = null;
    slot.live = false;
    if (this.#waiting && !slot.refused) {
      slot.refused = true;
      this.#waiting.refused();
    }
    if (this.#closed || !this.#stays || slot.retry !== null) return;
    slot.retry = setTimeout(() => {
      slot.retry = null;
      this.#connect(slot);
    }, slot.backoff);
    slot.backoff = Math.min(slot.backoff * 2, RETRY_MAX_MS);
  }

  // Asks every live relay to confirm the subscription again, and drops one
  // that does not answer within PROBE_MS. A socket the network dropped
  // without a word (a phone's radio asleep) still reads as open, and would
  // only fire close when TCP gives up, minutes later. `now` also redials at
  // once every relay waiting out a backoff: the page has just come back
  // online or into view, and the relays most likely answer again.
  #probe({ now = false } = {}) {
    if (this.#closed) return;
    for (const slot of this.#slots) {
      if (slot.retry !== null) {
        if (!now) continue;
        clearTimeout(slot.retry);
        slot.retry = null;
        slot.backoff = RETRY_MIN_MS;
        this.#connect(slot);
        continue;
      }
      const socket = slot.socket;
      if (!slot.live || socket?.readyState !== WebSocket.OPEN) continue;
      slot.answered = false;
      // NIP-01: a REQ under an open subscription's id replaces it, and the
      // relay answers with EOSE again.
      socket.send(slot.request);
      setTimeout(() => {
        if (slot.socket !== socket || slot.answered) return;
        this.#drop(socket);
        this.#lost(slot, socket);
      }, Math.min(PROBE_MS, this.#keepAliveMs));
    }
  }

  #drop(socket) {
    socket.onerror = socket.onclose = socket.onmessage = socket.onopen = null;
    try {
      socket.close();
    } catch {
      // A socket that throws on close is closed.
    }
  }

  #isEose(data) {
    try {
      const message = JSON.parse(data);
      return message[0] === "EOSE" && message[1] === this.#subscriptionId;
    } catch {
      return false;
    }
  }

  /// Sends EVENT text to every relay that is open.
  publish(text) {
    for (const { socket } of this.#slots) {
      if (socket?.readyState === WebSocket.OPEN) socket.send(text);
    }
  }

  close() {
    if (this.#closed) return;
    this.#closed = true;
    clearInterval(this.#keepAlive);
    globalThis.removeEventListener?.("online", this.#wake);
    globalThis.document?.removeEventListener("visibilitychange", this.#shown);
    const done = JSON.stringify(["CLOSE", this.#subscriptionId]);
    for (const slot of this.#slots) {
      clearTimeout(slot.retry);
      slot.retry = null;
      const socket = slot.socket;
      slot.socket = null;
      if (!socket) continue;
      try {
        if (socket.readyState === WebSocket.OPEN) socket.send(done);
      } catch {
        // Closed below either way.
      }
      this.#drop(socket);
    }
  }
}

/// Sets up WebRTC links through Nostr relays, by room code, instead of by
/// carrying two codes between devices. One side hosts a room; any device that
/// knows the room code joins it, and each join becomes one WebRtcCarrier link.
export class NostrSignaller {
  #node;
  #relays;
  #keepAliveMs;

  /// `relays` are WebSocket URLs (`wss://…`). Any NIP-01 relay that forwards
  /// ephemeral events will do; more than one rides out a relay that is down.
  /// `keepAliveMs` is how often a hosted room asks each relay to confirm it
  /// is still subscribed.
  constructor(node, { relays, keepAliveMs = KEEP_ALIVE_MS } = {}) {
    if (!Array.isArray(relays) || relays.length === 0) {
      throw new Error("NostrSignaller needs at least one relay URL");
    }
    if (!(keepAliveMs > 0)) throw new Error("keepAliveMs must be a positive number of milliseconds");
    this.#node = node;
    this.#relays = relays;
    this.#keepAliveMs = keepAliveMs;
  }

  /// A fresh room code to show the other players.
  newRoom() {
    return this.#node.newRoom();
  }

  /// Answers every device that joins `room`, through `carrier`. Resolves once
  /// the room is listening to `{ close() }`; `onConnection` gets each answered
  /// connection's `{ close() }`. Until it is closed the room redials a relay
  /// that drops and probes the live ones, so it outlasts a phone that slept
  /// or a network that went away. Closing stops answering; links already
  /// made stay up.
  async host(room, carrier, { onConnection } = {}) {
    // session -> { answer, connection, freshUntilMs, timer }
    const sessions = new Map();
    const forget = (session, entry) => {
      clearTimeout(entry.timer);
      if (sessions.get(session) === entry) sessions.delete(session);
    };
    // Keeps `entry` until its newest offer is no longer fresh; a repeat only
    // ever moves that later.
    const hold = (session, entry, freshUntilMs) => {
      if (freshUntilMs <= entry.freshUntilMs) return;
      entry.freshUntilMs = freshUntilMs;
      clearTimeout(entry.timer);
      entry.timer = setTimeout(() => forget(session, entry), Math.max(0, freshUntilMs - Date.now()));
    };
    const channel = new RoomChannel(this.#node, this.#relays, room, async (signal) => {
      if (signal.role !== "offer") return;
      const known = sessions.get(signal.session);
      if (known) {
        hold(signal.session, known, signal.freshUntilMs);
        if (known.answer) channel.publish(known.answer);
        return;
      }
      if (sessions.size >= MAX_SESSIONS) return;
      const entry = { answer: null, connection: null, freshUntilMs: -Infinity, timer: undefined };
      sessions.set(signal.session, entry);
      hold(signal.session, entry, signal.freshUntilMs);
      try {
        entry.connection = await carrier.answer(signal.code);
        entry.answer = await this.#node.signalEvent(room, "answer", signal.session, entry.connection.code);
        channel.publish(entry.answer);
        onConnection?.(entry.connection);
      } catch {
        entry.connection?.close();
        forget(signal.session, entry);
      }
    }, { keepAliveMs: this.#keepAliveMs });
    await channel.open();
    return {
      room,
      close: () => {
        channel.close();
        for (const [session, entry] of sessions) forget(session, entry);
      },
    };
  }

  /// Joins the device hosting `room`, through `carrier`. Resolves to the
  /// connection's `{ close() }` once the host's answer is accepted, and
  /// rejects when no answer comes within `timeoutMs`.
  async join(room, carrier, { timeoutMs = JOIN_TIMEOUT_MS } = {}) {
    const session = randomHex(16);
    const offer = await carrier.offer();
    let settle;
    const answered = new Promise((resolve, reject) => (settle = { resolve, reject }));
    let accepted = false;
    const channel = new RoomChannel(this.#node, this.#relays, room, async (signal) => {
      if (accepted || signal.role !== "answer" || signal.session !== session) return;
      accepted = true;
      try {
        await offer.accept(signal.code);
        settle.resolve();
      } catch (error) {
        settle.reject(error);
      }
    });
    let republish;
    let deadline;
    try {
      await channel.open();
      const publish = async () =>
        channel.publish(await this.#node.signalEvent(room, "offer", session, offer.code));
      await publish();
      republish = setInterval(() => publish().catch(() => {}), REPUBLISH_MS);
      deadline = setTimeout(
        () => settle.reject(new Error("no host answered in the room")),
        timeoutMs,
      );
      await answered;
      return { close: () => offer.close() };
    } catch (error) {
      offer.close();
      throw error;
    } finally {
      clearInterval(republish);
      clearTimeout(deadline);
      channel.close();
    }
  }
}
