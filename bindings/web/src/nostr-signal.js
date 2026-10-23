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
//   at most MAX_SESSIONS answered sessions, each for SESSION_HOLD_MS, so a
//   room code that leaked cannot make the host open connections without
//   bound; the engine's own link ceiling stands behind that.
// - A joiner publishes its offer until an answer for its session arrives or
//   its deadline passes, with a new event each time: relays drop an event id
//   they have seen, and the event is ephemeral, so a host that subscribed late
//   only sees one published after it did.

const OPEN_TIMEOUT_MS = 10000;
const REPUBLISH_MS = 2000;
const JOIN_TIMEOUT_MS = 30000;
const MAX_SESSIONS = 16;
const SESSION_HOLD_MS = 60000;

function randomHex(bytes) {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (b) =>
    b.toString(16).padStart(2, "0"),
  ).join("");
}

/// One subscription to a room on every relay. `onSignal` gets each signal a
/// relay delivers that opens for this room.
class RoomChannel {
  #node;
  #relays;
  #room;
  #onSignal;
  #sockets = [];
  #subscriptionId = randomHex(8);
  #closed = false;

  constructor(node, relays, room, onSignal) {
    this.#node = node;
    this.#relays = relays;
    this.#room = room;
    this.#onSignal = onSignal;
  }

  /// Resolves once at least one relay has the subscription live, and rejects
  /// when none does in time.
  async open() {
    const request = await this.#node.signalSubscription(this.#room, this.#subscriptionId);
    await new Promise((resolve, reject) => {
      let failed = 0;
      const timer = setTimeout(() => reject(new Error("no Nostr relay answered")), OPEN_TIMEOUT_MS);
      const fail = () => {
        if (++failed === this.#relays.length) {
          clearTimeout(timer);
          reject(new Error("every Nostr relay refused the connection"));
        }
      };
      for (const url of this.#relays) {
        let socket;
        try {
          socket = new WebSocket(url);
        } catch {
          fail();
          continue;
        }
        this.#sockets.push(socket);
        let live = false;
        socket.onopen = () => socket.send(request);
        socket.onerror = () => !live && fail();
        socket.onclose = () => !live && fail();
        socket.onmessage = ({ data }) => {
          if (typeof data !== "string") return;
          if (!live && this.#isEose(data)) {
            live = true;
            clearTimeout(timer);
            resolve();
            return;
          }
          this.#node
            .signalOpen(this.#room, data)
            .then((signal) => signal && !this.#closed && this.#onSignal(signal))
            .catch(() => {});
        };
      }
    });
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
    for (const socket of this.#sockets) {
      if (socket.readyState === WebSocket.OPEN) socket.send(text);
    }
  }

  close() {
    if (this.#closed) return;
    this.#closed = true;
    const done = JSON.stringify(["CLOSE", this.#subscriptionId]);
    for (const socket of this.#sockets) {
      try {
        if (socket.readyState === WebSocket.OPEN) socket.send(done);
        socket.close();
      } catch {
        // A socket that throws on close is closed.
      }
    }
  }
}

/// Sets up WebRTC links through Nostr relays, by room code, instead of by
/// carrying two codes between devices. One side hosts a room; any device that
/// knows the room code joins it, and each join becomes one WebRtcCarrier link.
export class NostrSignaller {
  #node;
  #relays;

  /// `relays` are WebSocket URLs (`wss://…`). Any NIP-01 relay that forwards
  /// ephemeral events will do; more than one rides out a relay that is down.
  constructor(node, { relays } = {}) {
    if (!Array.isArray(relays) || relays.length === 0) {
      throw new Error("NostrSignaller needs at least one relay URL");
    }
    this.#node = node;
    this.#relays = relays;
  }

  /// A fresh room code to show the other players.
  newRoom() {
    return this.#node.newRoom();
  }

  /// Answers every device that joins `room`, through `carrier`. Resolves once
  /// the room is listening to `{ close() }`; `onConnection` gets each answered
  /// connection's `{ close() }`. Closing stops answering; links already made
  /// stay up.
  async host(room, carrier, { onConnection } = {}) {
    const sessions = new Map(); // session -> { answer, connection } once answered
    const channel = new RoomChannel(this.#node, this.#relays, room, async (signal) => {
      if (signal.role !== "offer") return;
      const known = sessions.get(signal.session);
      if (known) {
        if (known.answer) channel.publish(known.answer);
        return;
      }
      if (sessions.size >= MAX_SESSIONS) return;
      const entry = { answer: null, connection: null };
      sessions.set(signal.session, entry);
      setTimeout(() => sessions.delete(signal.session), SESSION_HOLD_MS);
      try {
        entry.connection = await carrier.answer(signal.code);
        entry.answer = await this.#node.signalEvent(room, "answer", signal.session, entry.connection.code);
        channel.publish(entry.answer);
        onConnection?.(entry.connection);
      } catch {
        entry.connection?.close();
        sessions.delete(signal.session);
      }
    });
    await channel.open();
    return { room, close: () => channel.close() };
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
