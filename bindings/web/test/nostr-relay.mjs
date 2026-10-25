// A minimal NIP-01 relay for the browser tests: REQ, EVENT, CLOSE over a
// WebSocket, filters on `kinds`, `since`, `limit` and any `#` tag. It keeps
// every event that is not ephemeral (kinds 20000 to 29999, which signalling
// uses) and replaces an addressable one (30000 to 39999) by kind, author and
// `d` tag, so a REQ is answered with what it holds before its EOSE, as the
// Nostr transport's key packages, directory records and messages need. It
// verifies nothing: the engine is what refuses a forged event, and a relay
// that forwards anything is the stricter test of that. A client that connects on
// the path `/silent` is never answered: the relay that accepts a socket and
// never confirms a subscription. One on `/once` confirms its first
// subscription and then goes quiet, answering and forwarding nothing more:
// a socket the network lost without closing it. On `/first/<token>` the
// first socket for a token is served and every later one is silent: a relay
// that answered before a network change and now accepts and ignores a
// redial. On `/reject` every event is refused with `OK false`, and on
// `/closed` every subscription is ended with `CLOSED`, as a relay that wants
// authentication does. On `/slow` everything is answered, SLOW_MS late: a
// relay that answers after the others. Node's standard library only, so the
// test runner installs nothing.
//
// Usage: node test/nostr-relay.mjs <port>

import { createServer } from "node:http";
import { createHash } from "node:crypto";

const port = Number(process.argv[2] ?? 8766);
const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const SLOW_MS = 600;
const clients = new Set();
// Every event kept, oldest first.
const stored = [];
// The `/first/<token>` paths a socket has already connected on.
const served = new Set();

function frame(text) {
  const payload = Buffer.from(text);
  const length = payload.length;
  let header;
  if (length < 126) {
    header = Buffer.from([0x81, length]);
  } else if (length < 65536) {
    header = Buffer.alloc(4);
    header[0] = 0x81;
    header[1] = 126;
    header.writeUInt16BE(length, 2);
  } else {
    header = Buffer.alloc(10);
    header[0] = 0x81;
    header[1] = 127;
    header.writeBigUInt64BE(BigInt(length), 2);
  }
  return Buffer.concat([header, payload]);
}

function matches(filter, event) {
  if (Array.isArray(filter.kinds) && !filter.kinds.includes(event.kind)) return false;
  if (typeof filter.since === "number" && event.created_at < filter.since) return false;
  for (const [key, wanted] of Object.entries(filter)) {
    if (!key.startsWith("#") || !Array.isArray(wanted)) continue;
    const held = (event.tags ?? []).filter((t) => t[0] === key.slice(1)).map((t) => t[1]);
    if (!wanted.some((value) => held.includes(value))) return false;
  }
  return true;
}

const dTag = (event) => (event.tags ?? []).find((t) => t[0] === "d")?.[1];

function keep(event) {
  const kind = event?.kind;
  if (typeof kind !== "number" || (kind >= 20000 && kind < 30000)) return;
  if (kind >= 30000 && kind < 40000) {
    const at = stored.findIndex(
      (held) => held.kind === kind && held.pubkey === event.pubkey && dTag(held) === dTag(event),
    );
    if (at !== -1) {
      if (stored[at].created_at > event.created_at) return;
      stored.splice(at, 1);
    }
  }
  stored.push(event);
}

// What a REQ's filters match among the kept events: newest first, each
// filter cut to its own `limit`.
function history(filters) {
  const found = new Map();
  for (const filter of filters) {
    let matched = stored.filter((event) => matches(filter, event)).reverse();
    if (typeof filter.limit === "number") matched = matched.slice(0, filter.limit);
    for (const event of matched) found.set(event.id, event);
  }
  return [...found.values()];
}

class Client {
  subscriptions = new Map();
  #buffer = Buffer.alloc(0);
  #fragments = [];

