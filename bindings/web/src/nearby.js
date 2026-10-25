// Finding devices on the same network through Nostr relays (docs/bridges/web.md, W13).
//
// Invariants:
// - This file moves text. The room's topic and key, every seal and every
//   check on what a relay sent are in Rust (nearby.rs, in the node's
//   worker). The page learns the network's address, holds the relay sockets
//   and decides when to publish.
// - The relays and the STUN servers are the application's. Nothing here
//   names one: a NearbyRoom without relays, or without a STUN server and
//   without a `network`, refuses to open.
// - The room is weak: anyone who can guess a network's public address can
//   derive its key. So a presence carries only a key used for this room and
//   the label the application chose, and every connection code is sealed to
//   one presence key. A holder of the room key sees who is present and can
//   offer, and reads no code.
// - Nothing is answered without the application's say. Every offer goes to
//   `admit({ peer, label, signal })` first, and only `true` opens a
//   connection. `signal` aborts when the offer stops being fresh or the room
//   closes, so a prompt nobody answered can be taken down. The label is a
//   claim by whoever sent it; who is on the link is what the engine's
//   preamble and MLS session prove (W3).
// - A refused offer never takes the place of one still to be decided. The
//   room is public, so anyone can offer. A refused session used to stay among
//   the MAX_SESSIONS the host decides on, kept by every repeat, and sixteen
//   refused sessions republished by a stranger anywhere on the internet
//   locked the host out of admitting anyone, silently. Refused sessions are
//   kept apart (at most MAX_REFUSED, oldest forgotten first) only so their
//   repeats are not asked about again. An offer dropped because MAX_SESSIONS
//   are waiting on the application is reported as a `full` event.
// - A presence is republished every PRESENCE_MS and forgotten when unseen for
//   PRESENCE_STALE_MS, so the list is the devices present now. A full list
//   forgets the device heard from longest ago: refusing newcomers let
//   whoever filled it first keep everyone else off it.
// - A host answers a session once, and again only for that session's
//   repeats, for as long as its newest offer is fresh (W11).
// - A room opens once. One that failed to open, or was closed, holds no
//   socket and no room in the worker, and refuses to open or connect again.

import { RoomChannel } from "./nostr-signal.js";
import { checkRelays } from "./nostr-relays.js";

const PRESENCE_MS = 5000;
const PRESENCE_STALE_MS = 15000;
const REPUBLISH_MS = 2000;
const CONNECT_TIMEOUT_MS = 30000;
const ADDRESS_TIMEOUT_MS = 5000;
// After an IPv6 server-reflexive candidate, how long to wait for an IPv4 one.
const ADDRESS_SETTLE_MS = 500;
const KEEP_ALIVE_MS = 60000;
const MAX_PEERS = 64;
const MAX_SESSIONS = 16;
const MAX_REFUSED = 256;

function randomHex(bytes) {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (b) =>
    b.toString(16).padStart(2, "0"),
  ).join("");
}

/// The address the internet sees this device from, as the application's
/// STUN servers report it: a server-reflexive candidate a throwaway
/// connection gathers, IPv4 when there is one. Two devices on a dual-stack
/// network must pick the same family or they derive different rooms, and the
/// first candidate's family depends on which STUN answer arrived first, so
/// an IPv6 one waits `settleMs` for an IPv4 one. Rejects when none arrives
/// within `timeoutMs`, which is what a network that blocks STUN, or a list
/// with no STUN server, does.
export async function publicAddress(
  iceServers,
  { timeoutMs = ADDRESS_TIMEOUT_MS, settleMs = ADDRESS_SETTLE_MS } = {},
) {
  const stun = (iceServers ?? []).some((server) =>
    [server?.urls].flat().some((url) => typeof url === "string" && /^(stuns?|turns?):/.test(url)),
  );
  if (!stun) throw new Error("iceServers must name a STUN server to learn the network's address");
  const pc = new RTCPeerConnection({ iceServers });
  try {
    return await new Promise((resolve, reject) => {
      const found = [];
      let settle = null;
      const done = () => {
        clearTimeout(timer);
        clearTimeout(settle);
        if (found.length === 0) reject(new Error("no STUN server reported this network's address"));
        else resolve(found.find(isIpv4) ?? found[0]);
      };
      const timer = setTimeout(done, timeoutMs);
      pc.onicecandidate = ({ candidate }) => {
        // The end of gathering: nothing more will come.
        if (!candidate) return done();
        const address = reflexive(candidate);
        if (!address) return;
        found.push(address);
        if (isIpv4(address)) return done();
        settle ??= setTimeout(done, settleMs);
      };
      pc.createDataChannel("nearby");
      pc.createOffer()
        .then((offer) => pc.setLocalDescription(offer))
        .catch((error) => {
          clearTimeout(timer);
          reject(error);
        });
    });
  } finally {
    pc.close();
  }
}

