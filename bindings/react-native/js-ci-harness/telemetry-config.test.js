#!/usr/bin/env node
/**
 * Behavioral tests for the JS-layer marshalling of the telemetry surface
 * (`src/index.ts`).
 *
 * Drives the real compiled SDK against a stubbed native module and asserts
 * on the payloads it hands over. The failure this guards against is the
 * one every config bridge has had at least once: a field the app sets that
 * the bridge fills in with a literal, or drops, on its way to native. So the
 * assertions check absence as hard as presence: a field the app did not set
 * must not appear, because a bridge that materialised `debug: false` would
 * make the Rust default unreachable and nothing would report it.
 *
 * The sibling Rust guard `every_bridge_reads_the_telemetry_config_section`
 * pins that both native parsers read every field this file proves JS sends,
 * and that neither reads the platform fields the app must not set.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-telemetry-'));
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

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

let OfflineProtocol;

const newSdk = () => new OfflineProtocol({ appId: 'harness', profile: 'harness-profile' });

function onlyCall(method) {
  const matches = nativeCalls.filter((c) => c.method === method);
  assert.equal(matches.length, 1, `expected exactly one ${method} call, saw ${matches.length}`);
  return matches[0];
}

// ---------------------------------------------------------------------------
// enableTelemetry forwards the config verbatim
// ---------------------------------------------------------------------------

test('every configured field reaches native under its own name', async () => {
  const sdk = newSdk();
  const config = {
    apiKey: 'mp_key',
    appId: 'app_1',
    appVersion: '1.4.2',
    debug: true,
    flushIntervalMs: 5000,
    maxBatchBytes: 4096,
    maxBufferedRecords: 64,
    includeDeviceId: true,
    scrubIds: false,
    mlsVerbosity: 'diagnostic',
    metricsCadenceMs: 1000,
    routingDiagnostic: true,
    mlsSamplingBypass: true,
  };
  await sdk.enableTelemetry(config);
  assert.deepEqual(onlyCall('enableTelemetry').args, [config]);
});

test('a field the app did not set is absent, not defaulted, on the way to native', async () => {
  // Absence is the assertion. A bridge that sent `debug: false` or
  // `flushIntervalMs: 30000` here would make the Rust default unreachable.
  const sdk = newSdk();
  await sdk.enableTelemetry({ apiKey: 'mp_key', appId: 'app_1' });
  const [payload] = onlyCall('enableTelemetry').args;
  assert.deepEqual(Object.keys(payload).sort(), ['apiKey', 'appId']);
});

test('the platform fields are never sent by JS, so the native module fills them', async () => {
  const sdk = newSdk();
  await sdk.enableTelemetry({ apiKey: 'mp_key', appId: 'app_1' });
  const [payload] = onlyCall('enableTelemetry').args;
  assert.equal('os' in payload, false);
  assert.equal('osMajor' in payload, false);
});

// ---------------------------------------------------------------------------
// The rest of the surface is a thin pass-through
// ---------------------------------------------------------------------------

test('disable, flush, endSession and setEnabled each map to one native call', async () => {
  const sdk = newSdk();
  await sdk.disableTelemetry();
  await sdk.flushTelemetry();
  await sdk.endTelemetrySession();
  await sdk.setTelemetryEnabled(false);
  onlyCall('disableTelemetry');
  onlyCall('flushTelemetry');
  onlyCall('endTelemetrySession');
  assert.deepEqual(onlyCall('setTelemetryEnabled').args, [false]);
});

test('telemetryStats passes the native snapshot through and reads absence as null', async () => {
  nativeOverrides.telemetryStats = () =>
    Promise.resolve({
      buffered: 3,
      sentEvents: 40,
      acceptedEvents: 39,
      dropped: 1,
      sessionId: '6ba7b810-9dad-11d1-80b4-00c04fd430c8',
      lastFlushAtMs: 1747000000000,
    });
  const sdk = newSdk();
  const stats = await sdk.telemetryStats();
  assert.equal(stats.acceptedEvents, 39);
  assert.equal(stats.lastError, undefined);

  nativeCalls = [];
  nativeOverrides.telemetryStats = () => Promise.resolve(null);
  assert.equal(await sdk.telemetryStats(), null);
  nativeOverrides.telemetryStats = () => Promise.resolve(undefined);
  assert.equal(await sdk.telemetryStats(), null);
});

test('the sink API is gone: nothing in the SDK reaches for a telemetry event stream', async () => {
  const sdk = newSdk();
  for (const gone of ['installTelemetrySink', 'uninstallTelemetrySink', 'pollTelemetry', 'onTelemetry']) {
    assert.equal(typeof sdk[gone], 'undefined', `${gone} must not exist`);
  }
});

test('destroy does not touch telemetry: the pipe is the native instance\'s to stop', async () => {
  const sdk = newSdk();
  await sdk.enableTelemetry({ apiKey: 'mp_key', appId: 'app_1' });
  nativeCalls = [];
  await sdk.destroy();
  assert.equal(
    nativeCalls.some((c) => c.method === 'disableTelemetry'),
    false,
    'destroy must not disable telemetry behind the app\'s back; the native destroy stops the pipe'
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
      try {
        await fn();
        console.log(`  ✓ ${name}`);
      } catch (error) {
        failed += 1;
        console.log(`  ✗ ${name}\n      ${error.message}`);
      }
    }

    console.log(failed === 0 ? `\n${tests.length} passed.` : `\n${failed} of ${tests.length} FAILED.`);
    process.exitCode = failed === 0 ? 0 : 1;
  } finally {
    fs.rmSync(outDir, { recursive: true, force: true });
  }
})();
