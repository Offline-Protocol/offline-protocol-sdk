#!/usr/bin/env node
/**
 * Behavioral tests for the JS shape of a key package record (`src/index.ts`,
 * `toMlsKeyPackage`).
 *
 * Both native bridges send `createdAtMs`, `expiresAtMs` and `synced`. The
 * wrappers used to read `createdAt` and `isSynced`, so every caller received
 * `undefined` for both. Nothing failed: the typecheck sees an `any`, and the
 * Rust text guards cannot tell a key that is present from one that is read.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-kp-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

let nativeOverrides = {};

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

/** A record exactly as both native bridges build it. */
function nativeRecord(overrides = {}) {
  return {
    packageId: 'pkg-1',
    userId: 'off1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqa',
    keyPackageData: [1, 2, 3],
    createdAtMs: 1_700_000_000_000,
    expiresAtMs: 1_702_592_000_000,
    synced: true,
    ...overrides,
  };
}

let OfflineProtocol;

function newSdk() {
  return new OfflineProtocol({ appId: 'harness', profile: 'harness-profile' });
}

for (const method of ['mlsGenerateKeyPackage', 'mlsGetOrCreateKeyPackage']) {
  test(`${method} reads the timestamps and the synced flag the bridges send`, async () => {
    nativeOverrides[method] = () => Promise.resolve(nativeRecord());
    const pkg = await newSdk()[method]();

    assert.equal(pkg.createdAt, 1_700_000_000_000);
    assert.equal(pkg.expiresAt, 1_702_592_000_000);
    assert.equal(pkg.isSynced, true);
    assert.equal(pkg.packageId, 'pkg-1');
    assert.deepEqual(pkg.keyPackageData, [1, 2, 3]);
  });
}

test('mlsGetPendingKeyPackages maps every record the same way', async () => {
  nativeOverrides.mlsGetPendingKeyPackages = () =>
    Promise.resolve([nativeRecord(), nativeRecord({ packageId: 'pkg-2', synced: false })]);
  const pending = await newSdk().mlsGetPendingKeyPackages();

  assert.equal(pending.length, 2);
  assert.equal(pending[0].createdAt, 1_700_000_000_000);
  assert.equal(pending[0].expiresAt, 1_702_592_000_000);
  assert.equal(pending[1].packageId, 'pkg-2');
  assert.equal(pending[1].isSynced, false, 'false must stay false, not become undefined');
});

(async () => {
  const outDir = compileSdk();
  try {
    ({ OfflineProtocol } = require(path.join(outDir, 'index.js')));

    let failed = 0;
    for (const { name, fn } of tests) {
      nativeOverrides = { isMlsInitialized: () => Promise.resolve(true) };
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
