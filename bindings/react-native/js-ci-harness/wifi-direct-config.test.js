#!/usr/bin/env node
/**
 * Behavioral tests for `start()` bringing up the peer-stream slot
 * (`transports.wifiDirect`) from the config (`src/index.ts`).
 *
 * Both native modules build the Wi-Fi Direct manager only inside
 * `enableTransport`, and `start()` auto-enabled internet, Nostr and Reticulum
 * from their config sections but not this one. So `wifiDirect: { enabled: true }`,
 * the documented way to turn it on, left the transport off for the whole
 * session with nothing logged: found on two Android phones, where the demo
 * app sat on "Searching for peers" next to a live Wi-Fi Direct group.
 *
 * Drives the real compiled `OfflineProtocol` against a stubbed native module,
 * as relay-config.test.js does. See README.md for why the package has no other
 * JS test setup.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-wifi-direct-'));
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
/** Every native call the SDK made, in order: `{ method, args }`. */
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

// ---------------------------------------------------------------------------
// Scaffolding
// ---------------------------------------------------------------------------

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

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

let OfflineProtocol;

const newSdk = (config = {}) =>
  new OfflineProtocol({ appId: 'harness', profile: 'harness-profile', ...config });

/** The single call to `method`, asserting it happened exactly once. */
function onlyCall(method) {
  const matches = nativeCalls.filter((c) => c.method === method);
  assert.equal(matches.length, 1, `expected exactly one ${method} call, saw ${matches.length}`);
  return matches[0];
}

/** The JSON payload of the single call to `method`, parsed. */
function payloadOf(method) {
  return JSON.parse(onlyCall(method).args[0]);
}

// ---------------------------------------------------------------------------
// The peer-stream slot at start()
// ---------------------------------------------------------------------------

const enableCalls = (type) =>
  nativeCalls.filter((c) => c.method === 'enableTransport' && c.args[0] === type);

test('an enabled wifiDirect section is enabled at start, with its config', async () => {
  const wifiDirect = { enabled: true, autoAccept: true, groupOwnerIntent: 10 };
  const sdk = newSdk({ transports: { wifiDirect } });
  await sdk.start();

  const calls = enableCalls('wifiDirect');
  assert.equal(calls.length, 1, 'the configured transport must be started exactly once');
  assert.deepEqual(calls[0].args[1], wifiDirect, 'the section must cross the bridge whole');
  assert.equal(payloadOf('create').wifiDirectEnabled, true);
});

test('it is enabled after the protocol itself has started', async () => {
  const sdk = newSdk({ transports: { wifiDirect: { enabled: true } } });
  await sdk.start();

  const startAt = nativeCalls.findIndex((c) => c.method === 'start');
  const enableAt = nativeCalls.findIndex(
    (c) => c.method === 'enableTransport' && c.args[0] === 'wifiDirect'
  );
  assert.ok(startAt >= 0 && enableAt > startAt,
    'the manager hands the core every stream it proves, so the core must be running first');
});

test('an absent or disabled section starts nothing', async () => {
  for (const transports of [undefined, {}, { wifiDirect: { enabled: false } }]) {
    nativeCalls = [];
    const sdk = newSdk(transports ? { transports } : {});
    await sdk.start();
    assert.equal(enableCalls('wifiDirect').length, 0,
      `nothing asked for the transport: ${JSON.stringify(transports)}`);
  }
});

test('a failed enable warns and does not fail start()', async () => {
  nativeOverrides.enableTransport = (type) =>
    type === 'wifiDirect'
      ? Promise.reject(new Error('WiFi P2P is not available on this device'))
      : Promise.resolve();
  const sdk = newSdk({ transports: { wifiDirect: { enabled: true } } });
  await sdk.start();

  assert.ok(
    captured.warn.some((line) => line.includes('Wi-Fi Direct') && line.includes('not available')),
    `the reason must reach the log: ${JSON.stringify(captured.warn)}`
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
