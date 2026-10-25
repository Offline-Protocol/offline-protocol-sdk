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
//   A redialled socket that never confirms is dropped the same way: a relay
//   that accepts a socket and says nothing would otherwise hold its slot
//   for good. Coming back online or into view probes at once. A room that
//   dropped its sockets and never redialled answered nobody, while host()
//   had long since resolved and the page had no way to tell. A room whose
//   node stopped closes itself, as nothing it heard could be answered.
// - A relay waits on one confirmation at a time. A probe never starts while
//   another is unanswered: a later probe that reset the earlier one's state
//   would make an answer read as silence, and browsers run an interval's
//   next tick ahead of a deadline of the same length set inside it, so a
//   short keep-alive dropped relays that had answered.
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
// KEEP_ALIVE_MS. A socket that has not confirmed within PROBE_MS (or the
// keep-alive period, if that is shorter) of being dialled or asked is dropped.
const RETRY_MIN_MS = 1000;
const RETRY_MAX_MS = 30000;
const KEEP_ALIVE_MS = 60000;
const PROBE_MS = 10000;
// The longest delay a browser timer honours; a longer one fires at once.
const MAX_TIMER_MS = 0x7fffffff;

function randomHex(bytes) {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (b) =>
    b.toString(16).padStart(2, "0"),
  ).join("");
}

/// What a room channel subscribes with and opens through: `subscription(id)`
/// resolves to the REQ text, and `open(text)` to what a relay message
/// carries for the room, or null. Both run in the worker.
export function signalRoom(node, room) {
  return {
    subscription: (id) => node.signalSubscription(room, id),
    open: (text) => node.signalOpen(room, text),
  };
}

/// One subscription to a room on every relay. `onSignal` gets each signal a
/// relay delivers that opens for this room. A channel that `stays` (a hosted
/// room) keeps every relay subscribed for as long as it is open; one that
/// does not (a join, which lasts seconds) lets a relay that drops go.
export class RoomChannel {
  #source;
  #relays;
  #onSignal;
  #keepAliveMs;
  // One per relay: { url, socket, request, live, confirm, retry, backoff, refused }.
  // `confirm` is the deadline for the confirmation the socket waits on.
  #slots = [];
  #subscriptionId = randomHex(8);
  #closed = false;
  // While open() waits: { live(), refused(), failed(error) }.
  #waiting = null;
  #keepAlive = null;
  #wake = () => this.#probe({ now: true });
  #shown = () => document.visibilityState === "visible" && this.#probe({ now: true });

