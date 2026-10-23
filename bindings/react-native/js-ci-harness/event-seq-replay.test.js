#!/usr/bin/env node
/**
 * Behavioral tests for native event gap recovery (#514): the TypeScript layer
 * pauses dispatch on a skipped `seq`, fetches the missing events back with
 * `replayEvents`, and reports only what the native module could not serve.
 *
 * Drives the *real compiled* `OfflineProtocol` class against a stubbed native
 * module, so these assert what the code does rather than what it says. The
 * Rust text-guard `react_native_one_shot_event_set_matches_native` pins that
 * each piece of the mechanism is still present; only this can tell you they
 * still add up to correct delivery.
 *
 * See README.md for why the package has no other JS test setup.
 */
'use strict';

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const Module = require('node:module');
const os = require('node:os');
const path = require('node:path');

const PACKAGE_DIR = path.resolve(__dirname, '..');

/** The channel `setupEventSubscription` subscribes to. */
const EVENT_CHANNEL = 'OfflineProtocol_Event';

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

/**
 * Compiles `src/` into a scratch directory and returns it.
 *
 * Deliberately not the package's own `lib/`: that is gitignored and routinely
 * stale, and a harness that silently tests last month's build is worse than
 * no harness. Compiling here also means a failure is always about the source
 * in front of you.
 */
function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-harness-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

// ---------------------------------------------------------------------------
// The native stub
// ---------------------------------------------------------------------------

/** Per-test method overrides; anything absent resolves to `undefined`. */
let nativeOverrides = {};

/**
 * Events the stub hands over from inside `addListener`, mirroring Android's
 * sticky flush — which runs from the native `addListener` that
 * `NativeEventEmitter.addListener` invokes, i.e. synchronously inside the SDK
 * constructor, before the app can possibly have called `on(...)`. Reproducing
 * that timing is the entire point of this harness.
 */
let stickyOnSubscribe = [];

/** Every emitter the SDK has constructed, newest last. */
const buses = [];

