// A Bluetooth LE carrier over Web Bluetooth: the page as a GATT central (W14).
//
// Invariants (docs/bridges/web.md):
// - The page is a central and nothing else. It cannot advertise, so nothing
//   discovers it, and it serves no GATT, so a peripheral names it only by the
//   hello it writes. Every link starts from a person picking a device in the
//   browser's chooser, inside a user gesture, or from a device this origin
//   was granted before.
// - This file moves bytes. It reads the peripheral's Device id and Identity
//   and hands both to the engine, which verifies the identity, derives the
//   address and compares it with the Device id exactly; the peer is announced
//   under what the engine returns, never under what the peripheral served.
//   Fragments are cut and reassembled in Rust, by the codec the conformance
//   vectors check.
// - The order is the GATT contract's (docs/spec/ble-framing.md): choose the
//   instance, read and verify, subscribe to Message, write Hello, then tell
//   the engine the link is up. Nothing is written to Message before the
//   hello, so the peripheral can attribute every fragment.
// - The hello is required here, though the chapter makes it optional. The
//   chapter's leniency rests on the peripheral dialling an unnamed central
//   back, and nothing can dial a browser: a device without Hello, or one
//   that refuses the write, would be announced as a neighbour that neither
//   receives nor sends. Such a device is refused.
// - A disconnect is watched from the moment the GATT connects. A device that
//   drops during the handshake is refused, never announced; one that drops
//   later is reported lost exactly once, also when the page closed a link
//   whose server was already gone and no event follows.
// - Writes to one link go one at a time, in order. A link whose write fails
//   is closed: a fragment lost mid-message leaves the peer reassembling
//   something that never completes.
// - One carrier per node. The node has one outbound Bluetooth sink, and a
//   second carrier is refused rather than left to take the first one's
//   fragments.

const SERVICE = "6e400001-b5a3-f393-e0a9-e50e24dcca9e";
const MESSAGE = "6e400002-b5a3-f393-e0a9-e50e24dcca9e";
const DEVICE_ID = "6e400003-b5a3-f393-e0a9-e50e24dcca9e";
const IDENTITY = "6e400004-b5a3-f393-e0a9-e50e24dcca9e";
const APP_TAG = "6e400005-b5a3-f393-e0a9-e50e24dcca9e";
const HELLO = "6e400006-b5a3-f393-e0a9-e50e24dcca9e";

// The write payload assumed when the browser does not report one: the
// fragment floor every binding falls back to (BLE_MAX_FRAGMENT_SIZE).
const FLOOR_PAYLOAD = 185;

const bytesOf = (view) => new Uint8Array(view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength));

async function characteristics(service) {
  try {
    return await service.getCharacteristics();
  } catch {
    return [];
  }
}

/// Links this page's node to phones and boards running the SDK as a
/// Bluetooth LE peripheral, over Web Bluetooth. Chromium only. Start the node
/// with `ble: true`.
///
/// A `connection` event carries `{ peer, state }` as each link comes up
/// ("connected") and goes ("closed").
export class BluetoothCarrier extends EventTarget {
  #node;
  #bluetooth;
  // peer -> link: { device, peer, message, queue, inbound, linked, announced, gone, onDisconnect, onNotify }
  #links = new Map();
  #tag = null;
  #released = false;
  // device -> the handshake in flight for it, which a second connect joins.
  #connecting = new Map();

  constructor(node, { bluetooth = globalThis.navigator?.bluetooth } = {}) {
    super();
    if (!bluetooth) throw new Error("this browser has no Web Bluetooth");
    this.#node = node;
    this.#bluetooth = bluetooth;
    // Throws when another carrier holds the node's sink.
    node.setBluetoothSink({
      write: (peer, bytes) => this.#write(peer, bytes),
      close: () => this.#nodeStopped(),
    });
  }

  /// Whether this browser exposes Web Bluetooth at all.
  static get supported() {
    return Boolean(globalThis.navigator?.bluetooth);
  }

