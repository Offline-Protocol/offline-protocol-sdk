#!/usr/bin/env node
/**
 * Behavioral tests for `signDataBytes` — the byte-oriented sibling of
 * `signData` that crosses the legacy React Native bridge as a base64
 * string instead of a boxed `number[]`.
 *
 * The motivation: on `n`-byte payloads, `signData(number[])` allocates
 * ~8n bytes of JS heap (each byte becomes an f64-backed Number) because
 * the legacy React Native bridge serializes `NSArray<NSNumber *>` /
 * `ReadableArray` element by element. For `n = 55 MB` (a representative
 * LoRA delta on gemma-270m) that is ~440 MB of JS heap, which exceeds
 * the Dalvik growth limit on mid-tier Android devices and OOMs before
 * the native call starts.
 *
 * `signDataBytes` base64-encodes once on the JS side and crosses the
 * bridge as a single string. Peak JS heap drops from ~8n bytes to
 * ~2.33n bytes. The native side decodes to a byte array (one
 * allocation) and signs through the exact same Rust / UniFFI path that
 * backs `signData`, so Ed25519 signatures are byte-identical for the
 * same input.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-signbytes-'));
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

/**
 * Deterministic fake signature so byte-identity between the two call
 * paths is testable without a real Ed25519 keypair. The native side
 * normally signs the raw bytes, so the stub just returns a
 * length-stamped digest of the input bytes that happens to be the same
 * for `signData` and `signDataBase64` on the same input.
 */
function fakeSignature(bytes) {
  const out = new Uint8Array(64);
  for (let i = 0; i < bytes.length; i++) {
    out[i % 64] = (out[i % 64] + bytes[i]) & 0xff;
  }
  // Stamp the length into the last 4 bytes so a shorter input that
  // happens to produce the same byte-sum is still distinguishable.
  out[60] = (bytes.length >>> 24) & 0xff;
  out[61] = (bytes.length >>> 16) & 0xff;
  out[62] = (bytes.length >>> 8) & 0xff;
  out[63] = bytes.length & 0xff;
  return out;
}

let signDataCalls = 0;
let signDataBase64Calls = 0;
let lastSignDataArg = null;
let lastSignDataBase64Arg = null;

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      return (...args) => {
        if (method === 'signData') {
          signDataCalls += 1;
          lastSignDataArg = args[0];
          const bytes = Uint8Array.from(args[0]);
          const sig = fakeSignature(bytes);
          return Promise.resolve(Array.from(sig));
        }
        if (method === 'signDataBase64') {
          signDataBase64Calls += 1;
          lastSignDataBase64Arg = args[0];
          const binary = Buffer.from(args[0], 'base64');
          const bytes = new Uint8Array(binary.buffer, binary.byteOffset, binary.byteLength);
          const sig = fakeSignature(bytes);
          return Promise.resolve(Buffer.from(sig).toString('base64'));
        }
        return Promise.resolve();
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

function newSdk() {
  return new OfflineProtocol({ appId: 'harness', profile: 'harness-profile' });
}

let OfflineProtocol;

// ---------------------------------------------------------------------------
// Byte-identity: signDataBytes(bytes) === signData(Array.from(bytes))
// ---------------------------------------------------------------------------

test('byte-identity on a tiny payload', async () => {
  const sdk = newSdk();
  const bytes = new Uint8Array([0, 1, 2, 254, 255, 0x7a]);

  const viaBytes = await sdk.signDataBytes(bytes);
  const viaNumbers = await sdk.signData(Array.from(bytes));

  assert.equal(signDataBase64Calls, 1, 'signDataBytes must route through signDataBase64');
  assert.equal(signDataCalls, 1, 'signData must route through the legacy number[] path');

  assert.ok(viaBytes instanceof Uint8Array, 'signDataBytes must return a Uint8Array');
  assert.equal(viaBytes.length, 64, 'Ed25519 signatures are 64 bytes');
  assert.deepEqual(
    Array.from(viaBytes),
    viaNumbers,
    'the same input bytes must produce the same signature regardless of entry point'
  );
});

test('byte-identity on a payload that spans a chunk boundary (32 KB + overlap)', async () => {
  const sdk = newSdk();
  // 100 KB spans past BASE64_CHUNK_SIZE (32,766 bytes) three times, so
  // chunk joining is exercised and any boundary bug would surface.
  const bytes = new Uint8Array(100_000);
  for (let i = 0; i < bytes.length; i++) bytes[i] = (i * 31 + 7) & 0xff;

  const viaBytes = await sdk.signDataBytes(bytes);
  const viaNumbers = await sdk.signData(Array.from(bytes));

  assert.deepEqual(
    Array.from(viaBytes),
    viaNumbers,
    'chunked base64 encode must preserve byte identity across chunk joins'
  );
});

test('byte-identity on empty input', async () => {
  const sdk = newSdk();
  const bytes = new Uint8Array(0);

  const viaBytes = await sdk.signDataBytes(bytes);
  const viaNumbers = await sdk.signData(Array.from(bytes));

  assert.deepEqual(Array.from(viaBytes), viaNumbers, 'empty-input signature must agree');
});

// ---------------------------------------------------------------------------
// The native-bridge argument must be a single base64 string, not a boxed array
// ---------------------------------------------------------------------------

test('signDataBytes crosses the bridge as one base64 string', async () => {
  const sdk = newSdk();
  const bytes = new Uint8Array([10, 20, 30, 40, 50]);
  await sdk.signDataBytes(bytes);

  assert.equal(typeof lastSignDataBase64Arg, 'string', 'the native arg must be a string');
  // 5 bytes encode to 8 base64 chars (with padding).
  assert.equal(lastSignDataBase64Arg.length, 8, 'base64 of 5 bytes is 8 chars');
  // Round-trip decode must match the input exactly.
  const decoded = Uint8Array.from(Buffer.from(lastSignDataBase64Arg, 'base64'));
  assert.deepEqual(Array.from(decoded), [10, 20, 30, 40, 50]);
});

test('signData still crosses the bridge as number[] (legacy path unchanged)', async () => {
  const sdk = newSdk();
  await sdk.signData([1, 2, 3]);

  assert.ok(Array.isArray(lastSignDataArg), 'legacy signData must still receive a number[]');
  assert.deepEqual(lastSignDataArg, [1, 2, 3]);
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
      signDataCalls = 0;
      signDataBase64Calls = 0;
      lastSignDataArg = null;
      lastSignDataBase64Arg = null;
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

    if (failed > 0) {
      realConsole.log(`\n${failed} test(s) failed.`);
      process.exit(1);
    }
    realConsole.log(`\nAll ${tests.length} tests passed.`);
  } finally {
    fs.rmSync(outDir, { recursive: true, force: true });
  }
})().catch((error) => {
  realConsole.log(`fatal: ${error.stack || error.message}`);
  process.exit(1);
});
