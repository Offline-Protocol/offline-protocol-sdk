#!/usr/bin/env node
'use strict';

// The development transport hints (src/transportDiagnostics.ts): which native
// diagnostic messages turn into a console hint, how often, and what the hint
// says. The message strings and context keys are pinned against the native
// emit sites by the Rust guard transport_expectation_diagnostics_are_pinned_for_rn_hints.

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');

const PACKAGE_DIR = path.resolve(__dirname, '..');

function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc}: run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-tx-diag-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

const outDir = compileSdk();
const {
  devTransportDiagnosticsEnabled,
  maybeExplainTransportDiagnostic,
  bindDevTransportDiagnosticsFromConfig,
  resetTransportDiagnosticHintsForTests,
  TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_STARTED,
  TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_RESUMED,
  TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS,
  TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES,
} = require(path.join(outDir, 'transportDiagnostics.js'));

const logs = [];
const orig = console.warn;
console.warn = (msg) => logs.push(msg);

function emit(message, context) {
  maybeExplainTransportDiagnostic({ type: 'diagnostic', level: 'info', message, context });
}

function fresh() {
  logs.length = 0;
  resetTransportDiagnosticHintsForTests();
  bindDevTransportDiagnosticsFromConfig(true);
}

try {
  // Started and resumed share one hint, shown once.
  fresh();
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_STARTED);
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_STARTED);
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_RESUMED);
  assert.equal(logs.length, 1, 'formation started/resumed hint is shown once');
  assert.match(logs[0], /troubleshooting\.md#wifi-direct-is-slow-not-stuck$/);
  assert.ok(
    logs[0].includes('https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/'),
    'the link must resolve for an app that installed the package, outside the repo'
  );

  fresh();
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_RESUMED);
  assert.equal(logs.length, 1, 'resumed alone shows the formation hint');

  // Progress: one hint per report number, with the elapsed time.
  fresh();
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS, { elapsedMs: 61_000, report: 1 });
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS, { elapsedMs: 61_000, report: 1 });
  assert.equal(logs.length, 1, 'a repeated progress report is shown once');
  assert.ok(logs[0].includes('(61s elapsed)'), logs[0]);
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS, { elapsedMs: 125_000, report: 2 });
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS, { elapsedMs: 245_000, report: 3 });
  assert.equal(logs.length, 3, 'each report number is shown once');
  assert.ok(logs[2].includes('(245s elapsed)'), logs[2]);
  // A later attempt reuses report numbers: the hint set stays bounded.
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS, { elapsedMs: 63_000, report: 1 });
  assert.equal(logs.length, 3, 'a later attempt does not repeat the hints');

  fresh();
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS);
  assert.equal(logs.length, 1, 'progress without context still hints');
  assert.ok(!logs[0].includes('elapsed)'), 'no elapsed time is invented');
  emit(TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS, { report: 99 });
  assert.equal(logs.length, 1, 'an out-of-range report number shares the unnumbered key');

  // Several SDK apps: Android names the peer `address`, iOS `peripheral`.
  fresh();
  emit(TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES, { address: 'AA:BB:CC:DD:EE:FF', instances: 2 });
  assert.equal(logs.length, 1);
  assert.ok(logs[0].includes('Peer AA:BB:CC:DD:EE:FF is running several SDK apps'), logs[0]);
  assert.match(logs[0], /troubleshooting\.md#several-sdk-apps-on-one-phone$/);
  emit(TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES, { address: '11:22:33:44:55:66', instances: 2 });
  assert.equal(logs.length, 1, 'shown once per process, not once per (rotating) address');

  fresh();
  emit(TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES, {
    peripheral: '6F1C2A9E-0000-4000-8000-000000000001',
    instances: 2,
  });
  assert.ok(
    logs[0].includes('Peer 6F1C2A9E-0000-4000-8000-000000000001 is running'),
    `the iOS context key names the peer: ${logs[0]}`
  );

  fresh();
  emit(TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES);
  assert.ok(logs[0].startsWith('[OfflineProtocol] A peer is running'), logs[0]);

  // Unrelated diagnostics are ignored.
  fresh();
  emit('WiFi Direct transport stopped');
  assert.equal(logs.length, 0, 'an unrecognised diagnostic gives no hint');

  // The flag.
  fresh();
  bindDevTransportDiagnosticsFromConfig(false);
  emit(TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES, { address: 'aa:bb' });
  assert.equal(logs.length, 0, 'disabled flag silences hints');

  globalThis.__DEV__ = true;
  assert.equal(devTransportDiagnosticsEnabled(undefined), true, 'defaults on when __DEV__');
  assert.equal(devTransportDiagnosticsEnabled(false), false, 'false wins over __DEV__');
  // A later instance constructed without the flag goes back to the default.
  bindDevTransportDiagnosticsFromConfig(false);
  bindDevTransportDiagnosticsFromConfig(undefined);
  emit(TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES, { address: 'aa:bb' });
  assert.equal(logs.length, 1, 'an unset flag falls back to __DEV__');
  globalThis.__DEV__ = false;
  logs.length = 0;
  resetTransportDiagnosticHintsForTests();
  emit(TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES, { address: 'aa:bb' });
  assert.equal(logs.length, 0, 'an unset flag is off in a production build');
  bindDevTransportDiagnosticsFromConfig(true);
  emit(TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES, { address: 'aa:bb' });
  assert.equal(logs.length, 1, 'true opts a production build in');
  delete globalThis.__DEV__;
  assert.equal(devTransportDiagnosticsEnabled(undefined), false, 'defaults off without __DEV__');
} finally {
  console.warn = orig;
}
console.log('transport-diagnostics.test.js: ok');