  /// The addresses linked now.
  get peers() {
    return [...this.#links.keys()];
  }

  /// Opens the browser's chooser for devices offering the SDK's service, and
  /// links the one the person picks. Must run inside a user gesture (a
  /// click), as the browser requires. Resolves to the peer's address.
  async request() {
    this.#usable();
    const device = await this.#bluetooth.requestDevice({ filters: [{ services: [SERVICE] }] });
    return this.connect(device);
  }

  /// Devices this origin was granted before, where the browser remembers
  /// them, to `connect` without the chooser. A remembered device that is out
  /// of range may leave `connect` waiting on the browser's own timeout.
  async known() {
    return (await this.#bluetooth.getDevices?.()) ?? [];
  }

  /// Links `device`. Resolves to the address its identity proves; rejects,
  /// with the connection closed, when it does not verify, serves no Hello,
  /// refuses the hello, or disconnects before the link is up.
  async connect(device) {
    this.#usable();
    // Already linked on a live connection: the browser hands back the same
    // device, and the same characteristics, for one physical device, so a
    // second handshake would subscribe a second time and every fragment
    // would reach the engine once per handshake.
    const held = [...this.#links.values()].find((l) => l.device === device);
    if (held && device.gatt.connected) return held.peer;
    // A handshake already under way for this device is joined, not raced:
    // the engine holds one verification per address, so a second would
    // fail its link-up and take the first one's link down with it.
    const inflight = this.#connecting.get(device);
    if (inflight) return inflight;
    const handshake = this.#handshake(device).finally(() => this.#connecting.delete(device));
    this.#connecting.set(device, handshake);
    return handshake;
  }

  async #handshake(device) {
    const server = await device.gatt.connect();
    let ready;
    const link = {
      device,
      peer: null,
      message: null,
      queue: Promise.resolve(),
      // Notifications that arrive before the engine has the link wait for it.
      inbound: new Promise((resolve) => (ready = resolve)),
      // In #links, so the engine may have it.
      linked: false,
      // The page was told it is connected.
      announced: false,
      gone: false,
      onDisconnect: null,
      onNotify: null,
    };
    // Watched before the first await on the server, so a drop anywhere in
    // the handshake is seen.
    link.onDisconnect = () => {
      link.gone = true;
      if (link.linked) this.#lost(link);
      else this.#detach(link);
    };
    device.addEventListener("gattserverdisconnected", link.onDisconnect);
    const goneCheck = () => {
      if (link.gone || !device.gatt.connected) throw new Error("the device disconnected during the handshake");
    };
    try {
      const service = await this.#choose(await server.getPrimaryServices(SERVICE));
      const chars = new Map((await characteristics(service)).map((c) => [c.uuid.toLowerCase(), c]));
      const deviceId = chars.get(DEVICE_ID);
      const identity = chars.get(IDENTITY);
      const message = chars.get(MESSAGE);
      const hello = chars.get(HELLO);
      if (!deviceId || !identity || !message) {
        throw new Error("the device does not serve the SDK's handshake characteristics");
      }
      if (!hello) {
        // Nothing can dial a browser back, so the hello is the only way the
        // peripheral can attribute a write (W14).
        throw new Error("the device serves no Hello, and a browser can be named no other way");
      }
      const peer = await this.#node.bleVerify(
        bytesOf(await deviceId.readValue()),
        bytesOf(await identity.readValue()),
      );
      link.peer = peer;
      link.message = message;
      link.onNotify = ({ target }) => {
        const bytes = bytesOf(target.value);
        link.inbound = link.inbound
          .then(() => this.#node.bleFragmentReceived(peer, bytes))
          .catch(() => {});
      };
      message.addEventListener("characteristicvaluechanged", link.onNotify);
      await message.startNotifications();
      try {
        await hello.writeValueWithResponse(await this.#node.bleHello());
      } catch (error) {
        goneCheck();
        throw new Error(`the device refused the hello: ${error?.message ?? error}`);
      }
      goneCheck();
      // An earlier link to the same address is replaced. The engine keeps
      // the peer across the swap, so it is not reported lost.
      const previous = this.#links.get(peer);
      this.#links.set(peer, link);
      link.linked = true;
      if (previous) this.#retire(previous);
      // Proposed for the characteristic (web-bluetooth #672) and shipped by
      // no browser yet, so the floor is what runs today.
      const payload = message.maxWriteWithoutResponseSize ?? FLOOR_PAYLOAD;
      // In #links before the engine hears of it: the fragments its link-up
      // queues reach the sink ahead of this call's reply.
      await this.#node.bleLinkUp(peer, payload);
      goneCheck();
      // A connect to another device proving the same address replaced this
      // link while the engine took it; the newer one is the one announced.
      if (this.#links.get(peer) !== link) return peer;
      ready();
      link.announced = true;
      this.dispatchEvent(new CustomEvent("connection", { detail: { peer, state: "connected" } }));
      return peer;
    } catch (error) {
      if (link.linked) this.#lost(link);
      else this.#detach(link);
      if (device.gatt.connected) device.gatt.disconnect();
      throw error;
    }
  }

  /// Closes the link to `peer`, and reports it lost at once, whether or not
  /// the browser still has a connection to close.
  close(peer) {
    const link = this.#links.get(peer);
    if (link) this.#drop(link);
  }

  /// Closes every link.
  closeAll() {
    for (const link of [...this.#links.values()]) this.#drop(link);
  }

  /// Closes every link and gives the node's Bluetooth sink back, so another
  /// carrier can take it. This carrier links nothing afterwards.
  release() {
    if (this.#released) return;
    this.closeAll();
    this.#released = true;
    this.#node.setBluetoothSink(null);
  }

  // The chapter's instance choice: with one instance, that one; with several,
  // among those serving both Device id and Identity, the first whose App tag
  // is this application's, else the only untagged one, else the first.
  async #choose(services) {
    if (services.length === 0) throw new Error("the device does not offer the SDK's service");
    if (services.length === 1) return services[0];
    this.#tag ??= await this.#node.bleAppTag();
    const candidates = [];
    for (const service of services) {
      const chars = new Map((await characteristics(service)).map((c) => [c.uuid.toLowerCase(), c]));
      if (!chars.has(DEVICE_ID) || !chars.has(IDENTITY)) continue;
      let tag = null;
      try {
        tag = chars.has(APP_TAG) ? bytesOf(await chars.get(APP_TAG).readValue()) : null;
      } catch {
        tag = null;
      }
      candidates.push({ service, tag: tag?.length ? tag : null });
    }
    if (candidates.length === 0) return services[0];
    const ours = candidates.find(({ tag }) => tag && tag.length === this.#tag.length && tag.every((b, i) => b === this.#tag[i]));
    if (ours) return ours.service;
    const untagged = candidates.filter(({ tag }) => tag === null);
    if (untagged.length === 1) return untagged[0].service;
    return candidates[0].service;
  }

  #usable() {
    if (this.#released) throw new Error("this Bluetooth carrier was released");
  }

  #write(peer, bytes) {
    const link = this.#links.get(peer);
    if (!link) return;
    link.queue = link.queue
      .then(() => link.message.writeValueWithoutResponse(bytes))
      .catch(() => this.#drop(link));
  }

  // Disconnects `link` and reports it lost now, not on an event that a
  // server already gone never sends.
  #drop(link) {
    if (link.device.gatt.connected) link.device.gatt.disconnect();
    this.#lost(link);
  }

  // Removes both of a link's listeners. Every path that ends a link comes
  // here: a notification listener left on a characteristic the browser hands
  // out again would deliver each fragment once more per handshake.
  #detach(link) {
    link.device.removeEventListener("gattserverdisconnected", link.onDisconnect);
    if (link.onNotify) link.message?.removeEventListener("characteristicvaluechanged", link.onNotify);
  }

  // A link replaced by a newer one to the same address: closed without
  // telling the engine, which keeps the peer on the newer link.
  #retire(link) {
    this.#detach(link);
    if (link.announced) {
      this.dispatchEvent(new CustomEvent("connection", { detail: { peer: link.peer, state: "closed" } }));
    }
    link.announced = false;
    if (link.device !== this.#links.get(link.peer)?.device && link.device.gatt.connected) {
      link.device.gatt.disconnect();
    }
  }

  #lost(link) {
    this.#detach(link);
    if (this.#links.get(link.peer) !== link) return;
    this.#links.delete(link.peer);
    this.#node.bleLinkDown(link.peer);
    if (link.announced) {
      link.announced = false;
      this.dispatchEvent(new CustomEvent("connection", { detail: { peer: link.peer, state: "closed" } }));
    }
  }

  // The node stopped: its worker is gone, so the links carry nothing.
  #nodeStopped() {
    for (const link of [...this.#links.values()]) {
      this.#detach(link);
      if (link.device.gatt.connected) link.device.gatt.disconnect();
      this.#links.delete(link.peer);
      if (link.announced) {
        this.dispatchEvent(new CustomEvent("connection", { detail: { peer: link.peer, state: "closed" } }));
      }
    }
    this.#released = true;
  }
}
