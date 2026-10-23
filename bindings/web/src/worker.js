// The engine's home: one dedicated worker per node.
//
// Invariants (docs/bridges/web.md):
// - The engine lives here and nowhere else. The page talks to it by message,
//   so nothing the page does can re-enter the engine while it holds its lock.
// - This file moves bytes. What a link's bytes mean (the identity preamble,
//   the frame boundary, one link per address) is decided in Rust, by
//   WebEngine.linkReceived and WebEngine.nextOutbound.
// - One tick every 100 ms, as both mobile bridges run: process, then close
//   every carrier the engine closed, then hand every queued frame to its
//   carrier, then every queued event to the page.
// - A trap is fatal (W5). A Rust panic aborts the module, and JavaScript sees
//   it as a catchable WebAssembly.RuntimeError, so a worker that caught it and
//   ticked on would drive a module whose state stopped mid-update. The worker
//   says so once and stops.

import init, { WebEngine } from "../pkg/offline_protocol_web.js";
import { TabCarrier } from "./tab-carrier.js";

const TICK_MS = 100;

let engine = null;
let carrier = null;
let timer = null;
let dead = null;

// Stops everything after a trap. The page rejects every call in flight and
// every later one; the module is not called again.
function die(error) {
  dead = String(error?.message ?? error);
  clearInterval(timer);
  try {
    carrier?.close();
  } catch {
    // The carrier is JavaScript and does not trap; nothing to add.
  }
  post({ kind: "fatal", message: dead });
  self.close();
}

const trapped = (error) => error instanceof WebAssembly.RuntimeError;

function tick() {
  try {
    engine.process();
    flush();
  } catch (error) {
    if (trapped(error)) return die(error);
    post({ kind: "error", message: String(error) });
    flush();
  }
}

// Frames and events produced by a call are delivered before the call returns
// to the page, so a send does not wait for the next tick to leave.
function flush() {
  for (let link = engine.nextClosed(); link !== undefined; link = engine.nextClosed()) {
    carrier.engineClosed(link);
  }
  for (let frame = engine.nextOutbound(); frame; frame = engine.nextOutbound()) {
    const link = frame.link;
    const data = frame.data;
    frame.free();
    carrier.send(link, data);
  }
  for (let event = engine.pollEvent(); event !== undefined; event = engine.pollEvent()) {
    post({ kind: "event", event: JSON.parse(event) });
  }
}

function post(message) {
  self.postMessage(message);
}

const handlers = {
  async start({ appId, profile }) {
    if (engine) throw new Error("this worker already runs a node");
    await init();
    engine = new WebEngine(appId, profile);
    const address = engine.initialize();
    engine.start();
    carrier = new TabCarrier(appId, {
      open: () => engine.openLink(),
      received: (link, bytes) => {
        const reason = engine.linkReceived(link, bytes);
        flush();
        return reason;
      },
      closed: (link) => {
        engine.closeLink(link);
        flush();
      },
    });
    timer = setInterval(tick, TICK_MS);
    flush();
    return address;
  },
  sendMessage({ to, content }) {
    return engine.sendMessage(to, content);
  },
  createGroup({ name }) {
    return JSON.parse(engine.createGroup(name));
  },
  inviteToGroup({ groupId, member }) {
    engine.inviteToGroup(groupId, member);
  },
  sendGroupMessage({ groupId, content }) {
    return JSON.parse(engine.sendGroupMessage(groupId, content));
  },
  stop() {
    clearInterval(timer);
    carrier?.close();
    engine?.stop();
    flush();
  },
};

self.onmessage = async ({ data: { id, op, args } }) => {
  if (dead) return post({ kind: "reply", id, error: `the node stopped: ${dead}` });
  try {
    const handler = handlers[op];
    if (!handler) throw new Error(`unknown operation ${op}`);
    if (op !== "start" && !engine) throw new Error("start the node first");
    const result = await handler(args ?? {});
    if (engine) flush();
    post({ kind: "reply", id, result });
  } catch (error) {
    post({ kind: "reply", id, error: String(error?.message ?? error) });
    if (trapped(error)) die(error);
  }
};
