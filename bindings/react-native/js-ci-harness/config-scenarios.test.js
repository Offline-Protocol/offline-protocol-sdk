#!/usr/bin/env node
'use strict';

/**
 * Unit tests for config scenario presets (`configScenarios.ts`).
 */

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');

const PACKAGE_DIR = path.resolve(__dirname, '..');

function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run npm ci in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-scenario-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

const outDir = compileSdk();
const {
  forScenario,
  scenarioPreset,
  CONFIG_SCENARIO_IDS,
} = require(path.join(outDir, 'configScenarios.js'));

assert.deepEqual(CONFIG_SCENARIO_IDS.length, 5);

const local = forScenario('localBle', { appId: 'a', profile: 'p' });
assert.equal(local.appId, 'a');
assert.equal(local.transports?.ble?.enabled, true);
assert.equal(local.transports?.internet?.enabled, false);
assert.equal(local.dors, undefined);

const relay = forScenario('internetRelay', {
  appId: 'a',
  profile: 'p',
  overrides: {
    transports: {
      internet: { serverAddress: 'wss://x/ws' },
    },
  },
});
assert.equal(relay.transports?.internet?.enabled, true);
assert.equal(relay.transports?.internet?.serverAddress, 'wss://x/ws');
assert.equal(relay.dors?.preferOnline, true);

const dev = scenarioPreset('developmentCleartext');
assert.equal(dev.encryption?.enabled, false);

const proximity = forScenario('proximityMesh', { appId: 'a', profile: 'p' });
assert.equal(proximity.transports?.wifiDirect?.enabled, true);

console.log('7 passed.');
