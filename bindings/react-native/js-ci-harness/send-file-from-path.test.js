#!/usr/bin/env node
/**
 * Behavioural tests for the streaming file transport (`sendFileFromPath`
 * in `src/index.ts`).
 *
 * `sendFileFromPath` exists so React Native consumers can hand the SDK a
 * filesystem path instead of base64 bytes. The whole point is that the
 * full payload never crosses the JS/native bridge as a JS string, so
 * these tests pin two shapes:
 *
 *   1. The native call carries exactly the three forwarded arguments
 *      (recipient, filePath, fileName) in order. If this drifts, Kotlin
 *      and Swift silently fail at the bridge decode and the caller gets
 *      a cryptic native error.
 *   2. There is no base64 handling or `sendMedia`-style path crossing on
 *      the JS layer. The whole value of the API is lost the moment the
 *      JS wrapper materialises the bytes itself.
 *
 * See one-shot-hold.test.js for the stubbed-native-module harness pattern.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-sfp-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

let nativeOverrides = {};
let calls = [];

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      return (...args) => {
        calls.push([method, args]);
        const override = nativeOverrides[method];
        return override ? override(...args) : Promise.resolve('file-id-stub');
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
const FILE_PATH = '/tmp/some-delta.lora.json';
const FILE_NAME = 'training-crdt-abc-def.lora.json';

let OfflineProtocol;

function newSdk() {
  return new OfflineProtocol({ appId: 'harness', profile: 'harness-profile' });
}

function theSend() {
  const sends = calls.filter(([method]) => method.startsWith('send'));
  assert.equal(sends.length, 1, `expected one native send, got ${JSON.stringify(sends)}`);
  return sends[0];
}

test('sendFileFromPath routes to the native sendFileFromPath with three ordered args', async () => {
  const sdk = newSdk();
  const id = await sdk.sendFileFromPath({
    recipient: RECIPIENT,
    filePath: FILE_PATH,
    fileName: FILE_NAME,
  });
  assert.equal(id, 'file-id-stub');
  const [method, args] = theSend();
  assert.equal(method, 'sendFileFromPath', 'must not fall through to sendFile / sendMedia');
  assert.deepEqual(args, [RECIPIENT, FILE_PATH, FILE_NAME],
    'arg order is (recipient, filePath, fileName); drift here crashes the native decode');
});

test('sendFileFromPath never crosses base64 bytes', async () => {
  const sdk = newSdk();
  await sdk.sendFileFromPath({
    recipient: RECIPIENT,
    filePath: FILE_PATH,
    fileName: FILE_NAME,
  });
  const [, args] = theSend();
  // A path is small (bytes), base64 of a real payload is thousands+. If
  // this invariant breaks the whole purpose of the API is defeated: the
  // base64 bridge crossing is the OOM pathology this exists to avoid.
  for (const arg of args) {
    assert.equal(typeof arg, 'string');
    assert.ok(arg.length < 4096, `arg ${JSON.stringify(arg)} is suspiciously long for a path-only API`);
  }
});

test('sendFileFromPath surfaces a native rejection unchanged', async () => {
  nativeOverrides.sendFileFromPath = () =>
    Promise.reject(new Error('InvalidArgument: cannot read /nope: No such file or directory'));
  const sdk = newSdk();
  await assert.rejects(
    () => sdk.sendFileFromPath({
      recipient: RECIPIENT,
      filePath: '/nope',
      fileName: 'x',
    }),
    /cannot read \/nope/,
  );
});

test('sendFileFromPath forwards the exact file name advertised, not the basename of the path', async () => {
  // The companion-app's CRDT transport names files with a deterministic
  // shape derived from `(spaceId, contributionId)`, which may not match
  // the basename of `filePath` on disk. The native side must advertise
  // the caller-supplied name verbatim.
  const sdk = newSdk();
  await sdk.sendFileFromPath({
    recipient: RECIPIENT,
    filePath: '/tmp/renamed-on-disk.bin',
    fileName: 'training-crdt-space.contrib.lora.json',
  });
  const [, args] = theSend();
  assert.equal(args[2], 'training-crdt-space.contrib.lora.json');
});

test('legacy sendFile is unchanged and still routes through sendMedia', async () => {
  // Compatibility pin: the new API is additive. The existing sendFile
  // consumer path (which the companion-app will migrate off over time)
  // must keep working against the base64 contract.
  const sdk = newSdk();
  await sdk.sendFile({
    recipient: RECIPIENT,
    fileData: 'AAAA',
    fileName: 'legacy.bin',
  });
  const [method] = theSend();
  // sendFile delegates to sendMedia on the JS side; what native sees is
  // sendMedia. If this ever starts hitting sendFile/sendFileFromPath
  // directly, update this test and the consumer-migration note.
  assert.equal(method, 'sendMedia');
});

(async () => {
  const outDir = compileSdk();
  try {
    ({ OfflineProtocol } = require(path.join(outDir, 'index.js')));

    let failed = 0;
    for (const { name, fn } of tests) {
      nativeOverrides = { isMlsInitialized: () => Promise.resolve(true) };
      calls = [];
      captureConsole();
      try {
        await fn();
        releaseConsole();
        realConsole.log(`  ok ${name}`);
      } catch (error) {
        failed += 1;
        releaseConsole();
        realConsole.log(`  FAIL ${name}\n      ${error.message}`);
      }
    }

    realConsole.log(
      failed === 0
        ? `\n${tests.length} passed.`
        : `\n${failed} of ${tests.length} FAILED.`
    );
    process.exitCode = failed === 0 ? 0 : 1;
  } finally {
    releaseConsole();
    fs.rmSync(outDir, { recursive: true, force: true });
  }
})();
