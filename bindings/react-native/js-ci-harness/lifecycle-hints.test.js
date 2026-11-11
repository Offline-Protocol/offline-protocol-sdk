#!/usr/bin/env node
'use strict';

/**
 * Unit tests for lifecycle ordering helpers (no native runtime).
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-life-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' },
  );
  return outDir;
}

const outDir = compileSdk();
const {
  devLifecycleWarningsEnabled,
  lifecycleNeedsEngineMessage,
  lifecycleNoListenersMessage,
  warnIfCallBeforeEngineStart,
  warnIfNoListenersAtStart,
  bindDevLifecycleWarningsFromConfig,
  setModuleEngineStartedForHints,
  isModuleEngineStarted,
} = require(path.join(outDir, 'lifecycleHints.js'));

assert.equal(devLifecycleWarningsEnabled(false), false);
assert.equal(devLifecycleWarningsEnabled(true), true);

const engineMsg = lifecycleNeedsEngineMessage('sendMessage');
assert.match(engineMsg, /sendMessage/);
assert.match(engineMsg, /start\(\)/);

const listenerMsg = lifecycleNoListenersMessage();
assert.match(listenerMsg, /no on/);

let warned = [];
const prev = console.warn;
console.warn = (msg) => warned.push(msg);

bindDevLifecycleWarningsFromConfig(true);
warnIfCallBeforeEngineStart('sendMessage', false, true);
assert.equal(warned.length, 1);
warnIfCallBeforeEngineStart('sendMessage', false, true);
assert.equal(warned.length, 1, 'dedupes per context');

warned = [];
warnIfCallBeforeEngineStart('sendMessage', true, true);
assert.equal(warned.length, 0);

warned = [];
warnIfNoListenersAtStart(0, true);
assert.equal(warned.length, 1);
warnIfNoListenersAtStart(1, true);
assert.equal(warned.length, 1);

console.warn = prev;

setModuleEngineStartedForHints(true);
assert.equal(isModuleEngineStarted(), true);
setModuleEngineStartedForHints(false);
assert.equal(isModuleEngineStarted(), false);

console.log('9 passed.');
