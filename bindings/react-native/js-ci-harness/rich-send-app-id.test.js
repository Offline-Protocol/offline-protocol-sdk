#!/usr/bin/env node
/**
 * Behavioral tests for the per-send app label the rich sends hand to the
 * native module (`src/index.ts`), and for `appId`, its deprecated old name,
 * on the config and on each send.
 *
 * Only the rich native methods carry `app_label`, and `sendMessage` and
 * `sendMedia` take the rich path only when one of a listed set of params is
 * present. A label missing from that list would send a label-only call
 * down the plain path, where the message goes out under the configured label
 * and nothing reports it: the receiving instance simply routes it to the
 * wrong application. The Rust guard pins the spelling of the list; this
 * pins the behaviour.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-appid-'));
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

/** Every native call the SDK made, as `[method, args]`. */
let calls = [];

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      return (...args) => {
        calls.push([method, args]);
        const override = nativeOverrides[method];
        return override ? override(...args) : Promise.resolve('id');
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

const RECIPIENT = 'off1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqa';

let OfflineProtocol;
let ContentType;

function newSdk() {
  return new OfflineProtocol({ appLabel: 'harness', profile: 'harness-profile' });
}

/** The one send the SDK handed to native, whichever method it chose. */
function theSend() {
  const sends = calls.filter(([method]) => method.startsWith('send'));
  assert.equal(sends.length, 1, `expected one native send, got ${JSON.stringify(sends)}`);
  return sends[0];
}

const media = (params) => ({
  recipient: RECIPIENT,
  fileData: 'AAAA',
  fileName: 'photo.jpg',
  contentType: ContentType.Image,
  ...params,
});

// ---------------------------------------------------------------------------
// sendMessage
// ---------------------------------------------------------------------------

test('an appLabel-only sendMessage takes the rich path and carries the label', async () => {
  await newSdk().sendMessage({ recipient: RECIPIENT, content: 'hi', appLabel: 'other-app' });

  const [method, args] = theSend();
  assert.equal(method, 'sendMessageRich', 'only the rich native method carries app_label');
  assert.equal(args[4].app_label, 'other-app');
});

test('the old appId name on a send still carries the label', async () => {
  await newSdk().sendMessage({ recipient: RECIPIENT, content: 'hi', appId: 'other-app' });

  const [method, args] = theSend();
  assert.equal(method, 'sendMessageRich');
  assert.equal(args[4].app_label, 'other-app');
});

test('appLabel and appId that disagree on a send are refused before native', async () => {
  await assert.rejects(
    newSdk().sendMessage({ recipient: RECIPIENT, content: 'hi', appLabel: 'a', appId: 'b' }),
    /disagree/
  );
  assert.equal(calls.filter(([method]) => method.startsWith('send')).length, 0);
});

test('a null under either name is not set, so it is no conflict', async () => {
  await newSdk().sendMessage({ recipient: RECIPIENT, content: 'hi', appLabel: 'other-app', appId: null });
  const [, args] = theSend();
  assert.equal(args[4].app_label, 'other-app');

  calls.length = 0;
  await newSdk().sendMessage({ recipient: RECIPIENT, content: 'hi', appLabel: null, appId: 'other-app' });
  const [, oldArgs] = theSend();
  assert.equal(oldArgs[4].app_label, 'other-app');
});

test('a rich sendMessage without a label sends app_label null, not a stale value', async () => {
  await newSdk().sendMessage({
    recipient: RECIPIENT,
    content: 'hi',
    contentType: ContentType.Image,
  });

  const [method, args] = theSend();
  assert.equal(method, 'sendMessageRich');
  assert.equal(args[4].app_label, null, 'null falls back to the configured label in the core');
});

test('a plain sendMessage stays on the plain native method', async () => {
  await newSdk().sendMessage({ recipient: RECIPIENT, content: 'hi' });

  assert.equal(theSend()[0], 'sendMessage');
});

// ---------------------------------------------------------------------------
// sendMedia
// ---------------------------------------------------------------------------

test('an appLabel-only sendMedia takes the rich path and carries the label', async () => {
  await newSdk().sendMedia(media({ appLabel: 'other-app' }));

  const [method, args] = theSend();
  assert.equal(method, 'sendMediaRich', 'only the rich native method carries app_label');
  assert.equal(args[4].app_label, 'other-app');
});

test('the old appId name on a media send still carries the label', async () => {
  await newSdk().sendMedia(media({ appId: 'other-app' }));

  assert.equal(theSend()[1][4].app_label, 'other-app');
});

test('a plain sendMedia stays on the plain native method', async () => {
  await newSdk().sendMedia(media({}));

  assert.equal(theSend()[0], 'sendMedia');
});

// ---------------------------------------------------------------------------
// The configured label
// ---------------------------------------------------------------------------

/** The config the SDK handed to native on create, parsed. */
async function createdConfig(config) {
  await new OfflineProtocol(config).start();
  const created = calls.find(([method]) => method === 'create');
  assert.ok(created, `no create call in ${JSON.stringify(calls.map(([m]) => m))}`);
  const arg = created[1][0];
  return typeof arg === 'string' ? JSON.parse(arg) : arg;
}

test('the config hands native appLabel', async () => {
  const native = await createdConfig({ appLabel: 'app.muster', profile: 'p' });
  assert.equal(native.appLabel, 'app.muster');
});

test('a config with only the old appId keeps its value', async () => {
  const native = await createdConfig({ appId: 'app.muster', profile: 'p' });
  assert.equal(native.appLabel, 'app.muster', 'a shipped app must keep its storage namespace');
});

test('a config that disagrees, or names no label, is refused', () => {
  assert.throws(
    () => new OfflineProtocol({ appLabel: 'a', appId: 'b', profile: 'p' }),
    /disagree/
  );
  assert.throws(() => new OfflineProtocol({ profile: 'p' }), /appLabel is required/);
});

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

(async () => {
  const outDir = compileSdk();
  try {
    ({ OfflineProtocol, ContentType } = require(path.join(outDir, 'index.js')));

    let failed = 0;
    for (const { name, fn } of tests) {
      nativeOverrides = { isMlsInitialized: () => Promise.resolve(true) };
      calls = [];
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
