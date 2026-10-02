#!/usr/bin/env node
/**
 * Behavioral tests for the order in which a React Native interest
 * declaration reaches the native data store (`DataStore.setInterest` and
 * `OfflineProtocol.start` in `src/index.ts`).
 *
 * The engine's order is: open storage, declare interest, start. Its start-up
 * exchange offers every held space with the interest in force at that
 * instant, and a narrowing never deletes what a wider offer already pulled
 * in. React Native folds storage and start into one `start()` call, so the
 * SDK holds a declaration made before it and applies it between MLS
 * initialization and the native start (#472). Each failure here is silent on
 * a device: a declaration applied after the native start compiles, resolves,
 * and replicates the whole space first.
 *
 * The sibling Rust guard `react_native_start_applies_held_interest_before_the_engine_starts`
 * pins the same order as text. See README.md for why the package has no
 * other JS test setup.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-interest-'));
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
let DataStore;

const newSdk = (config = {}) =>
  new OfflineProtocol({ appId: 'harness', profile: 'harness-profile', ...config });

const methods = () => nativeCalls.map((c) => c.method);
const callsTo = (method) => nativeCalls.filter((c) => c.method === method);

/**
 * Clears the module-level hold left by the previous test. Only an instance
 * that created an engine resets it, so this starts one and destroys it.
 */
async function resetModuleState() {
  const overrides = nativeOverrides;
  nativeOverrides = {};
  const sdk = newSdk();
  await sdk.start();
  await sdk.destroy();
  nativeOverrides = overrides;
  nativeCalls = [];
}

function rejectWith(code, message) {
  const error = new Error(message);
  error.code = code;
  return Promise.reject(error);
}

// ---------------------------------------------------------------------------

test('a declaration before start() is held, and reaches native nowhere', async () => {
  await new DataStore().setInterest('space-1', ['inbox*']);
  assert.deepEqual(callsTo('dataSetInterest'), []);
});

test('start() applies it after MLS initialization and before the engine starts', async () => {
  await new DataStore().setInterest('space-1', ['inbox*', 'profile']);
  await newSdk().start();

  const order = methods().filter((m) =>
    ['create', 'initializeMlsWithSecureStorage', 'dataSetInterest', 'start'].includes(m)
  );
  assert.deepEqual(order, ['create', 'initializeMlsWithSecureStorage', 'dataSetInterest', 'start']);
  assert.deepEqual(callsTo('dataSetInterest')[0].args, ['space-1', ['inbox*', 'profile']]);
});

test('after start() a declaration goes straight to native', async () => {
  await newSdk().start();
  nativeCalls = [];
  await new DataStore().setInterest('space-2', ['notes*']);
  assert.deepEqual(callsTo('dataSetInterest').map((c) => c.args), [['space-2', ['notes*']]]);
});

test('the last declaration for a space wins, and each space is applied once', async () => {
  const store = new DataStore();
  await store.setInterest('space-1', ['a*']);
  await store.setInterest('space-2', ['b*']);
  await store.setInterest('space-1', ['c*']);
  await newSdk().start();

  assert.deepEqual(callsTo('dataSetInterest').map((c) => c.args), [
    ['space-1', ['c*']],
    ['space-2', ['b*']],
  ]);
});

test('a held declaration is a copy of the caller\'s array', async () => {
  const patterns = ['a*'];
  await new DataStore().setInterest('space-1', patterns);
  patterns.push('everything-else*');
  await newSdk().start();
  assert.deepEqual(callsTo('dataSetInterest')[0].args[1], ['a*']);
});

test('a refused declaration rejects start(), names the space, and never starts the engine', async () => {
  nativeOverrides.dataSetInterest = () => rejectWith('InvalidArgument', 'bad pattern');
  await new DataStore().setInterest('space-1', ['a*b']);

  let refusal;
  try {
    await newSdk().start();
  } catch (error) {
    refusal = error;
  }
  assert.ok(refusal, 'start() must reject when a held interest is refused');
  assert.match(refusal.message, /space-1/);
  assert.equal(refusal.code, 'InvalidArgument');
  assert.equal(callsTo('start').length, 0, 'the engine started under the default interest');
});

