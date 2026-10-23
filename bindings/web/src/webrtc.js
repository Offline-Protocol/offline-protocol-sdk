// Links between devices over WebRTC data channels, set up with no server.
//
// Invariants (docs/bridges/web.md, W9):
// - The carrier lives on the page. RTCPeerConnection exists only on the main
//   thread, so bytes cross to the node's worker by message, under a link id
//   the engine allocated (OfflineNode.openLink).
// - One reliable, ordered channel per peer. The engine's link is a byte
//   stream: the identity preamble must arrive before every frame, and a
//   frame's bytes in order. Bytes are cut into chunks of at most CHUNK_BYTES,
//   because every browser accepts a message that size and some refuse a
//   larger one, and the engine's stream reader joins them again. The
//   carrier never interprets a byte.
// - Signalling needs no network. Each side's description waits for ICE
//   gathering to finish, so one code carries every candidate, and the two
//   codes cross by whatever the people have: a QR code, a paste, a chat, or
//   a NostrSignaller room. With no ICE server (the default) the connection
//   is made on the local network or not at all; the application may name its
//   own STUN and TURN servers, and every candidate they yield goes in the
//   code.
// - A carrier address proves nothing. The connection only carries bytes; the
//   engine's preamble proves who is on the other end.
// - A connection closes on "failed" or "closed" only. "disconnected" may
//   recover on its own, and closing there would turn a Wi-Fi blip into two
//   new codes typed by hand. The page hears every state as a `connection`
//   event instead, so it can warn at "disconnected" (5 s after the last
//   answered check in Chrome) rather than look connected until "failed"
//   (15 s, when the engine reports the peer lost).
// - A connection reports nothing until offer() or answer() has handed the
//   page its object. A call that fails, or whose connection closed while it
//   gathered, rejects and reports nothing: an event naming an object the
//   page never received is one it cannot match to anything.

const CODE_PREFIX = "op1.";
const CHANNEL_ID = 0;
const CHUNK_BYTES = 16 * 1024;
const GATHER_MS = 5000;
// Above this many bytes buffered on the channel, sends wait for it to drain.
const HIGH_WATER_BYTES = 1024 * 1024;
// Above this many bytes waiting behind the channel, the peer is not keeping
// up and the connection is closed. The engine keeps every acknowledged
// message for a retry, so closing loses only what was never promised.
const MAX_BACKLOG_BYTES = 8 * 1024 * 1024;

async function compress(text) {
  const stream = new Blob([text]).stream().pipeThrough(new CompressionStream("deflate-raw"));
  const bytes = new Uint8Array(await new Response(stream).arrayBuffer());
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function decompress(code) {
  const base64 = code.replace(/-/g, "+").replace(/_/g, "/");
  const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
  return new Response(stream).text();
}

/// The description as a code: the SDP, every candidate included.
async function encode(description) {
  return CODE_PREFIX + (await compress(JSON.stringify({ t: description.type, s: description.sdp })));
}

async function decode(code, expected) {
  const text = code.trim();
  if (!text.startsWith(CODE_PREFIX)) throw new Error("not an Offline Protocol connection code");
  let parsed;
  try {
    parsed = JSON.parse(await decompress(text.slice(CODE_PREFIX.length)));
  } catch {
    throw new Error("the connection code is damaged or incomplete");
  }
  if (parsed?.t !== expected || typeof parsed.s !== "string") {
    throw new Error(`expected an ${expected} code`);
  }
  return { type: parsed.t, sdp: parsed.s };
}

function gathered(pc) {
  if (pc.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve) => {
    const timer = setTimeout(done, GATHER_MS);
    function done() {
      clearTimeout(timer);
      pc.removeEventListener("icegatheringstatechange", check);
      resolve();
    }
    function check() {
      if (pc.iceGatheringState === "complete") done();
    }
    pc.addEventListener("icegatheringstatechange", check);
  });
}

/// One connection toward one other device.
class Connection {
  #node;
  #pc;
  #channel;
  #link = null;
  #inbound = []; // bytes that arrived before the link id
  #outbound = Promise.resolve();
  // Bytes handed to #send and not yet on the channel.
  #backlog = 0;
  // Sends waiting for the channel to drain; close() releases them, as a
  // closed channel never fires bufferedamountlow.
  #draining = new Set();
  #closed = false;
  #onClose;
  #onState = null;
  #reported = null;

