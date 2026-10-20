#!/usr/bin/env node
/**
 * Binding-level pins for the per-recipient media send queue surface
 * (`getMediaQueueDepth`, plus the three `media_queue_*` event types).
 *
 * The queue's actual serialisation / timeout / hard-cap behaviour is pinned
 * on the Rust side (see `media_queue_*` tests in
 * `crates/offline-protocol/src/protocol/tests/mod.rs`). What this harness
 * holds is the shape at the binding boundary: the TS wrapper hands the
 * recipient to the native module, resolves to the number the native side
 * returns, and the three queue events parse into the typed shapes an app
 * subscribes to.
 *
 * Same stub-native-module shape as `one-shot-hold.test.js`.
 */
'use strict';

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const Module = require('node:module');
const os = require('node:os');
const path = require('node:path');

const PACKAGE_DIR = path.resolve(__dirname, '..');
const EVENT_CHANNEL = 'OfflineProtocol_Event';

function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-queue-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

let nativeOverrides = {};
let stickyOnSubscribe = [];
const buses = [];

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      return (...args) => {
        const override = nativeOverrides[method];
        return override ? override(...args) : Promise.resolve();
      };
    },
  }
);

class StubNativeEventEmitter {
  constructor() {
    this.listeners = new Map();
    buses.push(this);
  }

  addListener(channel, handler) {
    let handlers = this.listeners.get(channel);
    if (!handlers) {
      handlers = new Set();
      this.listeners.set(channel, handlers);
    }
    handlers.add(handler);

    if (channel === EVENT_CHANNEL && stickyOnSubscribe.length > 0) {
      const flushing = stickyOnSubscribe;
      stickyOnSubscribe = [];
      flushing.forEach((event) => handler({ eventJson: JSON.stringify(event) }));
    }

    return {
      remove: () => {
        handlers.delete(handler);
      },
    };
  }

  emit(event) {
    const handlers = this.listeners.get(EVENT_CHANNEL);
    if (!handlers) return;
    [...handlers].forEach((handler) => handler({ eventJson: JSON.stringify(event) }));
  }
}

const realLoad = Module._load;
Module._load = function loadWithReactNativeStub(request) {
  if (request === 'react-native') {
    return {
      NativeModules: { OfflineProtocolModule: nativeModule },
      NativeEventEmitter: StubNativeEventEmitter,
    };
  }
  return realLoad.apply(this, arguments);
};

