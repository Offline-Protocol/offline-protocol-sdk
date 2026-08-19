#!/usr/bin/env node
/**
 * Behavioral tests for the JS-layer marshalling of the data-layer
 * configuration and the `DataStore` surface (`src/index.ts`).
 *
 * Drives the *real compiled* SDK against a stubbed native module and asserts
 * on the payloads it hands over, because every failure in this layer is
 * silent. Two shapes in particular:
 *
 *  - A config field the bridge fills in with a literal makes the Rust default
 *    unreachable, and nothing anywhere reports it. That is why the assertions
 *    below check for *absence* as hard as they check for presence.
 *  - A `DataStore` method that marshals its arguments wrongly reaches native
 *    and fails there, far from the mistake.
 *
 * The sibling Rust guard `every_bridge_reads_the_data_config_section` pins
 * that both native parsers read the section this file proves JS sends. Both
 * halves are required: a payload nobody parses and a parser nobody feeds are
 * the same bug from opposite ends.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-data-'));
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

function onlyCall(method) {
  const matches = nativeCalls.filter((c) => c.method === method);
  assert.equal(matches.length, 1, `expected exactly one ${method} call, saw ${matches.length}`);
  return matches[0];
}

function payloadOf(method) {
  return JSON.parse(onlyCall(method).args[0]);
}

// ---------------------------------------------------------------------------
// The create-time data section
// ---------------------------------------------------------------------------

test('the data section reaches native when the app sets it', async () => {
  const sdk = newSdk({ data: { enabled: true } });
  await sdk.start();

  const payload = payloadOf('create');
  assert.deepEqual(payload.data, { enabled: true });
});

test('an unset data section is absent from the create payload', async () => {
  // Absence is the assertion. A bridge that sent `{ enabled: false }` here
  // would make the Rust default unreachable, and nothing would report it —
  // the layer would simply be off in a way no one could configure back on
  // from the core.
  const sdk = newSdk({});
  await sdk.start();

  const payload = payloadOf('create');
  assert.equal(
    'data' in payload,
    false,
    'an unconfigured data section must not be materialised by the bridge'
  );
});

test('a data section that disables the layer still crosses the bridge', async () => {
  // Explicitly off is not the same as unset: it must reach native, so an app
  // can turn the layer off after the default flips on.
  const sdk = newSdk({ data: { enabled: false } });
  await sdk.start();

  assert.deepEqual(payloadOf('create').data, { enabled: false });
});

// ---------------------------------------------------------------------------
// The DataStore surface
// ---------------------------------------------------------------------------

test('a map value is marshalled as a JSON DataValue', async () => {
  const store = new DataStore();
  await store.mapSet('space-1', 'doc-1', 'fields', 'name', { kind: 'text', value: 'Ada' });

  const call = onlyCall('dataMapSet');
  assert.deepEqual(call.args.slice(0, 4), ['space-1', 'doc-1', 'fields', 'name']);
  assert.deepEqual(JSON.parse(call.args[4]), { kind: 'text', value: 'Ada' });
});

test('a list value is marshalled the same way as a map value', async () => {
  const store = new DataStore();
  await store.listPush('space-1', 'doc-1', 'log', { kind: 'int', value: 7 });

  const call = onlyCall('dataListPush');
  assert.deepEqual(JSON.parse(call.args[3]), { kind: 'int', value: 7 });
});

test('mapGet parses the JSON native returns, and passes null through', async () => {
  nativeOverrides.dataMapGetJson = () => Promise.resolve('{"kind":"int","value":3}');
  const store = new DataStore();
  assert.deepEqual(await store.mapGet('s', 'd', 'm', 'k'), { kind: 'int', value: 3 });

  nativeCalls = [];
  nativeOverrides.dataMapGetJson = () => Promise.resolve(null);
  assert.equal(await store.mapGet('s', 'd', 'm', 'k'), null);
});

test('listDocs and listSpaces parse the JSON array native returns', async () => {
  nativeOverrides.dataListDocs = () => Promise.resolve('["alpha","beta"]');
  nativeOverrides.dataListSpaces = () => Promise.resolve('["space-1"]');
  const store = new DataStore();

  assert.deepEqual(await store.listDocs('space-1'), ['alpha', 'beta']);
  assert.deepEqual(await store.listSpaces(), ['space-1']);
});

test('docJson parses the document state native returns', async () => {
  nativeOverrides.dataDocJson = () => Promise.resolve('{"fields":{"name":"Ada"}}');
  const store = new DataStore();
  assert.deepEqual(await store.docJson('s', 'd'), { fields: { name: 'Ada' } });
});

test('text positions cross the bridge unchanged', async () => {
  const store = new DataStore();
  await store.textInsert('s', 'd', 'body', 5, 'hello');

  // Character offsets, not bytes: the core validates against the document's
  // character length, so any rounding here would be a silent off-by-N.
  assert.deepEqual(onlyCall('dataTextInsert').args, ['s', 'd', 'body', 5, 'hello']);
});

test('wipeAll is its own call, distinct from wipePersistedState', async () => {
  // They are not interchangeable: wipePersistedState clears the default
  // provider's account directory, which a custom data backend is not inside.
  const store = new DataStore();
  await store.wipeAll();

  onlyCall('dataWipeAll');
  assert.equal(
    nativeCalls.some((c) => c.method === 'wipePersistedState'),
    false
  );
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
