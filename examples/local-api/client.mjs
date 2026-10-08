#!/usr/bin/env node
// A client of the local API in Node, with no dependencies: the built-in
// WebSocket (Node 22 or later; it is global there) and the file system.
//
// The built-in client accepts only ws: and wss: URLs, so this example uses
// the service's loopback TCP carrier with its per-launch token; start the
// service with `--tcp PORT --token-file PATH`. A client that must use the
// Unix socket needs a WebSocket library that can dial one.
//
// What it shows, in order: hello with an application id and the token,
// subscribe, send_message and the event that settles it (message_delivered,
// or message_failed when the engine gives up; a message_undeliverable on
// the way is printed and waited past), a document edit
// (data.map_set, then data.doc_json to read it back), and the one-shot
// idiom (a fresh connection: hello, one request, close). An engine refusal
// is printed as the JSON-RPC code and the engine's variant name; any other
// failure as its message.
//
// Usage:
//   node examples/local-api/client.mjs --port 7800 --token-file /run/example/token \
//       --app-id notes [--to off1... --content "hi"] [--space notes-1 --doc todo]

import { readFileSync } from "node:fs";

if (typeof WebSocket === "undefined") {
  console.error("Node 22 or later is required: this example uses the built-in WebSocket");
  process.exit(1);
}

function parseArgs(argv) {
  const args = { appId: "notes", content: "hello from Node", space: "notes-1", doc: "todo", deliverTimeoutMs: 30000 };
  for (let i = 0; i < argv.length; i += 2) {
    const [flag, value] = [argv[i], argv[i + 1]];
    if (flag === "--port") args.port = Number(value);
    else if (flag === "--token-file") args.tokenFile = value;
    else if (flag === "--app-id") args.appId = value;
    else if (flag === "--to") args.to = value;
    else if (flag === "--content") args.content = value;
    else if (flag === "--space") args.space = value;
    else if (flag === "--doc") args.doc = value;
    else if (flag === "--deliver-timeout") args.deliverTimeoutMs = Number(value) * 1000;
    else throw new Error(`unknown flag ${flag}`);
  }
  if (!args.port || !args.tokenFile) throw new Error("--port and --token-file are required");
  return args;
}

class RpcFailure extends Error {
  constructor(error) {
    super(error.message);
    this.code = error.code;
    this.variant = error.data?.variant ?? null;
  }
}

// One connection: call() resolves with the matching response; events queue
// up. When the socket closes, every pending call and event wait is rejected
// rather than left hanging.
class Client {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map(); // id -> { resolve, reject }
    this.events = [];
    this.observers = []; // called with every event as it arrives
    this.waiters = []; // { matches, resolve, reject }
    socket.addEventListener("message", (frame) => {
      const message = JSON.parse(frame.data);
      if ("id" in message) {
        const waiter = this.pending.get(message.id);
        this.pending.delete(message.id);
        if (waiter) message.error ? waiter.reject(new RpcFailure(message.error)) : waiter.resolve(message.result);
        return;
      }
      this.events.push(message.params); // a notification: the event object itself
      for (const observe of this.observers) observe(message.params);
      this.waiters = this.waiters.filter((w) => !(w.matches(message.params) && (w.resolve(message.params), true)));
    });
    const fail = (reason) => {
      for (const waiter of [...this.pending.values(), ...this.waiters]) waiter.reject(new Error(reason));
      this.pending.clear();
      this.waiters = [];
    };
    socket.addEventListener("close", (event) => fail(`connection closed (${event.code})`), { once: true });
    socket.addEventListener("error", () => fail("connection error"), { once: true });
  }

  static async open(port) {
    const socket = new WebSocket(`ws://127.0.0.1:${port}/`);
    await new Promise((resolve, reject) => {
      socket.addEventListener("open", resolve, { once: true });
      socket.addEventListener("error", () => reject(new Error(`cannot connect to port ${port}`)), { once: true });
    });
    return new Client(socket);
  }

  call(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    });
  }

  // The next event with one of these types whose fields match; earlier ones count too.
  nextEvent(types, fields = {}, timeoutMs = 30000) {
    const wanted = Array.isArray(types) ? types : [types];
    const matches = (e) => wanted.includes(e.type) && Object.entries(fields).every(([k, v]) => e[k] === v);
    const found = this.events.find(matches);
    if (found) return Promise.resolve(found);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`no ${wanted.join("/")} within ${timeoutMs} ms`)), timeoutMs);
      const settle = (fn) => (value) => (clearTimeout(timer), fn(value));
      this.waiters.push({ matches, resolve: settle(resolve), reject: settle(reject) });
    });
  }

  close() {
    this.socket.close();
  }
}

async function hello(client, appId, token) {
  return client.call("hello", { app_id: appId, client: "client.mjs", token });
}

async function oneShot(port, token, appId, method) {
  const client = await Client.open(port);
  try {
    await hello(client, appId, token);
    return await client.call(method);
  } finally {
    client.close();
  }
}

async function main() {
  let client = null;
  try {
    const args = parseArgs(process.argv.slice(2));
    const token = readFileSync(args.tokenFile, "ascii").trim(); // written 0600 at launch
    client = await Client.open(args.port);
    console.log(JSON.stringify({ hello: await hello(client, args.appId, token) }));
    await client.call("subscribe", {
      types: ["message_received", "message_delivered", "message_failed", "message_undeliverable"],
    });
    if (args.to) {
      const messageId = await client.call("send_message", { recipient: args.to, content: args.content, priority: "Medium" });
      // message_undeliverable settles nothing: the recipient is away, the
      // engine keeps the message, may say so again on every probe, and
      // delivers it when the recipient is back. Print each as a status line
      // and keep waiting for the event that settles the id.
      const status = (e) => {
        if (e.type === "message_undeliverable" && e.message_id === messageId) console.log(JSON.stringify({ undeliverable: e }));
      };
      client.events.forEach(status);
      client.observers.push(status);
      const outcome = await client.nextEvent(["message_delivered", "message_failed"], { message_id: messageId }, args.deliverTimeoutMs);
      client.observers.splice(client.observers.indexOf(status), 1);
      console.log(JSON.stringify({ [outcome.type.replace(/^message_/, "")]: outcome }));
    }
    // create_doc is a no-op for a document that exists. A written value is
    // tagged with its kind; doc_json reads back plain JSON.
    await client.call("data.create_doc", { space_id: args.space, doc_id: args.doc });
    await client.call("data.map_set", {
      space_id: args.space, doc_id: args.doc, collection: "fields", key: "edited_by",
      value_json: JSON.stringify({ kind: "text", value: args.appId }),
    });
    const document = JSON.parse(await client.call("data.doc_json", { space_id: args.space, doc_id: args.doc }));
    console.log(JSON.stringify({ document }));
    console.log(JSON.stringify({ one_shot: await oneShot(args.port, token, args.appId, "local_address") }));
  } catch (error) {
    if (error instanceof RpcFailure) {
      console.error(JSON.stringify({ error: { code: error.code, variant: error.variant, message: error.message } }));
    } else {
      console.error(JSON.stringify({ error: { message: error.message } }));
    }
    return 1;
  } finally {
    client?.close();
  }
  return 0;
}

process.exitCode = await main();
