#!/usr/bin/env node
/**
 * Smoke tests for shared engine-bridge poll paths through the JS SDK
 * (`src/index.ts`): MLS readiness and Internet transport readiness.
 *
 * Static Rust guards and generator `--check` prove the native adapters compile
 * and delegate; this file proves the TypeScript layer still reaches the
 * correct native method names with no argument reshaping on the two queries
 * every app uses before showing MLS UI or an Internet-connected surface.
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
    throw new Error(`TypeScript not found at ${tsc} — run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-bridge-smoke-'));
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
        if (override) return override(...args);
        if (method === 'isMlsInitialized' || method === 'internetIsReady') {
          return Promise.resolve(false);
        }
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

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

let OfflineProtocol;

const newSdk = () => new OfflineProtocol({ appId: 'harness', profile: 'harness-profile' });

function onlyCall(method) {
  const matches = nativeCalls.filter((c) => c.method === method);
  assert.equal(matches.length, 1, `expected exactly one ${method} call, saw ${matches.length}`);
  return matches[0];
}

test('isMlsInitialized is a zero-arg native pass-through', async () => {
  const sdk = newSdk();
  assert.equal(await sdk.isMlsInitialized(), false);
  const call = onlyCall('isMlsInitialized');
  assert.deepEqual(call.args, []);
});

test('isInternetReady is a zero-arg native pass-through', async () => {
  const sdk = newSdk();
  assert.equal(await sdk.isInternetReady(), false);
  const call = onlyCall('internetIsReady');
  assert.deepEqual(call.args, []);
});

test('startup-style poll sequence hits MLS then Internet without reshaping', async () => {
  nativeOverrides.isMlsInitialized = () => Promise.resolve(true);
  nativeOverrides.internetIsReady = () => Promise.resolve(true);
  const sdk = newSdk();
  assert.equal(await sdk.isMlsInitialized(), true);
  assert.equal(await sdk.isInternetReady(), true);
  assert.deepEqual(
    nativeCalls.map((c) => c.method),
    ['isMlsInitialized', 'internetIsReady']
  );
  for (const { args } of nativeCalls) {
    assert.deepEqual(args, []);
  }
});

test('native booleans pass through unchanged', async () => {
  nativeOverrides.isMlsInitialized = () => Promise.resolve(true);
  nativeOverrides.internetIsReady = () => Promise.resolve(false);
  const sdk = newSdk();
  assert.equal(await sdk.isMlsInitialized(), true);
  assert.equal(await sdk.isInternetReady(), false);
});

(async () => {
  const outDir = compileSdk();
  try {
    ({ OfflineProtocol } = require(path.join(outDir, 'index.js')));

    let failed = 0;
    for (const { name, fn } of tests) {
      nativeOverrides = {};
      nativeCalls = [];
      try {
        await fn();
        console.log(`  ✓ ${name}`);
      } catch (error) {
        failed += 1;
        console.log(`  ✗ ${name}\n      ${error.message}`);
      }
    }

    console.log(failed === 0 ? `\n${tests.length} passed.` : `\n${failed} of ${tests.length} FAILED.`);
    process.exitCode = failed === 0 ? 0 : 1;
  } finally {
    fs.rmSync(outDir, { recursive: true, force: true });
  }
})();