const realConsole = { log: console.log, warn: console.warn, error: console.error };
let captured = { warn: [], error: [] };
function captureConsole() {
  captured = { warn: [], error: [] };
  console.log = () => {};
  console.warn = (...args) => captured.warn.push(args.join(' '));
  console.error = (...args) => captured.error.push(args.join(' '));
}
function releaseConsole() {
  Object.assign(console, realConsole);
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

let OfflineProtocol;

function newSdk() {
  stickyOnSubscribe = [];
  const sdk = new OfflineProtocol({ appId: 'harness', profile: 'harness-profile' });
  return { sdk, bus: buses[buses.length - 1] };
}

// ---------------------------------------------------------------------------
// getMediaQueueDepth
// ---------------------------------------------------------------------------

test('getMediaQueueDepth passes the recipient through to the native module', async () => {
  const { sdk } = newSdk();
  let sawRecipient = null;
  nativeOverrides.getMediaQueueDepth = (recipient) => {
    sawRecipient = recipient;
    return Promise.resolve(0);
  };
  const depth = await sdk.getMediaQueueDepth('off1bob');
  assert.equal(sawRecipient, 'off1bob');
  assert.equal(depth, 0);
});

test('getMediaQueueDepth resolves to the number the native side returns', async () => {
  const { sdk } = newSdk();
  nativeOverrides.getMediaQueueDepth = () => Promise.resolve(7);
  const depth = await sdk.getMediaQueueDepth('off1alice');
  assert.equal(depth, 7);
});

test('getMediaQueueDepth surfaces a native rejection to the caller', async () => {
  const { sdk } = newSdk();
  nativeOverrides.getMediaQueueDepth = () =>
    Promise.reject(new Error('ERROR_MEDIA_QUEUE: Protocol not initialized'));
  await assert.rejects(
    () => sdk.getMediaQueueDepth('off1somebody'),
    /ERROR_MEDIA_QUEUE/
  );
});

// ---------------------------------------------------------------------------
// media_queue_* event wiring
// ---------------------------------------------------------------------------

test('media_queue_enqueued is routed to its typed listener', async () => {
  const { sdk, bus } = newSdk();
  const seen = [];
  sdk.on('media_queue_enqueued', (event) => seen.push(event));
  bus.emit({
    type: 'media_queue_enqueued',
    timestamp: 1,
    file_id: 'file_abc',
    recipient: 'off1bob',
    queue_depth: 2,
  });
  await settle();
  assert.equal(seen.length, 1);
  assert.equal(seen[0].file_id, 'file_abc');
  assert.equal(seen[0].recipient, 'off1bob');
  assert.equal(seen[0].queue_depth, 2);
});

test('media_queue_dequeued carries queue_depth and wait_ms', async () => {
  const { sdk, bus } = newSdk();
  const seen = [];
  sdk.on('media_queue_dequeued', (event) => seen.push(event));
  bus.emit({
    type: 'media_queue_dequeued',
    timestamp: 2,
    file_id: 'file_abc',
    recipient: 'off1bob',
    queue_depth: 1,
    wait_ms: 480,
  });
  await settle();
  assert.equal(seen.length, 1);
  assert.equal(seen[0].queue_depth, 1);
  assert.equal(seen[0].wait_ms, 480);
});

test('media_queue_timeout carries wait_ms', async () => {
  const { sdk, bus } = newSdk();
  const seen = [];
  sdk.on('media_queue_timeout', (event) => seen.push(event));
  bus.emit({
    type: 'media_queue_timeout',
    timestamp: 3,
    file_id: 'file_abc',
    recipient: 'off1bob',
    wait_ms: 300_000,
  });
  await settle();
  assert.equal(seen.length, 1);
  assert.equal(seen[0].file_id, 'file_abc');
  assert.equal(seen[0].wait_ms, 300_000);
});

test("the 'all' listener receives every queue event in arrival order", async () => {
  const { sdk, bus } = newSdk();
  const seen = [];
  sdk.on('all', (event) => seen.push(event.type));
  bus.emit({
    type: 'media_queue_enqueued',
    timestamp: 1,
    file_id: 'f1',
    recipient: 'off1bob',
    queue_depth: 1,
  });
  bus.emit({
    type: 'media_queue_dequeued',
    timestamp: 2,
    file_id: 'f1',
    recipient: 'off1bob',
    queue_depth: 0,
    wait_ms: 50,
  });
  bus.emit({
    type: 'media_queue_timeout',
    timestamp: 3,
    file_id: 'f2',
    recipient: 'off1bob',
    wait_ms: 300_000,
  });
  await settle();
  assert.deepEqual(seen, [
    'media_queue_enqueued',
    'media_queue_dequeued',
    'media_queue_timeout',
  ]);
});

// ---------------------------------------------------------------------------
// Serialisation shape: enqueue -> dequeue path for the same file_id
// ---------------------------------------------------------------------------

test('enqueue then dequeue arrive paired under the same file_id', async () => {
  // Pins the shape apps subscribe to for "my transfer is waiting" vs "my
  // transfer is now on the wire". The Rust-side guarantee is "every
  // enqueue reaches exactly one dequeue (or exactly one timeout)"; this
  // asserts the binding exposes both events under the same file_id so an
  // app can correlate them without a side channel.
  const { sdk, bus } = newSdk();
  const states = new Map();
  sdk.on('media_queue_enqueued', (event) => {
    states.set(event.file_id, { enqueued: true });
  });
  sdk.on('media_queue_dequeued', (event) => {
    const prior = states.get(event.file_id) || {};
    states.set(event.file_id, { ...prior, dequeued: true, wait_ms: event.wait_ms });
  });
  bus.emit({
    type: 'media_queue_enqueued',
    timestamp: 1,
    file_id: 'file_match',
    recipient: 'off1bob',
    queue_depth: 1,
  });
  bus.emit({
    type: 'media_queue_dequeued',
    timestamp: 2,
    file_id: 'file_match',
    recipient: 'off1bob',
    queue_depth: 0,
    wait_ms: 125,
  });
  await settle();
  const final = states.get('file_match');
  assert.deepEqual(final, { enqueued: true, dequeued: true, wait_ms: 125 });
});

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

(async () => {
  const outDir = compileSdk();
  try {
    ({ OfflineProtocol } = require(path.join(outDir, 'index.js')));

    let failed = 0;
    for (const { name, fn } of tests) {
      nativeOverrides = { isMlsInitialized: () => Promise.resolve(true) };
      stickyOnSubscribe = [];
      captureConsole();
      try {
        await fn();
        releaseConsole();
        realConsole.log(`  ok  ${name}`);
      } catch (error) {
        failed += 1;
        releaseConsole();
        realConsole.log(`  FAIL ${name}\n      ${error.message}`);
      }
    }

    realConsole.log(
      failed === 0
        ? `\n${tests.length} passed.`
        : `\n${failed} of ${tests.length} FAILED.`
    );
    process.exitCode = failed === 0 ? 0 : 1;
  } finally {
    releaseConsole();
    fs.rmSync(outDir, { recursive: true, force: true });
  }
})();
