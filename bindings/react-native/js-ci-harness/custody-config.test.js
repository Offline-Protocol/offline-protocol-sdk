#!/usr/bin/env node
/**
 * Behavioral tests for the JS-layer marshalling of the custody configuration
 * section and the two custody methods (`src/index.ts`).
 *
 * Drives the *real compiled* SDK against a stubbed native module and asserts
 * on the payloads it hands over, because every failure in this layer is
 * silent: a config field the bridge fills in with a literal makes the Rust
 * default unreachable, and for custody the default that matters is "off". A
 * bridge that sent `{ enabled: false }` for an app that never mentioned
 * custody would keep every such app off forever, after the release that
 * flips the default. That is why the assertions below check for *absence* as
 * hard as they check for presence.
 *
 * The sibling Rust guard `every_bridge_reads_the_custody_config_section` pins
 * that both native parsers read the section this file proves JS sends.
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

function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc}: run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-custody-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

let nativeOverrides = {};
let nativeCalls = [];

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      return (...args) => {
        nativeCalls.push({ method, args });
        const override = nativeOverrides[method];
        return override ? override(...args) : Promise.resolve();
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

let OfflineProtocol;

const newSdk = (config = {}) =>
  new OfflineProtocol({ appId: 'harness', profile: 'harness-profile', ...config });

function onlyCall(method) {
  const matches = nativeCalls.filter((c) => c.method === method);
  assert.equal(matches.length, 1, `expected exactly one ${method} call, saw ${matches.length}`);
  return matches[0];
}

function payloadOf(method) {
  return JSON.parse(onlyCall(method).args[0]);
}

// ---------------------------------------------------------------------------
// The create-time custody section
// ---------------------------------------------------------------------------

test('the custody section reaches native field-for-field when the app sets it', async () => {
  const sdk = newSdk({
    custody: {
      enabled: true,
      holdMs: 3600000,
      maxEntriesPerDepositor: 16,
      maxBytesPerDepositor: 131072,
      maxEntries: 128,
      maxBytes: 4194304,
      strangerMaxEntries: 2,
      strangerMaxBytes: 65536,
      overflowPolicy: 'drop_newest',
    },
  });
  await sdk.start();

  assert.deepEqual(payloadOf('create').custody, {
    enabled: true,
    holdMs: 3600000,
    maxEntriesPerDepositor: 16,
    maxBytesPerDepositor: 131072,
    maxEntries: 128,
    maxBytes: 4194304,
    strangerMaxEntries: 2,
    strangerMaxBytes: 65536,
    overflowPolicy: 'drop_newest',
  });
});

test('an unset custody section is absent from the create payload', async () => {
  // Absence is the assertion. A bridge that sent `{ enabled: false }` here
  // would make the Rust default unreachable, and nothing would report it.
  const sdk = newSdk({});
  await sdk.start();

  assert.equal(
    'custody' in payloadOf('create'),
    false,
    'an unconfigured custody section must not be materialised by the bridge'
  );
});

test('a partial custody section carries only the fields it names', async () => {
  // The ordinary case: an app switches custody on and names nothing else.
  // Every unnamed field must stay absent so the core keeps its own value.
  const sdk = newSdk({ custody: { enabled: true } });
  await sdk.start();

  assert.deepEqual(payloadOf('create').custody, { enabled: true });
});

test('a custody section that switches it off still crosses the bridge', async () => {
  // Explicitly off is not the same as unset: it must reach native, so an app
  // can turn custody off after the default ever flips on.
  const sdk = newSdk({ custody: { enabled: false } });
  await sdk.start();

  assert.deepEqual(payloadOf('create').custody, { enabled: false });
});

// ---------------------------------------------------------------------------
// The two custody methods
// ---------------------------------------------------------------------------

test('getCustodyStats reads through to native unchanged', async () => {
  const stats = { held: 2, heldBytes: 4096, accepted: 3, delivered: 1, refusedStranger: 4 };
  nativeOverrides.getCustodyStats = () => Promise.resolve(stats);
  const sdk = newSdk({});
  await sdk.start();

  assert.deepEqual(await sdk.getCustodyStats(), stats);
  onlyCall('getCustodyStats');
});

test('eraseCustody is its own call, distinct from the data wipe', async () => {
  const sdk = newSdk({});
  await sdk.start();
  await sdk.eraseCustody();

  onlyCall('eraseCustody');
  assert.equal(
    nativeCalls.some((c) => c.method === 'dataWipeAll'),
    false
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
      nativeOverrides = {};
      nativeCalls = [];
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
