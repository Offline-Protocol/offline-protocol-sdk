#!/usr/bin/env node
'use strict';

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');

const PACKAGE_DIR = path.resolve(__dirname, '..');

function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run \`npm ci\` in ${PACKAGE_DIR} first.`);
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
  TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_STARTED,
  TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES,
} = require(path.join(outDir, 'transportDiagnostics.js'));

const logs = [];
const orig = console.warn;
console.warn = (msg) => logs.push(msg);

bindDevTransportDiagnosticsFromConfig(true);

maybeExplainTransportDiagnostic({
  type: 'diagnostic',
  level: 'info',
  message: TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_STARTED,
});
maybeExplainTransportDiagnostic({
  type: 'diagnostic',
  level: 'info',
  message: TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_STARTED,
});
assert.equal(logs.length, 1, 'wifi direct formation hint dedupes');

logs.length = 0;
maybeExplainTransportDiagnostic({
  type: 'diagnostic',
  level: 'info',
  message: TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES,
  context: { address: 'aa:bb' },
});
assert.ok(logs[0].includes('several SDK apps'));

bindDevTransportDiagnosticsFromConfig(false);
logs.length = 0;
maybeExplainTransportDiagnostic({
  type: 'diagnostic',
  level: 'info',
  message: TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES,
});
assert.equal(logs.length, 0, 'disabled flag silences hints');

globalThis.__DEV__ = true;
assert.equal(devTransportDiagnosticsEnabled(undefined), true, 'defaults on when __DEV__');
delete globalThis.__DEV__;
assert.equal(devTransportDiagnosticsEnabled(undefined), false, 'defaults off without __DEV__');

console.warn = orig;
console.log('transport-diagnostics.test.js: ok');
