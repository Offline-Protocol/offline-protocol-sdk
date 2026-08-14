#!/usr/bin/env node
/**
 * Behavioral tests for the JS-layer marshalling of the relay and DORS
 * configuration (`src/index.ts`).
 *
 * Drives the *real compiled* `OfflineProtocol` class against a stubbed native
 * module and asserts on the payloads it hands over, because every failure in
 * this layer is silent: a field dropped between JS and native leaves the
 * engine on its default with no error anywhere, which is exactly how
 * `allowRelay`, `minBatteryForRelay` and `relayThreshold` came to be
 * documented, accepted, carried across the bridge — and parsed by nothing.
 *
 * The sibling Rust guard `react_native_bridges_merge_dors_updates_from_the_live_config`
 * pins that the two native bridges merge a partial update onto the live
 * config; only this can tell you the partial update the SDK sends is partial
 * in the first place. Both halves are required: a payload that names every
 * field would make the merge moot, and a merge that defaulted to literals
 * would make the partial payload a silent reset.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-relay-'));
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
/** Every native call the SDK made, in order: `{ method, args }`. */
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

// ---------------------------------------------------------------------------
// Scaffolding
// ---------------------------------------------------------------------------

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

const newSdk = (config = {}) =>
  new OfflineProtocol({ appId: 'harness', profile: 'harness-profile', ...config });

/** The single call to `method`, asserting it happened exactly once. */
function onlyCall(method) {
  const matches = nativeCalls.filter((c) => c.method === method);
  assert.equal(matches.length, 1, `expected exactly one ${method} call, saw ${matches.length}`);
  return matches[0];
}

/** The JSON payload of the single call to `method`, parsed. */
function payloadOf(method) {
  return JSON.parse(onlyCall(method).args[0]);
}

// ---------------------------------------------------------------------------
// The create-time relay section
// ---------------------------------------------------------------------------

test('every relay field configured at create time reaches native', async () => {
  const sdk = newSdk({
    relay: {
      allowRelay: false,
      minBatteryForRelay: 55,
      relayThreshold: 9,
      relayPriority: 'never',
    },
  });
  await sdk.start();

  assert.deepEqual(payloadOf('updateRelayConfig'), {
    allowRelay: false,
    minBatteryForRelay: 55,
    relayThreshold: 9,
    relayPriority: 'never',
  });
});

test('a relay section without a priority still crosses the bridge', async () => {
  // The old gate was `if (relay?.relayPriority)`, so a config that set only a
  // battery floor was dropped whole — the shape most apps actually write.
  const sdk = newSdk({ relay: { minBatteryForRelay: 55 } });
  await sdk.start();

  assert.deepEqual(payloadOf('updateRelayConfig'), { minBatteryForRelay: 55 });
});

test('the legacy priority spelling still maps to the engine vocabulary', async () => {
  const sdk = newSdk({ relay: { relayPriority: 'medium' } });
  await sdk.start();

  assert.deepEqual(payloadOf('updateRelayConfig'), { relayPriority: 'auto' });
});

test('an unparseable priority does not poison the rest of the section', async () => {
  const sdk = newSdk({ relay: { allowRelay: true, relayPriority: 'sometimes' } });
  await sdk.start();

  assert.deepEqual(
    payloadOf('updateRelayConfig'),
    { allowRelay: true },
    'an unrecognised priority is dropped, not forwarded and not fatal to its siblings'
  );
});

test('no relay section means no relay call at all', async () => {
  const sdk = newSdk({});
  await sdk.start();

  assert.equal(
    nativeCalls.filter((c) => c.method === 'updateRelayConfig').length,
    0,
    'an app that configured nothing must not have its defaults restated over the bridge'
  );
});

// ---------------------------------------------------------------------------
// The runtime surface
// ---------------------------------------------------------------------------

test('updateRelayConfig sends only the fields it was given', async () => {
  // The bridges merge what arrives onto the live config, so an absent field
  // has to *stay* absent: serialising it as a default would silently overwrite
  // whatever the app configured at create time.
  const sdk = newSdk({});
  await sdk.updateRelayConfig({ minBatteryForRelay: 41 });

  assert.deepEqual(payloadOf('updateRelayConfig'), { minBatteryForRelay: 41 });
});

test('setRelayPriority normalises legacy input before it reaches native', async () => {
  const sdk = newSdk({});
  await sdk.setRelayPriority('high');

  assert.deepEqual(onlyCall('setRelayPriority').args, ['always']);
});

test('setRelayPriority rejects an unknown priority instead of forwarding it', async () => {
  const sdk = newSdk({});
  await assert.rejects(
    () => sdk.setRelayPriority('turbo'),
    /Invalid relay priority: turbo/,
    'forwarding it would land on the native default and silently change relay behaviour'
  );
  assert.equal(nativeCalls.filter((c) => c.method === 'setRelayPriority').length, 0);
});

test('getRelayConfig parses the JSON string both bridges return', async () => {
  nativeOverrides.getRelayConfig = () =>
    Promise.resolve(
      JSON.stringify({
        relayThreshold: 6,
        minBatteryForRelay: 41,
        allowRelay: true,
        relayPriority: 'auto',
      })
    );
  const sdk = newSdk({});

  assert.deepEqual(await sdk.getRelayConfig(), {
    relayThreshold: 6,
    minBatteryForRelay: 41,
    allowRelay: true,
    relayPriority: 'auto',
  });
});

// ---------------------------------------------------------------------------
// The DORS battery fields
// ---------------------------------------------------------------------------

test('the battery and relay DORS fields reach native, clamped', async () => {
  // These three had no path across the bridge at all, so *any* runtime DORS
  // update reset them to 20/30/4 — including one meaning to change something
  // else entirely.
  const sdk = newSdk({});
  await sdk.updateDorsConfig({
    lowBatteryThreshold: 250,
    relayMinBatteryLevel: -5,
    relayOptimalConnectionCount: 999,
  });

  assert.deepEqual(payloadOf('updateDorsConfig'), {
    lowBatteryThreshold: 100,
    relayMinBatteryLevel: 0,
    relayOptimalConnectionCount: 255,
  });
});

test('a DORS update naming one field sends only that field', async () => {
  const sdk = newSdk({});
  await sdk.updateDorsConfig({ stabilityWindowSecs: 33 });

  assert.deepEqual(
    payloadOf('updateDorsConfig'),
    { stabilityWindowSecs: 33 },
    'the bridges fill every absent field from the live config; naming one here is what makes ' +
      'a partial update partial rather than a full overwrite'
  );
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
