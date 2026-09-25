#!/usr/bin/env node
/**
 * Behavioral tests for the per-send application id the rich sends hand to
 * the native module (`src/index.ts`).
 *
 * Only the rich native methods carry `app_id`, and `sendMessage` and
 * `sendMedia` take the rich path only when one of a listed set of params is
 * present. An `appId` missing from that list would send an appId-only call
 * down the plain path, where the message goes out under the configured id
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
  return new OfflineProtocol({ appId: 'harness', profile: 'harness-profile' });
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

test('an appId-only sendMessage takes the rich path and carries the id', async () => {
  await newSdk().sendMessage({ recipient: RECIPIENT, content: 'hi', appId: 'other-app' });

  const [method, args] = theSend();
  assert.equal(method, 'sendMessageRich', 'only the rich native method carries app_id');
  assert.equal(args[4].app_id, 'other-app');
});

test('a rich sendMessage without appId sends app_id null, not a stale value', async () => {
  await newSdk().sendMessage({
    recipient: RECIPIENT,
    content: 'hi',
    contentType: ContentType.Image,
  });

  const [method, args] = theSend();
  assert.equal(method, 'sendMessageRich');
  assert.equal(args[4].app_id, null, 'null falls back to the configured id in the core');
});

test('a plain sendMessage stays on the plain native method', async () => {
  await newSdk().sendMessage({ recipient: RECIPIENT, content: 'hi' });

  assert.equal(theSend()[0], 'sendMessage');
});

// ---------------------------------------------------------------------------
// sendMedia
// ---------------------------------------------------------------------------

test('an appId-only sendMedia takes the rich path and carries the id', async () => {
  await newSdk().sendMedia(media({ appId: 'other-app' }));

  const [method, args] = theSend();
  assert.equal(method, 'sendMediaRich', 'only the rich native method carries app_id');
  assert.equal(args[4].app_id, 'other-app');
});

test('a plain sendMedia stays on the plain native method', async () => {
  await newSdk().sendMedia(media({}));

  assert.equal(theSend()[0], 'sendMedia');
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
