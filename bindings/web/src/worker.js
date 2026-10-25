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
// - Links have one of two owners. The tab carrier lives here; every other
//   carrier (WebRTC, which exists only on the main thread) lives on the page,
//   which opens a link by message and gets its frames back by message. The
//   engine cannot tell them apart, and the page's carriers obey the same
//   rules: the bytes are opaque, and every close reaches the other end.
// - The Nostr transport's sockets live here too, beside the engine they
//   feed, when the application gives the node relays (W12). They are the
//   application's relays: nothing here names one.
// - Storage opens before the engine does (W8). A durable node holds its
//   account's lock and its four log handles until it stops or the worker
//   ends, and the store key is zeroed once the engine has it.
// - A trap is fatal (W5). A Rust panic aborts the module, and JavaScript sees
//   it as a catchable WebAssembly.RuntimeError, so a worker that caught it and
//   ticked on would drive a module whose state stopped mid-update. The worker
//   says so once and stops.

import init, {
  WebEngine,
  storageNamespace,
  signalNewRoom,
  signalSubscription,
  signalEvent,
  signalOpen,
  parseInvite,
  NearbyRoomKeys,
  bleAppTag,
} from "../pkg/offline_protocol_web.js";
import { openDurableStore } from "./store.js";
import { TabCarrier } from "./tab-carrier.js";
import { NostrRelays, checkRelays } from "./nostr-relays.js";

const TICK_MS = 100;

let engine = null;
let carrier = null;
let nostr = null;
let store = null;
let startedAppId = null;
// Open nearby rooms, by id (W13). A page holds a few at most.
const nearby = new Map();
let nextNearby = 1;
const MAX_NEARBY_ROOMS = 4;
let timer = null;
let dead = null;
// Links whose carrier is on the page.
const pageLinks = new Set();

// Stops everything after a trap. The page rejects every call in flight and
// every later one; the module is not called again.
function die(error) {
  dead = String(error?.message ?? error);
  clearInterval(timer);
  try {
    carrier?.close();
    nostr?.close();
    store?.close();
  } catch {
    // The carriers and the store are JavaScript and do not trap.
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
    if (pageLinks.delete(link)) post({ kind: "linkClosed", link });
    else carrier?.engineClosed(link);
  }
  for (let frame = engine.nextOutbound(); frame; frame = engine.nextOutbound()) {
    const link = frame.link;
    const data = frame.data;
    frame.free();
    if (pageLinks.has(link)) self.postMessage({ kind: "frame", link, data }, [data.buffer]);
    else carrier?.send(link, data);
  }
  // Bluetooth fragments go to the page, which holds the GATT connections
  // (W14). Transferred, as a carrier frame is.
  for (let next = engine.bleNextFragment(); next; next = engine.bleNextFragment()) {
    const peer = next.peer;
    const data = next.data;
    next.free();
    self.postMessage({ kind: "bleFragment", peer, data }, [data.buffer]);
  }
  if (nostr?.live) {
    for (let next = engine.nostrNextEvent(); next !== undefined; next = engine.nostrNextEvent()) {
      nostr.publish(JSON.parse(next));
    }
    for (let next = engine.nostrNextQuery(); next !== undefined; next = engine.nostrNextQuery()) {
      nostr.query(JSON.parse(next));
    }
  }
  for (let event = engine.pollEvent(); event !== undefined; event = engine.pollEvent()) {
    post({ kind: "event", event: JSON.parse(event) });
  }
}

// What the relay sockets call. Each runs the engine, so each flushes: an
// answer to a relay leaves on the call that produced it, and a fatal trap in
// any of them ends the worker as a trap in a tick does.
function guarded(run) {
  return (...args) => {
    if (dead) return undefined;
    try {
      const result = run(...args);
      flush();
      return result;
    } catch (error) {
      if (trapped(error)) die(error);
      else post({ kind: "error", message: String(error?.message ?? error) });
      return undefined;
    }
  };
}

