// A stand-in for navigator.bluetooth whose devices are SDK peripherals
// played by real nodes, for the browser tests. No engine runs Web Bluetooth
// without a person at the chooser, so the carrier is handed this instead,
// and everything past the GATT calls (verification, the hello, the fragment
// codec, the session) is the real engine on both sides.
//
// A peripheral here does what the shipped ones do: it serves the Device id
// and Identity values its node gives it, takes a hello, attributes Message
// writes to the central the hello named and nothing before, and notifies
// the fragments its node queues for that central. It cannot verify a hello
// on its own (the browser is never a peripheral, so the engine has no call
// for it), so it is told the central's address and has its node check the
// hello against that.

const SERVICE = "6e400001-b5a3-f393-e0a9-e50e24dcca9e";
const MESSAGE = "6e400002-b5a3-f393-e0a9-e50e24dcca9e";
const DEVICE_ID = "6e400003-b5a3-f393-e0a9-e50e24dcca9e";
const IDENTITY = "6e400004-b5a3-f393-e0a9-e50e24dcca9e";
const APP_TAG = "6e400005-b5a3-f393-e0a9-e50e24dcca9e";
const HELLO = "6e400006-b5a3-f393-e0a9-e50e24dcca9e";

const view = (bytes) => new DataView(Uint8Array.from(bytes).buffer);

class Characteristic extends EventTarget {
  constructor(uuid, { value = null, onWrite = null } = {}) {
    super();
    this.uuid = uuid;
    this.value = value === null ? null : view(value);
    this.notifying = false;
    this.writes = [];
    this.onWrite = onWrite;
  }

  async readValue() {
    if (this.value === null) throw new Error("not readable");
    return this.value;
  }

  async writeValueWithResponse(bytes) {
    this.writes.push(Uint8Array.from(bytes));
    await this.onWrite?.(Uint8Array.from(bytes), { response: true });
  }

  async writeValueWithoutResponse(bytes) {
    this.writes.push(Uint8Array.from(bytes));
    await this.onWrite?.(Uint8Array.from(bytes), { response: false });
  }

  async startNotifications() {
    this.notifying = true;
    return this;
  }

  notify(bytes) {
    if (!this.notifying) return;
    this.value = view(bytes);
    this.dispatchEvent(new Event("characteristicvaluechanged"));
  }
}

class Service {
  constructor(characteristics) {
    this.uuid = SERVICE;
    this.characteristics = characteristics;
  }

  async getCharacteristics() {
    return this.characteristics;
  }
}

class Device extends EventTarget {
  constructor(name, services, { onDisconnect } = {}) {
    super();
    this.id = crypto.randomUUID();
    this.name = name;
    this.services = services;
    const device = this;
    this.gatt = {
      connected: false,
      device,
      async connect() {
        this.connected = true;
        return this;
      },
      disconnect() {
        if (!this.connected) return;
        this.connected = false;
        onDisconnect?.();
        setTimeout(() => device.dispatchEvent(new Event("gattserverdisconnected")), 0);
      },
      async getPrimaryServices(uuid) {
        return uuid === SERVICE ? device.services : [];
      },
    };
  }
}

/// A device whose one instance of the service is `node`, a peripheral that
/// expects the central at `centralAddress`. `options.servedId` overrides
/// the Device id it serves (to serve one its identity does not prove);
/// `options.extraInstances` puts other instances ahead of it;
/// `options.noHello` serves no Hello characteristic; `options.refuseHello`
/// rejects the hello write; `options.dropOnHello` disconnects as the hello
/// arrives, mid-handshake.
export async function peripheral(node, centralAddress, options = {}) {
  const encoder = new TextEncoder();
  let central = null;
  // Every write to Message or Hello, in the order they arrived.
  const order = [];
  const message = new Characteristic(MESSAGE, {
    onWrite: async (bytes) => {
      order.push("message");
      // Before a hello the shipped peripherals cannot attribute a write.
      if (central) await node.bleFragmentReceived(central, bytes);
    },
  });
  // A peripheral's stack answers one write before the next, so two hellos
  // in flight are verified and linked one after the other.
  let helloQueue = Promise.resolve();
  const hello = new Characteristic(HELLO, {
    onWrite: (bytes) => {
      const handled = helloQueue.then(async () => {
        order.push("hello");
        if (options.refuseHello) throw new DOMException("hello refused", "NotSupportedError");
        if (options.dropOnHello) {
          device.gatt.disconnect();
          return;
        }
        const named = await node.bleVerify(encoder.encode(centralAddress), bytes);
        await node.bleLinkUp(named, 244);
        central = named;
      });
      helloQueue = handled.catch(() => {});
      return handled;
    },
  });
  // The node plays a peripheral here, never a central, so its sink is this
  // fixture's to take.
  node.setBluetoothSink(null);
  node.setBluetoothSink({
    write: (peer, bytes) => {
      if (peer === central) message.notify(bytes);
    },
    close: () => {},
  });
  const ours = new Service([
    message,
    new Characteristic(DEVICE_ID, { value: encoder.encode(options.servedId ?? node.address) }),
    new Characteristic(IDENTITY, { value: await node.bleHello() }),
    ...(options.tag ? [new Characteristic(APP_TAG, { value: options.tag })] : []),
    ...(options.noHello ? [] : [hello]),
  ]);
  const device = new Device(`peripheral ${node.address.slice(0, 12)}`, [...(options.extraInstances ?? []), ours], {
    onDisconnect: () => {
      if (central) node.bleLinkDown(central);
      central = null;
    },
  });
  device.message = message;
  device.hello = hello;
  device.order = order;
  return device;
}

/// An instance of the service that another SDK application on the same
/// phone publishes: tagged with `tag`, serving `node`'s identity.
export async function foreignInstance(node, tag) {
  const encoder = new TextEncoder();
  return new Service([
    new Characteristic(MESSAGE),
    new Characteristic(DEVICE_ID, { value: encoder.encode(node.address) }),
    new Characteristic(IDENTITY, { value: await node.bleHello() }),
    new Characteristic(APP_TAG, { value: tag }),
    new Characteristic(HELLO),
  ]);
}

/// navigator.bluetooth with `devices` in range: the chooser picks the first
/// that offers the service the filter names.
export class FakeBluetooth {
  constructor(devices) {
    this.devices = devices;
    this.requests = [];
  }

  async requestDevice(options) {
    this.requests.push(options);
    const wanted = options.filters?.flatMap((f) => f.services ?? []) ?? [];
    const found = this.devices.find((d) => d.services.length > 0 && wanted.includes(SERVICE));
    if (!found) throw new DOMException("no device", "NotFoundError");
    return found;
  }

  async getDevices() {
    return this.devices;
  }
}