/** Method names the stub pretends not to have, per test. */
const hiddenMethods = new Set();

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      if (hiddenMethods.has(method)) return undefined;
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

  /** A live native emit, after the SDK is already subscribed. */
  emit(event, seq) {
    const handlers = this.listeners.get(EVENT_CHANNEL);
    if (!handlers) return;
    const payload = { eventJson: JSON.stringify(event) };
    if (typeof seq === 'number') payload.seq = seq;
    [...handlers].forEach((handler) => handler(payload));
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

// ---------------------------------------------------------------------------
// Scaffolding
// ---------------------------------------------------------------------------

const realConsole = { log: console.log, warn: console.warn, error: console.error };

function captureConsole() {
  console.log = () => {};
  console.warn = () => {};
  console.error = () => {};
}

function releaseConsole() {
  Object.assign(console, realConsole);
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

let OfflineProtocol;

/** A message event numbered `seq`, distinguishable by its id. */
const received = (seq) => ({ type: 'message_received', timestamp: seq, message_id: `m${seq}`, sender: 'bob', content: `c${seq}` });
const delivered = (seq) => ({ type: 'message_delivered', timestamp: seq, message_id: `d${seq}`, latency_ms: 1, hop_count: 0, transport: 'ble' });

/**
 * A native ring holding `events` by seq, served by the stubbed
 * `replayEvents`. Records every call it answers.
 */
function serveRing(events, { delayMs = 0, reject = false } = {}) {
  const calls = [];
  nativeOverrides.replayEvents = (fromSeq, toSeq) => {
    calls.push([fromSeq, toSeq]);
    const served = Object.entries(events)
      .map(([seq, event]) => ({ seq: Number(seq), eventJson: JSON.stringify(event) }))
      .filter((entry) => entry.seq >= fromSeq && entry.seq <= toSeq);
    return new Promise((resolve, rejectFn) =>
      setTimeout(() => (reject ? rejectFn(new Error('boom')) : resolve(served)), delayMs)
    );
  };
  return calls;
}

/** An SDK whose app listens to everything, recording what it saw. */
function listeningSdk() {
  const sdk = new OfflineProtocol({ appId: 'harness', profile: 'harness-profile' });
  const bus = buses[buses.length - 1];
  const seen = [];
  sdk.on('all', (event) => seen.push(event));
  return { sdk, bus, seen };
}

const ids = (seen) => seen.filter((e) => e.type !== 'diagnostic').map((e) => e.message_id);
const diagnostics = (seen, message) =>
  seen.filter((e) => e.type === 'diagnostic' && e.message === message);

// ---------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------

test('a skipped number is fetched back and dispatched in its place', async () => {
  const calls = serveRing({ 2: received(2) });
  const { bus, seen } = listeningSdk();
  bus.emit(received(0), 0);
  bus.emit(received(1), 1);
  bus.emit(received(3), 3);
  await settle();
  await settle();
  assert.deepEqual(calls, [[2, 2]]);
  assert.deepEqual(ids(seen), ['m0', 'm1', 'm2', 'm3']);
  assert.equal(diagnostics(seen, 'native_event_gap').length, 0);
  const replayed = diagnostics(seen, 'native_event_replayed');
  assert.equal(replayed.length, 1);
  assert.deepEqual(replayed[0].context, { from: 2, to: 2, recovered: 1, missing: 0 });
  assert.equal(replayed[0].level, 'info');
});

test('a lost settlement on the sender side is recovered too', async () => {
  serveRing({ 1: delivered(1) });
  const { bus, seen } = listeningSdk();
  bus.emit(delivered(0), 0);
  bus.emit(delivered(2), 2);
  await settle();
  await settle();
  assert.deepEqual(ids(seen), ['d0', 'd1', 'd2']);
});

test('events arriving while the replay is in flight wait behind it, in order', async () => {
  serveRing({ 1: received(1) }, { delayMs: 5 });
  const { bus, seen } = listeningSdk();
  bus.emit(received(0), 0);
  bus.emit(received(2), 2);
  bus.emit(received(3), 3);
  assert.deepEqual(ids(seen), ['m0'], 'nothing past the gap goes out before the replay answers');
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(ids(seen), ['m0', 'm1', 'm2', 'm3']);
});

test('a second gap while the first replay is in flight is recovered by the same pause', async () => {
  const calls = serveRing({ 1: received(1), 4: received(4) }, { delayMs: 5 });
  const { bus, seen } = listeningSdk();
  bus.emit(received(0), 0);
  bus.emit(received(2), 2);
  bus.emit(received(3), 3);
  bus.emit(received(5), 5);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(calls, [[1, 1], [4, 4]]);
  assert.deepEqual(ids(seen), ['m0', 'm1', 'm2', 'm3', 'm4', 'm5']);
});

// ---------------------------------------------------------------------------
// What cannot be recovered is still reported
// ---------------------------------------------------------------------------

test('a number the ring no longer holds is reported as a gap where it fell', async () => {
  serveRing({ 2: received(2) });
  const { bus, seen } = listeningSdk();
  bus.emit(received(0), 0);
  bus.emit(received(3), 3);
  await settle();
  await settle();
  assert.deepEqual(ids(seen), ['m0', 'm2', 'm3']);
  const gaps = diagnostics(seen, 'native_event_gap');
  assert.equal(gaps.length, 1);
  assert.deepEqual(gaps[0].context, { expected: 1, received: 2, missing: 1 });
  const order = seen.map((e) => (e.type === 'diagnostic' ? e.message : e.message_id));
  assert.ok(
    order.indexOf('native_event_gap') > order.indexOf('m0') &&
      order.indexOf('native_event_gap') < order.indexOf('m2'),
    `the gap is reported where it fell: ${order.join(',')}`
  );
});

test('a ring that serves nothing reports the whole range and keeps going', async () => {
  serveRing({});
  const { bus, seen } = listeningSdk();
  bus.emit(received(0), 0);
  bus.emit(received(4), 4);
  await settle();
  await settle();
  assert.deepEqual(ids(seen), ['m0', 'm4']);
  const gaps = diagnostics(seen, 'native_event_gap');
  assert.equal(gaps.length, 1);
  assert.deepEqual(gaps[0].context, { expected: 1, received: 4, missing: 3 });
  assert.equal(diagnostics(seen, 'native_event_replayed').length, 0);
});

test('a rejected replay reports the gap and releases what waited', async () => {
  serveRing({ 1: received(1) }, { reject: true });
  const { bus, seen } = listeningSdk();
  bus.emit(received(0), 0);
  bus.emit(received(2), 2);
  await settle();
  await settle();
  assert.deepEqual(ids(seen), ['m0', 'm2']);
  assert.equal(diagnostics(seen, 'native_event_gap').length, 1);
});

test('a module that never answers is given up on after the timeout', async () => {
  nativeOverrides.replayEvents = () => new Promise(() => {});
  const { bus, seen } = listeningSdk();
  bus.emit(received(0), 0);
  bus.emit(received(2), 2);
  assert.deepEqual(ids(seen), ['m0']);
  await new Promise((resolve) => setTimeout(resolve, 2_100));
  assert.deepEqual(ids(seen), ['m0', 'm2']);
  assert.equal(diagnostics(seen, 'native_event_gap').length, 1);
});

// ---------------------------------------------------------------------------
// Numbering edges that are not gaps
// ---------------------------------------------------------------------------

test('a recreated native module counting from zero is not a gap', async () => {
  const calls = serveRing({});
  const { bus, seen } = listeningSdk();
  bus.emit(received(10), 10);
  bus.emit(received(11), 11);
  bus.emit(received(0), 0);
  bus.emit(received(1), 1);
  await settle();
  assert.deepEqual(calls, []);
  assert.deepEqual(ids(seen), ['m10', 'm11', 'm0', 'm1']);
  assert.equal(diagnostics(seen, 'native_event_gap').length, 0);
});

test('a restart while a replay is in flight releases it first', async () => {
  serveRing({ 1: received(1) }, { delayMs: 50 });
  const { bus, seen } = listeningSdk();
  bus.emit(received(0), 0);
  bus.emit(received(2), 2);
  bus.emit({ ...received(0), message_id: 'n0' }, 0);
  assert.deepEqual(ids(seen), ['m0', 'm2', 'n0']);
  assert.equal(diagnostics(seen, 'native_event_gap').length, 1);
  await new Promise((resolve) => setTimeout(resolve, 70));
  assert.deepEqual(ids(seen), ['m0', 'm2', 'n0'], 'a late answer for the old count is ignored');
});

test('an event without a number is dispatched as it comes', async () => {
  const calls = serveRing({});
  const { bus, seen } = listeningSdk();
  bus.emit(received(0));
  bus.emit(received(7));
  await settle();
  assert.deepEqual(calls, []);
  assert.deepEqual(ids(seen), ['m0', 'm7']);
});

test('a module without replayEvents reports the gap as before', async () => {
  hiddenMethods.add('replayEvents');
  const { bus, seen } = listeningSdk();
  bus.emit(received(0), 0);
  bus.emit(received(2), 2);
  assert.deepEqual(ids(seen), ['m0', 'm2'], 'nothing waits on a module that cannot answer');
  const gaps = diagnostics(seen, 'native_event_gap');
  assert.equal(gaps.length, 1);
  assert.deepEqual(gaps[0].context, { expected: 1, received: 2, missing: 1 });
});

test('teardown drops a paused dispatch rather than handing it to the next account', async () => {
  serveRing({ 1: received(1) }, { delayMs: 10 });
  const { sdk, bus, seen } = listeningSdk();
  bus.emit(received(0), 0);
  bus.emit(received(2), 2);
  await sdk.destroy();
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.deepEqual(ids(seen), ['m0']);
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
      hiddenMethods.clear();
      stickyOnSubscribe = [];
      captureConsole();
      try {
        await fn();
        releaseConsole();
        realConsole.log(`  ✓ ${name}`);
      } catch (error) {
        failed += 1;
        releaseConsole();
        realConsole.log(`  ✗ ${name}\n      ${error.message}`);
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
