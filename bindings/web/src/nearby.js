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
//   `admit({ peer, label })` first, and only a true answer opens a
//   connection. The label is a claim by whoever sent it; who is on the link
//   is what the engine's preamble and MLS session prove (W3).
// - A presence is republished every PRESENCE_MS and forgotten when unseen for
//   PRESENCE_STALE_MS, so the list is the devices present now. A host
//   answers a session once, and again only for that session's repeats, for as
//   long as its newest offer is fresh, and holds at most MAX_SESSIONS, as a
//   signalling room does (W11).

import { RoomChannel } from "./nostr-signal.js";
import { checkRelays } from "./nostr-relays.js";

const PRESENCE_MS = 5000;
const PRESENCE_STALE_MS = 15000;
const REPUBLISH_MS = 2000;
const CONNECT_TIMEOUT_MS = 30000;
const ADDRESS_TIMEOUT_MS = 5000;
const KEEP_ALIVE_MS = 60000;
const MAX_PEERS = 64;
const MAX_SESSIONS = 16;

function randomHex(bytes) {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (b) =>
    b.toString(16).padStart(2, "0"),
  ).join("");
}

/// The address the internet sees this device from, as the application's
/// STUN servers report it: the first server-reflexive candidate a throwaway
/// connection gathers. Rejects when none arrives within `timeoutMs`, which
/// is what a network that blocks STUN, or a list with no STUN server, does.
export async function publicAddress(iceServers, { timeoutMs = ADDRESS_TIMEOUT_MS } = {}) {
  const stun = (iceServers ?? []).some((server) =>
    [server?.urls].flat().some((url) => typeof url === "string" && /^(stuns?|turns?):/.test(url)),
  );
  if (!stun) throw new Error("iceServers must name a STUN server to learn the network's address");
  const pc = new RTCPeerConnection({ iceServers });
  try {
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error("no STUN server reported this network's address")),
        timeoutMs,
      );
      pc.onicecandidate = ({ candidate }) => {
        if (!candidate) return;
        const address = reflexive(candidate);
        if (address) {
          clearTimeout(timer);
          resolve(address);
        }
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
function reflexive(candidate) {
  if (candidate.type) return candidate.type === "srflx" ? candidate.address : null;
  // An engine without the parsed fields: `candidate:<f> <c> <p> <prio> <addr> <port> typ srflx …`.
  const parts = candidate.candidate.split(" ");
  return parts[7] === "srflx" ? parts[4] : null;
}

/// The devices behind this network's address for one application, through
/// relays: each NearbyRoom announces itself, lists the others, and links
/// over `carrier` (a WebRtcCarrier) to one its person picks.
///
/// `admit({ peer, label })` decides every offer this device receives; it is
/// required, so a page that opens a room has said who it lets in.
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
  #closed = false;
  // key -> { label, seenAt }
  #peers = new Map();
  // session -> { answer, connection, freshUntilMs, refused }
  #sessions = new Map();
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
  /// answers or no relay confirms the room.
  async open() {
    const network = this.#network ?? (await publicAddress(this.#iceServers));
    this.#room = await this.#node.nearbyOpen(network, this.#label);
    const id = this.#room.id;
    this.#channel = new RoomChannel(
      {
        subscription: (sub) => this.#node.nearbySubscription(id, sub),
        open: (text) => this.#node.nearbyOpenMessage(id, text),
      },
      this.#relays,
      (opened) => this.#heard(opened),
      { keepAliveMs: KEEP_ALIVE_MS },
    );
    try {
      await this.#channel.open();
    } catch (error) {
      this.close();
      throw error;
    }
    await this.#announce();
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
    if (!this.#room) throw new Error("open the room first");
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
      const publish = async () =>
        this.#channel.publish(await this.#node.nearbyOffer(this.#room.id, peer, session, offer.code));
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

  /// Stops announcing and answering. Links already made stay up.
  close() {
    if (this.#closed) return;
    this.#closed = true;
    clearInterval(this.#timer);
    this.#channel?.close();
    for (const entry of this.#sessions.values()) clearTimeout(entry.timer);
    this.#sessions.clear();
    if (this.#room) this.#node.nearbyClose(this.#room.id).catch(() => {});
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
    if (!known && this.#peers.size >= MAX_PEERS) return;
    this.#peers.set(peer, { label, seenAt: Date.now() });
    if (!known || known.label !== label) this.#changed();
  }

  async #offered({ peer, label, session, code, freshUntilMs }) {
    const known = this.#sessions.get(session);
    if (known) {
      this.#hold(session, known, freshUntilMs);
      if (known.answer) this.#channel.publish(known.answer);
      return;
    }
    if (this.#sessions.size >= MAX_SESSIONS) return;
    const entry = { answer: null, connection: null, freshUntilMs: -Infinity, timer: undefined };
    this.#sessions.set(session, entry);
    this.#hold(session, entry, freshUntilMs);
    try {
      if (!(await this.#admit({ peer, label }))) return;
      if (this.#closed) return;
      entry.connection = await this.#carrier.answer(code);
      entry.answer = await this.#node.nearbyAnswer(this.#room.id, peer, session, entry.connection.code);
      this.#channel.publish(entry.answer);
    } catch {
      entry.connection?.close();
      this.#forget(session, entry);
    }
  }

  // Keeps `entry` until its newest offer is no longer fresh, so a relay's
  // late repeat reaches the session it belongs to (W11).
  #hold(session, entry, freshUntilMs) {
    if (freshUntilMs <= entry.freshUntilMs) return;
    entry.freshUntilMs = freshUntilMs;
    clearTimeout(entry.timer);
    entry.timer = setTimeout(() => this.#forget(session, entry), Math.max(0, freshUntilMs - Date.now()));
  }

  #forget(session, entry) {
    clearTimeout(entry.timer);
    if (this.#sessions.get(session) === entry) this.#sessions.delete(session);
  }

  #changed() {
    this.dispatchEvent(new CustomEvent("peers", { detail: this.peers }));
  }
}
