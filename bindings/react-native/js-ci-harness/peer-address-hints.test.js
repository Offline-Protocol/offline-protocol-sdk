#!/usr/bin/env node
'use strict';

/**
 * Development warnings for peer ids that are not `off1…`-shaped
 * (`src/peerAddressHints.ts` and its call sites in `src/index.ts`).
 *
 * Pins three things:
 *
 *   1. The shape rule, which mirrors `Username::looks_like_address` in core.
 *   2. `ProtocolConfig.devPeerAddressWarnings` governs every call site,
 *      including `DataStore` and `MeshServices`, which have no config of
 *      their own. A per-call flag once left those four sites on the
 *      `__DEV__` default, so `false` did not silence them.
 *   3. The warning fires once per (method, peer id), never for a valid
 *      address or an empty id, and its dedupe set is bounded.
 *
 * See one-shot-hold.test.js for the stubbed-native-module harness pattern.
 */

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const Module = require('node:module');
const os = require('node:os');
const path = require('node:path');

const PACKAGE_DIR = path.resolve(__dirname, '..');
const VALID = 'off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn';

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

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      return () => Promise.resolve('stub');
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

let warnings = [];
const realWarn = console.warn;
function captureWarnings() {
  warnings = [];
  console.warn = (...args) => warnings.push(args.join(' '));
}
function releaseWarnings() {
  console.warn = realWarn;
}

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

const outDir = compileSdk();
const hints = require(path.join(outDir, 'peerAddressHints.js'));
const sdk = require(path.join(outDir, 'index.js'));

function fresh(config) {
  hints.resetPeerAddressWarningsForTests();
  delete globalThis.__DEV__;
  return new sdk.OfflineProtocol({ appId: 'harness', profile: 'harness', ...config });
}

test('the shape rule matches core', () => {
  assert.equal(hints.MESH_ADDRESS_ENCODED_LEN, 44);
  assert.equal(hints.MESH_ADDRESS_PREFIX, 'off1');
  assert.ok(hints.looksLikeMeshAddress(VALID));
  assert.ok(!hints.looksLikeMeshAddress('user456'));
  assert.ok(!hints.looksLikeMeshAddress('bob'));
  assert.ok(!hints.looksLikeMeshAddress(VALID.toUpperCase()));
  assert.ok(!hints.looksLikeMeshAddress(`${VALID}x`));
  assert.ok(!hints.looksLikeMeshAddress(`off2${VALID.slice(4)}`));
  assert.ok(!hints.looksLikeMeshAddress(undefined));
});

test('the public surface exports the shape check, not the logging internals', () => {
  assert.equal(typeof sdk.looksLikeMeshAddress, 'function');
  assert.equal(sdk.MESH_ADDRESS_ENCODED_LEN, 44);
  assert.equal(sdk.MESH_ADDRESS_PREFIX, 'off1');
  assert.equal(sdk.warnPeerIdIfMisshapen, undefined);
  assert.equal(sdk.devPeerAddressWarningsEnabled, undefined);
  assert.equal(sdk.peerAddressMisshapeMessage, undefined);
});

test('the default follows __DEV__', () => {
  delete globalThis.__DEV__;
  assert.equal(hints.devPeerAddressWarningsEnabled(undefined), false);
  globalThis.__DEV__ = false;
  assert.equal(hints.devPeerAddressWarningsEnabled(undefined), false);
  globalThis.__DEV__ = true;
  assert.equal(hints.devPeerAddressWarningsEnabled(undefined), true);
  assert.equal(hints.devPeerAddressWarningsEnabled(false), false);
  globalThis.__DEV__ = false;
  assert.equal(hints.devPeerAddressWarningsEnabled(true), true);
  delete globalThis.__DEV__;
});

test('the message names the method, the id and the guide', () => {
  const msg = hints.peerAddressMisshapeMessage('user456', 'sendMessage');
  assert.match(msg, /sendMessage/);
  assert.match(msg, /user456/);
  assert.match(msg, /identity-for-integrators/);
});

