#!/usr/bin/env node
/**
 * Behavioral tests for the `peer_rssi` opt-in (`src/index.ts`,
 * `syncPeerRssiEvents`).
 *
 * The engine emits `peer_rssi` from every Bluetooth LE advertisement it can
 * attribute, so it is off unless the app listens. Both failures are silent: a
 * switch that never turns on looks like no peer in range, and one that never
 * turns off spends the radio's whole reading rate on a bridge nobody reads.
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

/** Compiles `src/` to a scratch dir. See one-shot-hold.test.js for why. */
function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-rssi-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

/** Every `setPeerRssiEvents` argument, in order. */
let switches = [];
/** The config JSON the last `create` received. */
let createdWith = null;

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      return (...args) => {
        if (method === 'setPeerRssiEvents') switches.push(args[0]);
        if (method === 'create') createdWith = JSON.parse(args[0]);
        if (method === 'isMlsInitialized') return Promise.resolve(true);
        return Promise.resolve();
      };
    },
  }
);

class StubNativeEventEmitter {
  addListener() {
    return { remove: () => {} };
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
const quiet = () => {
  console.log = console.warn = console.error = () => {};
};

const tests = [];
const test = (name, fn) => tests.push({ name, fn });
let OfflineProtocol;
const newSdk = () => new OfflineProtocol({ appId: 'harness', profile: 'harness-profile' });
const noop = () => {};

test('a listener registered before start() switches it on at creation', async () => {
  const sdk = newSdk();
  sdk.on('peer_rssi', noop);
  assert.deepEqual(switches, [], 'nothing to tell before the engine exists');
  await sdk.start();
  assert.deepEqual(switches, [true]);
});

test('only the first listener and the last removal reach native', async () => {
  const sdk = newSdk();
  await sdk.start();
  const other = () => {};
  sdk.on('peer_rssi', noop).on('peer_rssi', other);
  sdk.off('peer_rssi', noop);
  assert.deepEqual(switches, [true]);
  sdk.off('peer_rssi', other);
  assert.deepEqual(switches, [true, false]);
});

test("an 'all' listener does not switch it on", async () => {
  const sdk = newSdk();
  await sdk.start();
  sdk.on('all', noop).on('neighbor_discovered', noop);
  assert.deepEqual(switches, []);
});

test('removeAllListeners switches it off', async () => {
  const sdk = newSdk();
  await sdk.start();
  sdk.on('peer_rssi', noop);
  sdk.removeAllListeners();
  assert.deepEqual(switches, [true, false]);
});

test('a restart after destroy() switches it on again for the new engine', async () => {
  const sdk = newSdk();
  sdk.on('peer_rssi', noop);
  await sdk.start();
  await sdk.destroy();
  sdk.on('peer_rssi', noop);
  await sdk.start();
  assert.deepEqual(switches, [true, false, true]);
});

test('advertiseNodeHash reaches the native config, and is absent unless set', async () => {
  // Android reads `transports.ble.advertiseNodeHash`; a transform that drops
  // it leaves the scan response without the hash and no error anywhere.
  await new OfflineProtocol({
    appId: 'harness',
    profile: 'harness-profile',
    transports: { ble: { enabled: true, advertiseNodeHash: true } },
  }).start();
  assert.equal(createdWith.transports.ble.advertiseNodeHash, true);
  await newSdk().start();
  assert.equal(createdWith.transports?.ble?.advertiseNodeHash, undefined);
});

(async () => {
  const outDir = compileSdk();
  try {
    ({ OfflineProtocol } = require(path.join(outDir, 'index.js')));
    let failed = 0;
    for (const { name, fn } of tests) {
      switches = [];
      quiet();
      try {
        await fn();
        Object.assign(console, realConsole);
        realConsole.log(`  ✓ ${name}`);
      } catch (error) {
        failed += 1;
        Object.assign(console, realConsole);
        realConsole.log(`  ✗ ${name}\n      ${error.message}`);
      }
    }
    realConsole.log(
      failed === 0 ? `\n${tests.length} passed.` : `\n${failed} of ${tests.length} FAILED.`
    );
    process.exitCode = failed === 0 ? 0 : 1;
  } finally {
    Object.assign(console, realConsole);
    fs.rmSync(outDir, { recursive: true, force: true });
  }
})();
