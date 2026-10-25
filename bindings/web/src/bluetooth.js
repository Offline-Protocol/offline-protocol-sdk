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
// - Writes to one link go one at a time, in order. A link whose write fails
//   is closed: a fragment lost mid-message leaves the peer reassembling
//   something that never completes.

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
  // peer -> { device, message, queue, onDisconnect }
  #links = new Map();
  #tag = null;

  constructor(node, { bluetooth = globalThis.navigator?.bluetooth } = {}) {
    super();
    if (!bluetooth) throw new Error("this browser has no Web Bluetooth");
    this.#node = node;
    this.#bluetooth = bluetooth;
    node.setBluetoothSink((peer, bytes) => this.#write(peer, bytes));
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
    const device = await this.#bluetooth.requestDevice({ filters: [{ services: [SERVICE] }] });
    return this.connect(device);
  }

  /// Devices this origin was granted before, where the browser remembers
  /// them, to `connect` without the chooser.
  async known() {
    return (await this.#bluetooth.getDevices?.()) ?? [];
  }

  /// Links `device`. Resolves to the address its identity proves; rejects,
  /// with the connection closed, when it does not verify.
  async connect(device) {
    const server = await device.gatt.connect();
    try {
      const service = await this.#choose(await server.getPrimaryServices(SERVICE));
      const chars = new Map((await characteristics(service)).map((c) => [c.uuid.toLowerCase(), c]));
      const deviceId = chars.get(DEVICE_ID);
      const identity = chars.get(IDENTITY);
      const message = chars.get(MESSAGE);
      if (!deviceId || !identity || !message) {
        throw new Error("the device does not serve the SDK's handshake characteristics");
      }
      const peer = await this.#node.bleVerify(
        bytesOf(await deviceId.readValue()),
        bytesOf(await identity.readValue()),
      );
      this.#links.get(peer)?.device.gatt.disconnect();
      // Notifications that arrive before the engine has the link wait for it.
      let ready;
      const link = {
        device,
        message,
        queue: Promise.resolve(),
        onDisconnect: null,
        inbound: new Promise((resolve) => (ready = resolve)),
      };
      message.addEventListener("characteristicvaluechanged", ({ target }) => {
        const bytes = bytesOf(target.value);
        link.inbound = link.inbound
          .then(() => this.#node.bleFragmentReceived(peer, bytes))
          .catch(() => {});
      });
      await message.startNotifications();
      const hello = chars.get(HELLO);
      if (hello) {
        // A refused hello is not a reason to drop the link (the chapter's
        // rule); the peripheral then resolves the central as it can.
        await hello.writeValueWithResponse(await this.#node.bleHello()).catch(() => {});
      }
      link.onDisconnect = () => this.#lost(peer, link);
      device.addEventListener("gattserverdisconnected", link.onDisconnect);
      this.#links.set(peer, link);
      const payload = server.maxWriteWithoutResponseSize ?? FLOOR_PAYLOAD;
      await this.#node.bleLinkUp(peer, payload);
      ready();
      this.dispatchEvent(new CustomEvent("connection", { detail: { peer, state: "connected" } }));
      return peer;
    } catch (error) {
      device.gatt.disconnect();
      throw error;
    }
  }

  /// Closes the link to `peer`.
  close(peer) {
    this.#links.get(peer)?.device.gatt.disconnect();
  }

  /// Closes every link.
  closeAll() {
    for (const peer of this.peers) this.close(peer);
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

  #write(peer, bytes) {
    const link = this.#links.get(peer);
    if (!link) return;
    link.queue = link.queue
      .then(() => link.message.writeValueWithoutResponse(bytes))
      .catch(() => link.device.gatt.disconnect());
  }

  #lost(peer, link) {
    link.device.removeEventListener("gattserverdisconnected", link.onDisconnect);
    if (this.#links.get(peer) !== link) return;
    this.#links.delete(peer);
    this.#node.bleLinkDown(peer);
    this.dispatchEvent(new CustomEvent("connection", { detail: { peer, state: "closed" } }));
  }
}