// The address of a server-reflexive candidate, or null for any other kind.
// An engine may leave any parsed field out, so each falls back to the
// candidate line: `candidate:<f> <c> <p> <prio> <addr> <port> typ srflx …`.
function reflexive(candidate) {
  const parts = (candidate.candidate ?? "").split(" ");
  if ((candidate.type ?? parts[7]) !== "srflx") return null;
  return candidate.address || parts[4] || null;
}

const isIpv4 = (address) => !address.includes(":");

/// The devices behind this network's address for one application, through
/// relays: each NearbyRoom announces itself, lists the others, and links
/// over `carrier` (a WebRtcCarrier) to one its person picks.
///
/// `admit({ peer, label, signal })` decides every offer this device
/// receives; it is required, so a page that opens a room has said who it
/// lets in. A `full` event reports an offer dropped because MAX_SESSIONS
/// were already waiting on `admit`.
export class NearbyRoom extends EventTarget {
  #node;
  #carrier;
  #relays;
  #iceServers;
  #network;
  #label;
  #admit;
  #room = null;
  #channel = null;
  #timer = null;
  #opened = false;
  #live = false;
  #closed = false;
  // key -> { label, seenAt }
  #peers = new Map();
  // session -> { answer, connection, freshUntilMs, timer, abort }: offers the
  // application is deciding on or admitted. At most MAX_SESSIONS.
  #sessions = new Map();
  // session -> { freshUntilMs, timer }: offers the application refused,
  // ignored while fresh. At most MAX_REFUSED, oldest first out.
  #refused = new Map();
  // session -> (opened) => void, for this device's own offers.
  #joins = new Map();

  constructor(node, carrier, { relays, iceServers, network, label = "", admit } = {}) {
    super();
    this.#relays = checkRelays(relays);
    if (typeof admit !== "function") {
      throw new Error("NearbyRoom needs admit({ peer, label }): who this device lets in is the application's call");
    }
    if (network === undefined && !Array.isArray(iceServers)) {
      throw new Error("NearbyRoom needs iceServers with a STUN server, or the network's address");
    }
    this.#node = node;
    this.#carrier = carrier;
    this.#iceServers = iceServers;
    this.#network = network;
    this.#label = label;
    this.#admit = admit;
  }

