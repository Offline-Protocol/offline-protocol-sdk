#!/usr/bin/env node
/**
 * Behavioral tests for the priority argument `forwardMessage` hands to the
 * native module (`src/index.ts`).
 *
 * A nullable number cannot cross this bridge. React Native forces every
 * `NSNumber` argument to non-null, because numbers are not nullable on
 * Android, and refuses a null one before the Swift method is entered, so
 * neither the resolver nor the rejecter runs and the promise never settles.
 * This layer passed `null` for an omitted priority until #417, which hung
 * `forwardMessage` forever on iOS debug builds. The repair was to resolve the
 * documented default here instead, so what needs pinning is on this side of
 * the bridge: the argument is always a number, and it is the right one.
 *
 * The Rust guard cannot cover this. A nullable number and a nullable object
 * share an ABI class, so both bridge halves agree while React Native rejects
 * the call anyway.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-fwd-'));
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

/** The argument list of every `forwardMessage` call the SDK made. */
let forwardCalls = [];

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      return (...args) => {
        if (method === 'forwardMessage') forwardCalls.push(args);
        const override = nativeOverrides[method];
        return override ? override(...args) : Promise.resolve();
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
const ORIGINAL_JSON = '{"id":"m1","content":"hello"}';

/** The slot the priority occupies in the native call. */
const PRIORITY_ARG = 2;

let OfflineProtocol;
let MessagePriority;

function newSdk() {
  return new OfflineProtocol({ appId: 'harness', profile: 'harness-profile' });
}

function forwardWith(params) {
  return newSdk().forwardMessage({
    originalMessageJson: ORIGINAL_JSON,
    newRecipient: RECIPIENT,
    ...params,
  });
}

// ---------------------------------------------------------------------------
// The argument is a number, never null
// ---------------------------------------------------------------------------

test('an omitted priority crosses as the Medium integer, not null', async () => {
  await forwardWith({});

  const priority = forwardCalls[0][PRIORITY_ARG];
  assert.equal(
    priority,
    MessagePriority.Medium,
    'the core resolves an absent priority to Medium, so this layer sends it explicitly'
  );
  assert.equal(
    typeof priority,
    'number',
    'React Native refuses a null number argument before the Swift method runs, and the promise then never settles (#417)'
  );
});

test('every priority in the enum crosses as a number', async () => {
  for (const name of ['Low', 'Medium', 'High', 'Critical']) {
    forwardCalls = [];
    await forwardWith({ priority: MessagePriority[name] });

    const priority = forwardCalls[0][PRIORITY_ARG];
    assert.equal(priority, MessagePriority[name], `${name} must cross unchanged`);
    assert.equal(typeof priority, 'number', `${name} must cross as a number`);
  }
});

// ---------------------------------------------------------------------------
// Zero is a priority, not an absence
// ---------------------------------------------------------------------------

test('Low survives the default rather than being read as absent', async () => {
  await forwardWith({ priority: MessagePriority.Low });

  // `MessagePriority.Low` is 0. Resolving the default with `||` instead of
  // `??` silently upgrades every Low forward to Medium, which no caller can
  // see: the message sends either way, just at the wrong priority.
  assert.equal(
    forwardCalls[0][PRIORITY_ARG],
    MessagePriority.Low,
    'Low is 0 and must not be treated as an unset priority'
  );
});

// ---------------------------------------------------------------------------
// The rest of the call is unchanged
// ---------------------------------------------------------------------------

test('the message and recipient reach native alongside the priority', async () => {
  nativeOverrides.forwardMessage = () => Promise.resolve('new-id');

  const messageId = await forwardWith({ priority: MessagePriority.High });

  assert.deepEqual(forwardCalls[0], [ORIGINAL_JSON, RECIPIENT, MessagePriority.High]);
  assert.equal(messageId, 'new-id', 'the native message id is returned to the caller');
});

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

(async () => {
  const outDir = compileSdk();
  try {
    ({ OfflineProtocol, MessagePriority } = require(path.join(outDir, 'index.js')));

    let failed = 0;
    for (const { name, fn } of tests) {
      nativeOverrides = { isMlsInitialized: () => Promise.resolve(true) };
      forwardCalls = [];
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