function relaysFor() {
  return {
    // Not `guarded`: it returns text rather than running the engine, and the
    // driver treats a throw as a relay lost. A trap is still fatal here, as
    // everywhere: without this the driver would take it for a lost relay and
    // call the stopped module again on every redial.
    subscription: (id) => {
      try {
        return engine.nostrSubscription(id);
      } catch (error) {
        if (trapped(error)) die(error);
        throw error;
      }
    },
    event: guarded((text) => engine.nostrRelayEvent(text)),
    status: guarded((live) => engine.nostrStatusChanged(live)),
    confirmed: guarded((messageId) => engine.nostrConfirmSent(messageId)),
    failed: guarded((messageId, reason) => engine.nostrSendFailed(messageId, reason)),
    completed: guarded((queryId) => engine.nostrQueryCompleted(queryId)),
    // A relay the node cannot use reaches the page as an `error` event.
    warn: (text) => post({ kind: "error", message: text }),
  };
}

function post(message) {
  self.postMessage(message);
}

function nearbyRoom(id) {
  const room = nearby.get(id);
  if (!room) throw new Error("that nearby room is closed");
  return room;
}

const handlers = {
  async start({ appId, profile, storage = "durable", tabs = true, instance, wasm, nostr: relays, ble = false }) {
    if (engine) throw new Error("this worker already runs a node");
    const nostrRelays = relays === undefined ? undefined : checkRelays(relays.relays);
    // A host that cannot serve the module beside this file (an editor
    // plugin, an extension) hands over its bytes or a compiled module.
    await init(wasm === undefined ? undefined : { module_or_path: wasm });
    // Nothing is kept from a start that fails: no engine, and no lock or
    // handle, so the page can start the node again once the cause is gone.
    let address;
    const next = WebEngine.withOptions(
      appId,
      profile,
      relays !== undefined,
      relays?.coldContact ?? true,
      relays?.usernameDiscovery ?? false,
      Boolean(ble),
    );
    try {
      if (storage === "durable") {
        store = await openDurableStore(storageNamespace(appId, profile));
        try {
          address = next.initializeDurable(store.key, ...store.handles);
        } finally {
          store.key.fill(0);
        }
      } else if (storage === "memory") {
        address = next.initialize();
      } else {
        throw new Error(`unknown storage ${storage}; expected "durable" or "memory"`);
      }
      next.start();
    } catch (error) {
      if (!trapped(error)) next.free();
      store?.close();
      store = null;
      throw error;
    }
    engine = next;
    startedAppId = appId;
    if (tabs) {
      carrier = new TabCarrier(
        appId,
        {
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
        },
        instance,
      );
    }
    if (nostrRelays) {
      nostr = new NostrRelays(nostrRelays, relaysFor());
      nostr.open();
    }
    timer = setInterval(tick, TICK_MS);
    flush();
    return address;
  },
  openLink() {
    const link = engine.openLink();
    pageLinks.add(link);
    return link;
  },
  linkReceived({ link, bytes }) {
    if (!pageLinks.has(link)) return "unknown link";
    const reason = engine.linkReceived(link, bytes);
    if (reason !== undefined) pageLinks.delete(link);
    return reason;
  },
  closeLink({ link }) {
    if (pageLinks.delete(link)) engine.closeLink(link);
  },
  stats() {
    const stats = engine.storeStats();
    return stats === undefined ? null : JSON.parse(stats);
  },
  sendMessage({ to, content, ephemeral = false }) {
    return engine.sendMessage(to, content, Boolean(ephemeral));
  },
  createGroup({ name }) {
    return JSON.parse(engine.createGroup(name));
  },
  inviteToGroup({ groupId, member }) {
    engine.inviteToGroup(groupId, member);
  },
  removeFromGroup({ groupId, member }) {
    engine.removeFromGroup(groupId, member);
  },
  groupInfo({ groupId }) {
    const info = engine.groupInfo(groupId);
    return info === undefined ? null : JSON.parse(info);
  },
  // Signalling through Nostr (W11). The page holds the relay sockets; these
  // build and open what crosses them.
  signalNewRoom() {
    return signalNewRoom();
  },
  signalSubscription({ room, subscriptionId }) {
    return signalSubscription(room, subscriptionId);
  },
  signalEvent({ room, role, session, code }) {
    return signalEvent(room, role, session, code);
  },
  signalOpen({ room, message }) {
    const opened = signalOpen(room, message);
    return opened === undefined ? null : JSON.parse(opened);
  },
  // Sessions and the directory (W12). They work on any node; reaching a
  // device by its address alone, and the directory, need relays.
  establishSecureSession({ address }) {
    return engine.establishSecureSession(address);
  },
  resolveUsername({ username }) {
    return engine.resolveUsername(username);
  },
  createInvite({ petname, sign = false }) {
    return engine.createInvite(petname ?? undefined, Boolean(sign));
  },
  parseInvite({ blob }) {
    return JSON.parse(parseInvite(blob));
  },
  // A nearby room's half in the worker (W13). The page holds the relay
  // sockets and the STUN lookup; these derive, seal and open.
  nearbyOpen({ network, label = "" }) {
    if (nearby.size >= MAX_NEARBY_ROOMS) throw new Error(`${MAX_NEARBY_ROOMS} nearby rooms are already open`);
    const room = new NearbyRoomKeys(startedAppId, network, label);
    const id = nextNearby++;
    nearby.set(id, room);
    return { id, key: room.key };
  },
  nearbySubscription({ id, subscriptionId }) {
    return nearbyRoom(id).subscription(subscriptionId);
  },
  nearbyPresence({ id }) {
    return nearbyRoom(id).presence();
  },
  nearbyOffer({ id, to, session, code }) {
    return nearbyRoom(id).offer(to, session, code);
  },
  nearbyAnswer({ id, to, session, code }) {
    return nearbyRoom(id).answer(to, session, code);
  },
  nearbyOpenMessage({ id, message }) {
    const opened = nearby.get(id)?.open(message);
    return opened === undefined ? null : JSON.parse(opened);
  },
  nearbyClose({ id }) {
    nearby.get(id)?.free();
    nearby.delete(id);
  },
  // The Bluetooth slot's half in the worker (W14). The page holds the GATT
  // connection and moves bytes; these verify, announce and reassemble.
  bleHello() {
    return engine.bleHello();
  },
  bleAppTag() {
    return bleAppTag(startedAppId);
  },
  bleVerify({ deviceId, identity }) {
    return engine.bleVerify(deviceId, identity);
  },
  bleLinkUp({ peer, maxPayload }) {
    engine.bleLinkUp(peer, maxPayload);
  },
  bleFragmentReceived({ peer, bytes }) {
    engine.bleFragmentReceived(peer, bytes);
  },
  bleLinkDown({ peer }) {
    engine.bleLinkDown(peer);
  },
  sendGroupMessage({ groupId, content, ephemeral = false }) {
    return JSON.parse(engine.sendGroupMessage(groupId, content, Boolean(ephemeral)));
  },
  stop() {
    clearInterval(timer);
    carrier?.close();
    carrier = null;
    nostr?.close();
    nostr = null;
    for (const room of nearby.values()) room.free();
    nearby.clear();
    engine.stop();
    flush();
    pageLinks.clear();
    store?.close();
    store = null;
  },
};

self.onmessage = async ({ data: { id, op, args } }) => {
  if (dead) return post({ kind: "reply", id, error: `the node stopped: ${dead}` });
  try {
    const handler = handlers[op];
    if (!handler) throw new Error(`unknown operation ${op}`);
    if (op !== "start" && !engine) throw new Error("start the node first");
    const result = await handler(args ?? {});
    if (op === "openLink") {
      // The page learns the link id from this reply, and drops a frame for a
      // link it does not know, so the reply goes ahead of the link's preamble.
      post({ kind: "reply", id, result });
      return flush();
    }
    if (engine) flush();
    post({ kind: "reply", id, result });
  } catch (error) {
    post({ kind: "reply", id, error: String(error?.message ?? error) });
    if (trapped(error)) die(error);
  }
};