  /// The devices present now: `[{ peer, label, seenAt }]`, newest first.
  get peers() {
    return [...this.#peers]
      .map(([peer, { label, seenAt }]) => ({ peer, label, seenAt }))
      .sort((a, b) => b.seenAt - a.seenAt);
  }

  /// This device's key in the room, once open.
  get key() {
    return this.#room?.key ?? null;
  }

  /// Learns the network's address (unless given one), subscribes on every
  /// relay and starts announcing this device. Rejects when no STUN server
  /// answers or no relay confirms the room, with everything it opened
  /// closed. A room opens once.
  async open() {
    if (this.#closed) throw new Error("this nearby room is closed");
    if (this.#opened) throw new Error("this nearby room is already open");
    this.#opened = true;
    try {
      const network = this.#network ?? (await publicAddress(this.#iceServers));
      if (this.#closed) throw new Error("the nearby room was closed while it opened");
      this.#room = await this.#node.nearbyOpen(network, this.#label);
      const id = this.#room.id;
      if (this.#closed) throw new Error("the nearby room was closed while it opened");
      this.#channel = new RoomChannel(
        {
          subscription: (sub) => this.#node.nearbySubscription(id, sub),
          open: (text) => this.#node.nearbyOpenMessage(id, text),
        },
        this.#relays,
        (opened) => this.#heard(opened),
        { keepAliveMs: KEEP_ALIVE_MS },
      );
      await this.#channel.open();
      await this.#announce();
      if (this.#closed) throw new Error("the nearby room was closed while it opened");
    } catch (error) {
      this.close();
      throw error;
    }
    this.#live = true;
    this.#timer = setInterval(() => {
      this.#announce().catch(() => {});
      this.#sweep();
    }, PRESENCE_MS);
    return this;
  }

  /// Links to the device that announced `peer`. Resolves to the
  /// connection's `{ close() }` once that device admitted this one and its
  /// answer was accepted; rejects when no answer comes within `timeoutMs`.
  async connect(peer, { timeoutMs = CONNECT_TIMEOUT_MS } = {}) {
    if (this.#closed) throw new Error("this nearby room is closed");
    if (!this.#live) throw new Error("open the room first");
    const session = randomHex(16);
    const offer = await this.#carrier.offer();
    let republish;
    let deadline;
    try {
      const answered = new Promise((resolve, reject) => {
        this.#joins.set(session, (opened) => {
          if (opened.peer !== peer) return;
          this.#joins.delete(session);
          offer.accept(opened.code).then(resolve, reject);
        });
        deadline = setTimeout(() => reject(new Error("the device did not answer")), timeoutMs);
      });
      const publish = async () => {
        if (!this.#room) throw new Error("this nearby room is closed");
        this.#channel.publish(await this.#node.nearbyOffer(this.#room.id, peer, session, offer.code));
      };
      await publish();
      republish = setInterval(() => publish().catch(() => {}), REPUBLISH_MS);
      await answered;
      return { close: () => offer.close() };
    } catch (error) {
      offer.close();
      throw error;
    } finally {
      this.#joins.delete(session);
      clearInterval(republish);
      clearTimeout(deadline);
    }
  }

  /// Stops announcing and answering. Links already made stay up. Safe to
  /// call at any point, an open() still running included.
  close() {
    this.#closed = true;
    this.#live = false;
    clearInterval(this.#timer);
    this.#timer = null;
    this.#channel?.close();
    for (const map of [this.#sessions, this.#refused]) {
      for (const [session, entry] of [...map]) this.#forget(map, session, entry);
    }
    if (this.#room) {
      const { id } = this.#room;
      this.#room = null;
      this.#node.nearbyClose(id).catch(() => {});
    }
  }

  async #announce() {
    if (this.#closed) return;
    this.#channel.publish(await this.#node.nearbyPresence(this.#room.id));
  }

  #sweep() {
    const stale = Date.now() - PRESENCE_STALE_MS;
    let changed = false;
    for (const [peer, { seenAt }] of this.#peers) {
      if (seenAt < stale) {
        this.#peers.delete(peer);
        changed = true;
      }
    }
    if (changed) this.#changed();
  }

  #heard(opened) {
    if (this.#closed) return;
    if (opened.kind === "presence") return this.#present(opened);
    if (opened.kind === "answer") return this.#joins.get(opened.session)?.(opened);
    if (opened.kind === "offer") return this.#offered(opened);
  }

  #present({ peer, label }) {
    const known = this.#peers.get(peer);
    if (!known && this.#peers.size >= MAX_PEERS) {
      let oldest = null;
      for (const [key, { seenAt }] of this.#peers) {
        if (oldest === null || seenAt < oldest.seenAt) oldest = { key, seenAt };
      }
      this.#peers.delete(oldest.key);
    }
    this.#peers.set(peer, { label, seenAt: Date.now() });
    if (!known || known.label !== label) this.#changed();
  }

  async #offered({ peer, label, session, code, freshUntilMs }) {
    const refused = this.#refused.get(session);
    if (refused) return this.#hold(this.#refused, session, refused, freshUntilMs);
    const known = this.#sessions.get(session);
    if (known) {
      this.#hold(this.#sessions, session, known, freshUntilMs);
      if (known.answer) this.#channel.publish(known.answer);
      return;
    }
    if (this.#sessions.size >= MAX_SESSIONS) {
      this.dispatchEvent(new CustomEvent("full", { detail: { peer, label } }));
      return;
    }
    const entry = {
      answer: null,
      connection: null,
      freshUntilMs: -Infinity,
      timer: undefined,
      abort: new AbortController(),
    };
    this.#sessions.set(session, entry);
    this.#hold(this.#sessions, session, entry, freshUntilMs);
    let admitted = false;
    try {
      admitted = (await this.#admit({ peer, label, signal: entry.abort.signal })) === true;
    } catch {
      // An application that throws has not said yes.
    }
    // Forgotten while the application decided: the joiner stopped asking, or
    // the room closed. Nobody is waiting for this answer now.
    if (this.#sessions.get(session) !== entry) return;
    if (!admitted) {
      this.#forget(this.#sessions, session, entry);
      return this.#refuse(session, entry.freshUntilMs);
    }
    try {
      entry.connection = await this.#carrier.answer(code);
      if (this.#sessions.get(session) !== entry) return entry.connection.close();
      entry.answer = await this.#node.nearbyAnswer(this.#room.id, peer, session, entry.connection.code);
      this.#channel.publish(entry.answer);
    } catch {
      entry.connection?.close();
      this.#forget(this.#sessions, session, entry);
    }
  }

  #refuse(session, freshUntilMs) {
    if (this.#refused.size >= MAX_REFUSED) {
      const [oldest, entry] = this.#refused.entries().next().value;
      this.#forget(this.#refused, oldest, entry);
    }
    const entry = { freshUntilMs: -Infinity, timer: undefined };
    this.#refused.set(session, entry);
    this.#hold(this.#refused, session, entry, freshUntilMs);
  }

  // Keeps `entry` in `map` until its newest offer is no longer fresh, so a
  // relay's late repeat reaches the session it belongs to (W11).
  #hold(map, session, entry, freshUntilMs) {
    if (freshUntilMs <= entry.freshUntilMs) return;
    entry.freshUntilMs = freshUntilMs;
    clearTimeout(entry.timer);
    entry.timer = setTimeout(
      () => this.#forget(map, session, entry),
      Math.max(0, freshUntilMs - Date.now()),
    );
  }

  #forget(map, session, entry) {
    clearTimeout(entry.timer);
    entry.abort?.abort();
    if (map.get(session) === entry) map.delete(session);
  }

  #changed() {
    this.dispatchEvent(new CustomEvent("peers", { detail: this.peers }));
  }
}