  /// `keepAliveMs` makes the channel stay, probing each relay that often.
  constructor(source, relays, onSignal, { keepAliveMs = null } = {}) {
    this.#source = source;
    this.#relays = relays;
    this.#onSignal = onSignal;
    this.#keepAliveMs = keepAliveMs === null ? null : Math.min(keepAliveMs, MAX_TIMER_MS);
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
        const settle = (error) => {
          clearTimeout(timer);
          if (error) reject(error);
          else resolve();
        };
        this.#waiting = {
          live: () => settle(),
          refused: () => {
            if (++refused === this.#relays.length) {
              settle(new Error("every Nostr relay refused the connection"));
            }
          },
          failed: (error) => settle(error),
        };
        for (const url of this.#relays) {
          const slot = { url, socket: null, request: null, live: false, confirm: null, retry: null, backoff: RETRY_MIN_MS, refused: false };
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
    try {
      // Built again for every socket, so a resubscription asks from now on.
      slot.request = await this.#source.subscription(this.#subscriptionId);
    } catch (error) {
      // The node stopped: nothing this room hears could be answered.
      return this.#failed(error);
    }
    if (this.#closed) return;
    let socket;
    try {
      socket = new WebSocket(slot.url);
    } catch {
      // A URL the browser refuses (malformed, a blocked port) stays refused.
      return this.#lost(slot, null, { again: false });
    }
    slot.socket = socket;
    slot.live = false;
    // A socket that hangs while it connects counts as one that never confirms.
    this.#await(slot, socket);
    // A socket that fails fires both error and close; #lost acts on the first.
    socket.onerror = () => this.#lost(slot, socket);
    socket.onclose = () => this.#lost(slot, socket);
    socket.onopen = () => socket.send(slot.request);
    socket.onmessage = ({ data }) => {
      if (typeof data !== "string") return;
      if (this.#isEose(data)) {
        if (slot.socket !== socket) return;
        clearTimeout(slot.confirm);
        slot.confirm = null;
        if (!slot.live) {
          slot.live = true;
          slot.backoff = RETRY_MIN_MS;
          this.#waiting?.live();
        }
        return;
      }
      this.#source
        .open(data)
        .then((signal) => signal && !this.#closed && this.#onSignal(signal))
        .catch(() => {});
    };
  }

  // In a channel that stays, drops `socket` unless its relay confirms the
  // subscription in time. Only EOSE clears the deadline, and nothing resets
  // it, so an answer is never read as silence.
  #await(slot, socket) {
    if (!this.#stays) return;
    clearTimeout(slot.confirm);
    slot.confirm = setTimeout(() => {
      slot.confirm = null;
      if (slot.socket !== socket) return;
      this.#drop(socket);
      this.#lost(slot, socket);
    }, Math.min(PROBE_MS, this.#keepAliveMs));
  }

  // The node refused to build a subscription, which it does only once it
  // has stopped: open() rejects with its error, and a room already open
  // closes rather than redial forever.
  #failed(error) {
    if (this.#waiting) this.#waiting.failed(error);
    else this.close();
  }

  // The relay behind `slot` is gone: counted once against open(), and dialled
  // again after a backoff if the channel stays and `again`. A socket that
  // closes after the subscription went live used to be left closed, and a
  // hosted room whose sockets the network had dropped (a phone that slept,
  // Wi-Fi that came back) went on looking open while nobody could reach it.
  #lost(slot, socket, { again = true } = {}) {
    if (slot.socket !== socket) return;
    slot.socket = null;
    slot.live = false;
    clearTimeout(slot.confirm);
    slot.confirm = null;
    if (this.#waiting && !slot.refused) {
      slot.refused = true;
      this.#waiting.refused();
    }
    if (!again || this.#closed || !this.#stays || slot.retry !== null) return;
    slot.retry = setTimeout(() => {
      slot.retry = null;
      this.#connect(slot);
    }, slot.backoff);
    slot.backoff = Math.min(slot.backoff * 2, RETRY_MAX_MS);
  }

  // Asks every live relay to confirm the subscription again, and drops one
  // that does not answer within PROBE_MS. A socket the network dropped
  // without a word (a phone's radio asleep) still reads as open, and would
  // only fire close when TCP gives up, minutes later. A relay still owing a
  // confirmation is left to its deadline. `now` also redials at once every
  // relay waiting out a backoff: the page has just come back online or into
  // view, and the relays most likely answer again.
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
      if (!slot.live || slot.confirm !== null || socket?.readyState !== WebSocket.OPEN) continue;
      // NIP-01: a REQ under an open subscription's id replaces it, and the
      // relay answers with EOSE again.
      socket.send(slot.request);
      this.#await(slot, socket);
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
      clearTimeout(slot.confirm);
      slot.retry = slot.confirm = null;
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
    if (typeof keepAliveMs !== "number" || !(keepAliveMs > 0)) {
      throw new Error("keepAliveMs must be a positive number of milliseconds");
    }
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
  /// or a network that went away. A room whose node stops closes itself.
  /// Closing stops answering; links already made stay up.
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
    const channel = new RoomChannel(signalRoom(this.#node, room), this.#relays, async (signal) => {
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
    const channel = new RoomChannel(signalRoom(this.#node, room), this.#relays, async (signal) => {
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