test('a bad id warns once per method; a valid or empty id never warns', async () => {
  const protocol = fresh({ devPeerAddressWarnings: true });
  captureWarnings();
  try {
    await protocol.sendMessage({ recipient: 'user456', content: 'hi' });
    await protocol.sendMessage({ recipient: 'user456', content: 'hi' });
    await protocol.sendMessage({ recipient: VALID, content: 'hi' });
    await protocol.sendMessage({ recipient: '', content: 'hi' });
    await protocol.blockUser('user456');
  } finally {
    releaseWarnings();
  }
  assert.equal(warnings.length, 2, warnings.join('\n'));
  assert.match(warnings[0], /sendMessage/);
  assert.match(warnings[1], /blockUser/);
});

// Every call site reached without an OfflineProtocol method, plus one that is.
async function callEverySiteWithoutAConfig(protocol) {
  const store = new sdk.DataStore();
  const services = new sdk.MeshServices();
  await protocol.isUserBlocked('bob');
  await store.provideAttachment('space', 'bob', 'hash', '');
  await store.declineAttachment('space', 'bob', 'hash');
  await store.fetchAttachmentFrom('space', 'bob', 'hash');
  await services.sendServiceRequest('bob', 'svc', 'GET', '');
  await services.respondToServiceRequest('req', 'bob', 'ok', '');
  await services.subscribeService('bob', 'svc', 'topic');
}

const SITES = [
  'isUserBlocked',
  'provideAttachment',
  'declineAttachment',
  'fetchAttachmentFrom',
  'sendServiceRequest',
  'respondToServiceRequest',
  'subscribeService',
];

test('devPeerAddressWarnings: false silences DataStore and MeshServices in a dev build', async () => {
  // Lifecycle warnings are on in a dev build too; this test is about peer ids only.
  const protocol = fresh({ devPeerAddressWarnings: false, devLifecycleWarnings: false });
  globalThis.__DEV__ = true;
  captureWarnings();
  try {
    await callEverySiteWithoutAConfig(protocol);
  } finally {
    releaseWarnings();
    delete globalThis.__DEV__;
  }
  assert.deepEqual(warnings, []);
});

test('devPeerAddressWarnings: true reaches DataStore and MeshServices in a release build', async () => {
  const protocol = fresh({ devPeerAddressWarnings: true });
  globalThis.__DEV__ = false;
  captureWarnings();
  try {
    await callEverySiteWithoutAConfig(protocol);
  } finally {
    releaseWarnings();
    delete globalThis.__DEV__;
  }
  assert.equal(warnings.length, SITES.length, warnings.join('\n'));
  SITES.forEach((site, i) => assert.match(warnings[i], new RegExp(`\\] ${site}:`)));
});

test('the dedupe set is bounded and warnings still print past the bound', () => {
  fresh({ devPeerAddressWarnings: true });
  captureWarnings();
  try {
    for (let i = 0; i < hints.MAX_WARNED_KEYS + 10; i++) {
      hints.warnPeerIdIfMisshapen(`peer-${i}`, 'sendMessage');
    }
    hints.warnPeerIdIfMisshapen('peer-past-the-bound', 'sendMessage');
    hints.warnPeerIdIfMisshapen('peer-past-the-bound', 'sendMessage');
  } finally {
    releaseWarnings();
  }
  assert.equal(warnings.length, hints.MAX_WARNED_KEYS + 12);
});

(async () => {
  let failed = 0;
  for (const { name, fn } of tests) {
    try {
      await fn();
      console.log(`ok   ${name}`);
    } catch (err) {
      failed++;
      console.error(`FAIL ${name}\n${err && err.stack}`);
    }
  }
  console.log(`${tests.length - failed} passed, ${failed} failed.`);
  if (failed) process.exit(1);
})();
