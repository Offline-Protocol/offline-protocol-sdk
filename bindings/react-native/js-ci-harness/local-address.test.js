#!/usr/bin/env node
/**
 * Behavioral tests for the JS-layer cache of this device's derived address
 * (`src/index.ts`, `cachedLocalAddress`).
 *
 * The address is no longer something the app configured — it is minted from an
 * identity key in this profile's storage and can only be read back. The JS
 * layer caches it so that comparisons against "us" do not each cost a native
 * round-trip, and every failure that cache can have is silent: an address
 * served after the identity it names is gone, or a comparison run before the
 * cache is warm, both produce a confidently wrong answer and no error.
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

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

/** Compiles `src/` to a scratch dir. See one-shot-hold.test.js for why. */
function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-addr-'));
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

let nativeOverrides = {};

/** Every `localAddress()` call the SDK made, so a cache hit is observable. */
let localAddressCalls = 0;

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      return (...args) => {
        if (method === 'localAddress') localAddressCalls += 1;
        const override = nativeOverrides[method];
        return override ? override(...args) : Promise.resolve();
      };
    },
  }
);

class StubNativeEventEmitter {
  constructor() {
    this.listeners = new Map();
  }

  addListener(channel, handler) {
    let handlers = this.listeners.get(channel);
    if (!handlers) {
      handlers = new Set();
      this.listeners.set(channel, handlers);
    }
    handlers.add(handler);
    return { remove: () => handlers.delete(handler) };
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

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

const ADDRESS_A = 'off1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqa';
const ADDRESS_B = 'off1zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzb';

function newSdk(config = {}) {
  return new OfflineProtocol({ appId: 'harness', profile: 'harness-profile', ...config });
}

let OfflineProtocol;

// ---------------------------------------------------------------------------
// The cache is warm as soon as start() resolves
// ---------------------------------------------------------------------------

test('start() caches the address without waiting for the identity_ready event', async () => {
  const sdk = newSdk();
  await sdk.start();

  // No event was ever emitted — only the eager read during start() can have
  // populated this. Anything comparing a peer against "us" is safe the moment
  // start() resolves.
  assert.equal(await sdk.localAddress(), ADDRESS_A);
});

test('a cached address is served without a second native call', async () => {
  const sdk = newSdk();
  await sdk.start();

  const callsAfterStart = localAddressCalls;
  await sdk.localAddress();
  await sdk.localAddress();

  assert.equal(
    localAddressCalls,
    callsAfterStart,
    'repeat reads must be answered from the cache, which is the reason it exists'
  );
});

test('the identity_ready event populates the cache', async () => {
  const sdk = newSdk();
  // Reach the emitter the way the SDK's own subscription does, then hand it
  // the event the native side would send during initialization.
  sdk.emitEvent({ type: 'identity_ready', timestamp: 1, address: ADDRESS_B });

  assert.equal(await sdk.localAddress(), ADDRESS_B);
});

// ---------------------------------------------------------------------------
// The cache must not outlive the identity it names
// ---------------------------------------------------------------------------

test('destroy() clears the cached address', async () => {
  const sdk = newSdk();
  await sdk.start();
  assert.equal(await sdk.localAddress(), ADDRESS_A);

  await sdk.destroy();

  // The documented cleanup flow is destroy → wipePersistedState → start, and
  // the restart mints a *new* identity. A surviving cache answers with the
  // dead one forever, because the cache hit means the native side is never
  // asked again.
  nativeOverrides.localAddress = () => Promise.resolve(ADDRESS_B);
  assert.equal(
    await sdk.localAddress(),
    ADDRESS_B,
    'after destroy the address must be re-read, not served from the previous identity'
  );
});

test('a restart under a new identity reports the new address', async () => {
  const sdk = newSdk();
  await sdk.start();
  await sdk.destroy();

  nativeOverrides.localAddress = () => Promise.resolve(ADDRESS_B);
  await sdk.start();

  assert.equal(await sdk.localAddress(), ADDRESS_B);
});

// ---------------------------------------------------------------------------
// Session attribution depends on knowing which half of the pair is us
// ---------------------------------------------------------------------------

const WELCOME = {
  groupId: 'g',
  welcomeData: 'd',
  inviterId: ADDRESS_B,
  timestampMs: 1,
};

/** Our own address first in the member list — the order that misattributes. */
const SESSION_RAW = { memberIds: [ADDRESS_A, ADDRESS_B], groupId: 'g', epoch: 0 };

test('session attribution picks the peer, not us', async () => {
  const sdk = newSdk();
  await sdk.start();

  nativeOverrides.mlsJoinSession = () => Promise.resolve(SESSION_RAW);

  const info = await sdk.mlsJoinSession(WELCOME);
  assert.equal(info.otherUserId, ADDRESS_B, 'the peer is the member that is not us');
});

test('session attribution reports nothing rather than guessing before the address is known', async () => {
  const sdk = newSdk();
  // No start(), so the cache is empty — `find(id => id !== null)` would match
  // the first member, which is us here.
  nativeOverrides.mlsJoinSession = () => Promise.resolve(SESSION_RAW);

  const info = await sdk.mlsJoinSession(WELCOME);
  assert.equal(
    info.otherUserId,
    '',
    'without our own address there is no way to tell the pair apart, and a guess names us as the peer'
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
      nativeOverrides = {
        isMlsInitialized: () => Promise.resolve(true),
        localAddress: () => Promise.resolve(ADDRESS_A),
      };
      localAddressCalls = 0;
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
      failed === 0 ? `\n${tests.length} passed.` : `\n${failed} of ${tests.length} FAILED.`
    );
    process.exitCode = failed === 0 ? 0 : 1;
  } finally {
    releaseConsole();
    fs.rmSync(outDir, { recursive: true, force: true });
  }
})();
