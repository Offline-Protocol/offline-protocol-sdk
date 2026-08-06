#!/usr/bin/env node
/**
 * Behavioral tests for the JS-layer one-shot event hold (`src/index.ts`).
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

  /** A live native emit, after the SDK is already subscribed. */
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

// ---------------------------------------------------------------------------
// Scaffolding
// ---------------------------------------------------------------------------

const realConsole = { log: console.log, warn: console.warn, error: console.error };
let captured = { warn: [], error: [] };

/**
 * Silences and records SDK console output. Applied around every test by the
 * runner — the SDK is chatty and its own warnings are what some of these
 * assert on — and called again inside a test that needs a clean buffer from
 * some point onwards.
 */
function captureConsole() {
  captured = { warn: [], error: [] };
  console.log = () => {};
  console.warn = (...args) => captured.warn.push(args.join(' '));
  console.error = (...args) => captured.error.push(args.join(' '));
}

function releaseConsole() {
  Object.assign(console, realConsole);
}

/** Drains the microtask queue, so any scheduled replay has run. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

function newSdk({ sticky = [], config = {} } = {}) {
  stickyOnSubscribe = sticky;
  const sdk = new OfflineProtocol({ appId: 'harness', userId: 'harness-user', ...config });
  return { sdk, bus: buses[buses.length - 1] };
}

const SUPERSEDED = { type: 'internet_session_superseded', timestamp: 1, reason: 'replaced' };
const MESH_STOPPED = { type: 'mesh_stopped_by_user', timestamp: 1 };
const PERIODIC = { type: 'internet_status_changed', timestamp: 1, connected: false };

let OfflineProtocol;

// ---------------------------------------------------------------------------
// The hold: an event that arrives before the app has a listener
// ---------------------------------------------------------------------------

test('a one-shot flushed during construction reaches a listener registered after it', async () => {
  const { sdk } = newSdk({ sticky: [MESH_STOPPED] });
  const seen = [];
  sdk.on('mesh_stopped_by_user', (event) => seen.push(event));
  await settle();
  assert.deepEqual(
    seen.map((event) => event.type),
    ['mesh_stopped_by_user']
  );
});

test('the hold survives an await between construction and on()', async () => {
  const { sdk } = newSdk({ sticky: [SUPERSEDED] });
  await settle();
  await settle();
  const seen = [];
  sdk.on('internet_session_superseded', (event) => seen.push(event));
  await settle();
  assert.equal(seen.length, 1);
});

test('a replay never runs before the on() that registered it returns', async () => {
  const { sdk } = newSdk({ sticky: [MESH_STOPPED] });
  let registrationReturned = false;
  let sawRegistrationReturned = null;
  // Recorded rather than asserted in place: `emitEvent` wraps every listener
  // call in try/catch, so an assertion that throws in here is swallowed and
  // the test passes vacuously.
  sdk.on('mesh_stopped_by_user', () => {
    sawRegistrationReturned = registrationReturned;
  });
  registrationReturned = true;
  await settle();
  assert.equal(sawRegistrationReturned, true, 'the handler ran synchronously inside on()');
});

test("a same-tick 'all' listener added after a specific one also receives the replay", async () => {
  const { sdk } = newSdk({ sticky: [MESH_STOPPED] });
  const specific = [];
  const all = [];
  sdk.on('mesh_stopped_by_user', (event) => specific.push(event));
  sdk.on('all', (event) => all.push(event));
  await settle();
  assert.equal(specific.length, 1, 'specific listener');
  assert.equal(all.length, 1, "'all' listener registered in the same tick");
});

test('three registrations in one tick each deliver the held event exactly once', async () => {
  const { sdk } = newSdk({ sticky: [MESH_STOPPED] });
  const counts = [0, 0, 0];
  sdk.on('mesh_stopped_by_user', () => (counts[0] += 1));
  sdk.on('mesh_stopped_by_user', () => (counts[1] += 1));
  sdk.on('mesh_stopped_by_user', () => (counts[2] += 1));
  await settle();
  assert.deepEqual(counts, [1, 1, 1], 'removal happens when the replay is scheduled, not when it runs');
});

test("both one-shot tags can be held at once and both reach one 'all' listener, in order", async () => {
  const { sdk } = newSdk({ sticky: [SUPERSEDED, MESH_STOPPED] });
  const seen = [];
  sdk.on('all', (event) => seen.push(event.type));
  await settle();
  assert.deepEqual(seen, ['internet_session_superseded', 'mesh_stopped_by_user']);
});

test('a repeated one-shot collapses to the newest copy', async () => {
  const { sdk } = newSdk({
    sticky: [
      { ...SUPERSEDED, reason: 'first' },
      { ...SUPERSEDED, reason: 'second' },
    ],
  });
  const seen = [];
  sdk.on('internet_session_superseded', (event) => seen.push(event.reason));
  await settle();
  assert.deepEqual(seen, ['second']);
});

test('removing the listener before the replay runs re-holds the event', async () => {
  const { sdk } = newSdk({ sticky: [MESH_STOPPED] });
  const first = [];
  const handler = (event) => first.push(event);
  sdk.on('mesh_stopped_by_user', handler);
  sdk.off('mesh_stopped_by_user', handler);
  await settle();
  assert.equal(first.length, 0, 'removed listener must not be called');

  const second = [];
  sdk.on('mesh_stopped_by_user', (event) => second.push(event));
  await settle();
  assert.equal(second.length, 1, 'emitEvent must re-hold what it could not deliver');
});

// ---------------------------------------------------------------------------
// What is *not* held
// ---------------------------------------------------------------------------

test('a periodic event is not held for a late listener', async () => {
  const { sdk } = newSdk({ sticky: [PERIODIC] });
  const seen = [];
  sdk.on('internet_status_changed', (event) => seen.push(event));
  await settle();
  assert.equal(seen.length, 0, 'a replayed periodic event would report a state that has since changed');
});

test('an event delivered live is not also held', async () => {
  const { sdk, bus } = newSdk();
  const live = [];
  sdk.on('mesh_stopped_by_user', (event) => live.push(event));
  bus.emit(MESH_STOPPED);
  assert.equal(live.length, 1, 'live delivery is synchronous');

  const later = [];
  sdk.on('mesh_stopped_by_user', (event) => later.push(event));
  await settle();
  assert.equal(later.length, 0);
});

test('a listener that throws still counts as delivered — the event is not re-held', async () => {
  const { sdk, bus } = newSdk();
  sdk.on('mesh_stopped_by_user', () => {
    throw new Error('handler blew up');
  });
  bus.emit(MESH_STOPPED);
  assert.equal(captured.error.length, 1, 'the throw is caught and logged, not propagated');

  const later = [];
  sdk.on('mesh_stopped_by_user', (event) => later.push(event));
  await settle();
  assert.equal(later.length, 0, 'the emit, not its outcome, discards the hold');
});

// ---------------------------------------------------------------------------
// Staleness transitions
// ---------------------------------------------------------------------------

test('start() sweeps a hold no listener claimed', async () => {
  const { sdk } = newSdk({ sticky: [MESH_STOPPED] });
  await sdk.start();
  const seen = [];
  sdk.on('mesh_stopped_by_user', (event) => seen.push(event));
  await settle();
  assert.equal(seen.length, 0, 'a mesh coming up must not be reported as stopped');
});

test('start() does not swallow a hold a synchronous on() already claimed', async () => {
  const { sdk } = newSdk({ sticky: [SUPERSEDED] });
  const seen = [];
  sdk.on('internet_session_superseded', (event) => seen.push(event));
  await sdk.start();
  assert.equal(seen.length, 1, 'the claim was removed at scheduling time and delivers during start()');
});

test("enableTransport('internet') drops a held supersede", async () => {
  const { sdk } = newSdk({ sticky: [SUPERSEDED] });
  await sdk.enableTransport('internet', { enabled: true, serverAddress: 'wss://relay.invalid' });
  const seen = [];
  sdk.on('internet_session_superseded', (event) => seen.push(event));
  await settle();
  assert.equal(seen.length, 0, 'the enable clears the latch the held event reports');
});

test('a failed enableTransport(\'internet\') keeps the held supersede', async () => {
  const { sdk } = newSdk({ sticky: [SUPERSEDED] });
  nativeOverrides.enableTransport = () => Promise.reject(new Error('no route to host'));
  await assert.rejects(() => sdk.enableTransport('internet', { enabled: true }));

  const seen = [];
  sdk.on('internet_session_superseded', (event) => seen.push(event));
  await settle();
  assert.equal(seen.length, 1, 'a failed enable leaves the latch set, so the event is still true');
});

test('enabling another transport leaves a held supersede alone', async () => {
  const { sdk } = newSdk({ sticky: [SUPERSEDED] });
  await sdk.enableTransport('wifiDirect', { enabled: true });
  const seen = [];
  sdk.on('internet_session_superseded', (event) => seen.push(event));
  await settle();
  assert.equal(seen.length, 1);
});

test('destroy() clears an unclaimed hold', async () => {
  const { sdk } = newSdk({ sticky: [MESH_STOPPED] });
  await sdk.destroy();
  const seen = [];
  sdk.on('mesh_stopped_by_user', (event) => seen.push(event));
  await settle();
  assert.equal(seen.length, 0);
});

test('destroy() in the same tick as on() does not leak the event into the next session', async () => {
  // `on(...)` schedules a replay and empties the hold; `destroy()` then runs
  // `removeAllListeners()` before that replay, so the replay finds nothing
  // listening and re-holds. Instances are reusable, and the documented order
  // registers listeners *before* `start()` — so without the post-yield clear
  // in `destroy()`, the next session's first `on(...)` gets last session's
  // event, with nothing to correct it.
  const { sdk } = newSdk({ sticky: [MESH_STOPPED] });
  sdk.on('mesh_stopped_by_user', () => {});
  await sdk.destroy();

  const seen = [];
  sdk.on('mesh_stopped_by_user', (event) => seen.push(event));
  await settle();
  assert.equal(seen.length, 0, 'a re-held entry must not survive destroy()');
});

// ---------------------------------------------------------------------------
// The silence this closes
// ---------------------------------------------------------------------------

test('the no-listener drop warning fires once per event type', async () => {
  const { bus } = newSdk();
  bus.emit(PERIODIC);
  bus.emit(PERIODIC);
  bus.emit({ type: 'peer_discovered', timestamp: 1 });
  const drops = captured.warn.filter((line) => line.includes('Dropped a'));
  assert.equal(drops.length, 2, 'one warning per type, not per event');
});

test('no drop warning while the app has any listener registered', async () => {
  const { sdk, bus } = newSdk();
  sdk.on('message_received', () => {});
  captureConsole();
  bus.emit(PERIODIC);
  assert.equal(
    captured.warn.filter((line) => line.includes('Dropped a')).length,
    0,
    'an app that listens selectively is making a choice, not a mistake'
  );
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