  constructor(socket, mode) {
    this.socket = socket;
    this.mode = mode;
    socket.on("data", (chunk) => this.#read(chunk));
    socket.on("close", () => clients.delete(this));
    socket.on("error", () => clients.delete(this));
  }

  get silent() {
    return this.mode === "silent" || this.mode === "quiet";
  }

  send(message) {
    if (this.mode === "quiet") return;
    const write = () => {
      if (!this.socket.destroyed) this.socket.write(frame(JSON.stringify(message)));
    };
    if (this.mode === "slow") setTimeout(write, SLOW_MS);
    else write();
  }

  #read(chunk) {
    this.#buffer = Buffer.concat([this.#buffer, chunk]);
    for (;;) {
      if (this.#buffer.length < 2) return;
      const fin = (this.#buffer[0] & 0x80) !== 0;
      const opcode = this.#buffer[0] & 0x0f;
      const masked = (this.#buffer[1] & 0x80) !== 0;
      let length = this.#buffer[1] & 0x7f;
      let at = 2;
      if (length === 126) {
        if (this.#buffer.length < 4) return;
        length = this.#buffer.readUInt16BE(2);
        at = 4;
      } else if (length === 127) {
        if (this.#buffer.length < 10) return;
        length = Number(this.#buffer.readBigUInt64BE(2));
        at = 10;
      }
      const maskAt = at;
      if (masked) at += 4;
      if (this.#buffer.length < at + length) return;
      const payload = Buffer.from(this.#buffer.subarray(at, at + length));
      if (masked) {
        for (let i = 0; i < payload.length; i++) payload[i] ^= this.#buffer[maskAt + (i % 4)];
      }
      this.#buffer = this.#buffer.subarray(at + length);
      if (opcode === 0x8) {
        this.socket.end(Buffer.from([0x88, 0]));
        return;
      }
      if (opcode === 0x9) {
        this.socket.write(Buffer.concat([Buffer.from([0x8a, payload.length]), payload]));
        continue;
      }
      if (opcode === 0x1 || opcode === 0x0) {
        this.#fragments.push(payload);
        if (fin) {
          const text = Buffer.concat(this.#fragments).toString("utf8");
          this.#fragments = [];
          this.#message(text);
        }
      }
    }
  }

  #message(text) {
    let message;
    try {
      message = JSON.parse(text);
    } catch {
      return this.send(["NOTICE", "unreadable message"]);
    }
    if (this.silent) return;
    const [type, ...rest] = message;
    if (type === "REQ" && this.mode === "closed") {
      this.send(["CLOSED", rest[0], "auth-required: this relay serves members only"]);
    } else if (type === "REQ") {
      const [id, ...filters] = rest;
      this.subscriptions.set(id, filters);
      for (const event of history(filters)) this.send(["EVENT", id, event]);
      this.send(["EOSE", id]);
      if (this.mode === "once") this.mode = "quiet";
    } else if (type === "CLOSE") {
      this.subscriptions.delete(rest[0]);
    } else if (type === "EVENT" && this.mode === "reject") {
      this.send(["OK", rest[0]?.id ?? "", false, "blocked: this relay takes nothing"]);
    } else if (type === "EVENT") {
      const event = rest[0];
      keep(event);
      this.send(["OK", event?.id ?? "", true, ""]);
      for (const client of clients) {
        for (const [id, filters] of client.subscriptions) {
          if (filters.some((filter) => matches(filter, event))) client.send(["EVENT", id, event]);
        }
      }
    }
  }
}

const server = createServer((_, response) => response.writeHead(426).end());
server.on("upgrade", (request, socket) => {
  const key = request.headers["sec-websocket-key"];
  if (!key) return socket.destroy();
  const accept = createHash("sha1").update(key + GUID).digest("base64");
  socket.write(
    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
      `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
  );
  let mode = {
    "/silent": "silent",
    "/once": "once",
    "/reject": "reject",
    "/closed": "closed",
    "/slow": "slow",
  }[request.url] ?? "open";
  if (request.url.startsWith("/first/")) {
    mode = served.has(request.url) ? "silent" : "open";
    served.add(request.url);
  }
  clients.add(new Client(socket, mode));
});
server.listen(port, "127.0.0.1", () => console.log(`nostr test relay on ${port}`));