test('the refused declaration stays held until the app corrects it', async () => {
  nativeOverrides.dataSetInterest = (_space, patterns) =>
    patterns.includes('a*b') ? rejectWith('InvalidArgument', 'bad pattern') : Promise.resolve();
  const store = new DataStore();
  await store.setInterest('space-1', ['a*b']);
  const sdk = newSdk();
  await assert.rejects(sdk.start());
  await assert.rejects(sdk.start(), 'a retry without a correction must fail the same way');
  assert.equal(callsTo('start').length, 0);

  await store.setInterest('space-1', ['a*']);
  await sdk.start();
  assert.deepEqual(callsTo('dataSetInterest').at(-1).args, ['space-1', ['a*']]);
  assert.equal(callsTo('start').length, 1);
});

test('with the data layer off, a held declaration is dropped with a warning and start() proceeds', async () => {
  for (const code of ['DataDisabled', 'DataStorageUnavailable']) {
    await resetModuleState();
    nativeOverrides.dataSetInterest = () => rejectWith(code, 'off');
    const store = new DataStore();
    await store.setInterest('space-1', ['a*']);
    await store.setInterest('space-2', ['b*']);
    captured.warn = [];
    await newSdk().start();

    assert.equal(callsTo('start').length, 1, `${code}: the engine did not start`);
    assert.equal(callsTo('dataSetInterest').length, 1, `${code}: kept applying after the layer said off`);
    assert.ok(captured.warn.some((w) => w.includes(code)), `${code}: no warning`);
  }
});

test('destroy() discards a held declaration and holds the next one again', async () => {
  const sdk = newSdk();
  await sdk.emitTestEvent(); // creates the engine without starting it
  await new DataStore().setInterest('space-old', ['a*']);
  await sdk.destroy();
  nativeCalls = [];

  await new DataStore().setInterest('space-new', ['b*']);
  assert.deepEqual(callsTo('dataSetInterest'), [], 'a declaration after destroy() must be held');
  await newSdk().start();
  assert.deepEqual(callsTo('dataSetInterest').map((c) => c.args), [['space-new', ['b*']]]);
});

test('after a started engine is destroyed, the next declaration is held again', async () => {
  const sdk = newSdk();
  await sdk.start();
  await sdk.destroy();
  nativeCalls = [];
  await new DataStore().setInterest('space-1', ['a*']);
  assert.deepEqual(callsTo('dataSetInterest'), []);
});

test('destroying an instance that never created an engine leaves a running store alone', async () => {
  const running = newSdk();
  await running.start();
  const stale = newSdk();
  await stale.destroy();
  nativeCalls = [];

  await new DataStore().setInterest('space-1', ['a*']);
  assert.deepEqual(
    callsTo('dataSetInterest').map((c) => c.args),
    [['space-1', ['a*']]],
    'a stale instance turned a live declaration into a held one nothing will apply'
  );
});

test('a refused space name on a layer that is off does not hold the engine back', async () => {
  // The engine validates the name before it checks the layer, so the refusal
  // says InvalidArgument; the probe is what reveals the layer is off.
  nativeOverrides.dataSetInterest = () => rejectWith('InvalidArgument', 'bad space');
  nativeOverrides.dataListSpaces = () => rejectWith('DataDisabled', 'off');
  await new DataStore().setInterest('bad/space', ['a*']);
  await newSdk().start();
  assert.equal(callsTo('start').length, 1);
  assert.ok(captured.warn.some((w) => w.includes('DataDisabled')));
});

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

(async () => {
  const outDir = compileSdk();
  try {
    ({ OfflineProtocol, DataStore } = require(path.join(outDir, 'index.js')));

    let failed = 0;
    for (const { name, fn } of tests) {
      nativeOverrides = {};
      captureConsole();
      try {
        await resetModuleState();
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
