// A minimal NIP-01 relay for the browser tests: REQ, EVENT, CLOSE over a
// WebSocket, filters on `kinds`, `since` and `#t`. It stores nothing, which is
// all an ephemeral signalling event needs, and verifies nothing: the pages'
// own code (signal.rs) is what refuses a forged event, and a relay that
// forwards anything is the stricter test of that. A client that connects on
// the path `/silent` is never answered: the relay that accepts a socket and
// never confirms a subscription. Node's standard library only, so the test
// runner installs nothing.
//
// Usage: node test/nostr-relay.mjs <port>

import { createServer } from "node:http";
import { createHash } from "node:crypto";

const port = Number(process.argv[2] ?? 8766);
const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const clients = new Set();

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
  if (Array.isArray(filter["#t"])) {
    const topics = (event.tags ?? []).filter((t) => t[0] === "t").map((t) => t[1]);
    if (!filter["#t"].some((t) => topics.includes(t))) return false;
  }
  return true;
}

class Client {
  subscriptions = new Map();
  #buffer = Buffer.alloc(0);
  #fragments = [];

  constructor(socket, silent) {
    this.socket = socket;
    this.silent = silent;
    socket.on("data", (chunk) => this.#read(chunk));
    socket.on("close", () => clients.delete(this));
    socket.on("error", () => clients.delete(this));
  }

  send(message) {
    if (!this.socket.destroyed) this.socket.write(frame(JSON.stringify(message)));
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
    if (type === "REQ") {
      const [id, ...filters] = rest;
      this.subscriptions.set(id, filters);
      this.send(["EOSE", id]);
    } else if (type === "CLOSE") {
      this.subscriptions.delete(rest[0]);
    } else if (type === "EVENT") {
      const event = rest[0];
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
  clients.add(new Client(socket, request.url === "/silent"));
});
server.listen(port, "127.0.0.1", () => console.log(`nostr test relay on ${port}`));