  constructor(node, iceServers, onClose) {
    this.#node = node;
    this.#onClose = onClose;
    this.#pc = new RTCPeerConnection({ iceServers });
    this.#channel = this.#pc.createDataChannel("offline-protocol", {
      negotiated: true,
      id: CHANNEL_ID,
      ordered: true,
    });
    this.#channel.binaryType = "arraybuffer";
    this.#channel.bufferedAmountLowThreshold = HIGH_WATER_BYTES / 2;
    this.#channel.onopen = () => this.#opened();
    this.#channel.onmessage = ({ data }) => this.#received(new Uint8Array(data));
    this.#channel.onclose = () => this.close();
    this.#pc.onconnectionstatechange = () => {
      const state = this.#pc.connectionState;
      if (state === "failed" || state === "closed") this.close();
      else if (state === "connected" || state === "disconnected") this.#report(state);
    };
  }

  /// Starts reporting to `onState`, beginning with the state the connection
  /// is already in, so a change before this call is not lost.
  listen(onState) {
    this.#onState = onState;
    const state = this.#pc.connectionState;
    if (state === "connected" || state === "disconnected") this.#report(state);
  }

  // Once per change: "connected", "disconnected", then "closed" exactly once.
  // Nothing before listen(). dispatchEvent never throws for a listener (the
  // browser reports the exception), so a throwing listener cannot stop this.
  #report(state) {
    if (this.#onState === null || this.#reported === state || this.#reported === "closed") return;
    this.#reported = state;
    this.#onState(state);
  }

  get peerConnection() {
    return this.#pc;
  }

  get closed() {
    return this.#closed;
  }

  async offer() {
    await this.#pc.setLocalDescription(await this.#pc.createOffer());
    await gathered(this.#pc);
    return encode(this.#pc.localDescription);
  }

  async answer(offerCode) {
    await this.#pc.setRemoteDescription(await decode(offerCode, "offer"));
    await this.#pc.setLocalDescription(await this.#pc.createAnswer());
    await gathered(this.#pc);
    return encode(this.#pc.localDescription);
  }

  async accept(answerCode) {
    await this.#pc.setRemoteDescription(await decode(answerCode, "answer"));
  }

  async #opened() {
    try {
      this.#link = await this.#node.openLink({
        send: (bytes) => this.#send(bytes),
        close: () => this.close({ engine: true }),
      });
    } catch {
      // The engine's link ceiling, or a node that ended.
      return this.close();
    }
    if (this.#closed) {
      this.#node.closeLink(this.#link);
      return;
    }
    for (const bytes of this.#inbound.splice(0)) this.#deliver(bytes);
  }

  #received(bytes) {
    if (this.#link === null) this.#inbound.push(bytes);
    else this.#deliver(bytes);
  }

  #deliver(bytes) {
    // The node's calls are answered in the order they were made, so bytes
    // reach the engine in the order they arrived.
    this.#node.linkReceived(this.#link, bytes).catch(() => this.close());
  }

  #send(bytes) {
    if (this.#closed) return;
    this.#backlog += bytes.length;
    if (this.#backlog > MAX_BACKLOG_BYTES) return this.close();
    // Chained, so a wait for the buffer to drain never reorders bytes.
    this.#outbound = this.#outbound.then(async () => {
      try {
        await this.#transmit(bytes);
      } finally {
        this.#backlog -= bytes.length;
      }
    });
  }

  async #transmit(bytes) {
    for (let at = 0; at < bytes.length && !this.#closed; at += CHUNK_BYTES) {
      if (this.#channel.bufferedAmount > HIGH_WATER_BYTES) {
        await new Promise((resolve) => {
          const done = () => {
            this.#draining.delete(done);
            resolve();
          };
          this.#draining.add(done);
          this.#channel.addEventListener("bufferedamountlow", done, { once: true });
        });
      }
      if (this.#channel.readyState !== "open") return;
      this.#channel.send(bytes.subarray(at, at + CHUNK_BYTES));
    }
  }

  /// Closes the connection. The other end sees its channel close, which is
  /// its signal that this side refused or left.
  close({ engine = false } = {}) {
    if (this.#closed) return;
    this.#closed = true;
    for (const done of [...this.#draining]) done();
    if (this.#link !== null && !engine) this.#node.closeLink(this.#link);
    this.#channel.close();
    this.#pc.close();
    this.#onClose(this);
    this.#report("closed");
  }
}

/// WebRTC links for one node. Each connection takes two codes: the side that
/// offers shows an offer code and reads back an answer code; the other side
/// reads the offer and shows its answer.
///
/// Every change of a connection's state is a `connection` event with
/// `{ state, connection }` as its detail: `state` is "connected",
/// "disconnected" (no answer to the last checks; it may recover) or "closed"
/// (once, at the end), and `connection` is the object `offer()` or `answer()`
/// resolved to, so a page with several devices can tell them apart. A call
/// that rejects reports nothing.
export class WebRtcCarrier extends EventTarget {
  #node;
  #iceServers;
  #connections = new Set();

  /// `iceServers` is RTCPeerConnection's list, for networks where devices
  /// cannot reach each other directly: a STUN server for addresses behind a
  /// NAT, a TURN server to relay. The default is none, so links stay on the
  /// local network and no server learns of them (W9).
  constructor(node, { iceServers = [] } = {}) {
    super();
    this.#node = node;
    this.#iceServers = iceServers;
  }

  /// Starts a connection. Resolves to `{ code, accept(answerCode), close() }`:
  /// show `code` to the other device, then pass its answer to `accept`.
  async offer() {
    const connection = this.#track(this.#connection());
    const code = await this.#code(connection, () => connection.offer());
    return this.#hand(connection, {
      code,
      accept: (answerCode) => connection.accept(answerCode),
      close: () => connection.close(),
    });
  }

  /// Answers another device's offer. Resolves to `{ code, close() }`: show
  /// `code` to the device that offered.
  async answer(offerCode) {
    const connection = this.#track(this.#connection());
    const code = await this.#code(connection, () => connection.answer(offerCode));
    return this.#hand(connection, { code, close: () => connection.close() });
  }

  /// Closes every connection.
  close() {
    for (const connection of [...this.#connections]) connection.close();
  }

  #connection() {
    return new Connection(this.#node, this.#iceServers, (c) => this.#connections.delete(c));
  }

  // The connection's code, or a rejection that closes it. A connection that
  // closed while it gathered (carrier.close(), or the page leaving) rejects
  // too, rather than resolve to a code nobody can use.
  async #code(connection, make) {
    let code;
    try {
      code = await make();
    } catch (error) {
      connection.close();
      throw error;
    }
    if (connection.closed) throw new Error("the connection closed before its code was ready");
    return code;
  }

  // Hands `handle` to the page: from here every change of state is an event
  // naming it.
  #hand(connection, handle) {
    connection.listen((state) =>
      this.dispatchEvent(new CustomEvent("connection", { detail: { state, connection: handle } })),
    );
    return handle;
  }

  #track(connection) {
    this.#connections.add(connection);
    return connection;
  }
}
