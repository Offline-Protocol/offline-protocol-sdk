#!/usr/bin/env node
'use strict';

/**
 * Unit tests for the lifecycle ordering helpers in `src/lifecycleHints.ts`
 * (no native runtime). lifecycle-ordering.test.js covers the call sites.
 *
 * Pins the flag resolution (explicit value beats `__DEV__`), per-key dedupe,
 * the interest narrowing rule, and that the helpers stay internal: none of
 * them is re-exported from the package entry point.
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
    throw new Error(`TypeScript not found at ${tsc} — run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-life-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

const outDir = compileSdk();
const hints = require(path.join(outDir, 'lifecycleHints.js'));
const {
  devLifecycleWarningsEnabled,
  lifecycleNeedsEngineMessage,
  lifecycleNoListenersMessage,
  isInterestNarrowing,
  warnIfCallBeforeEngineStart,
  warnIfNoListenersAtStart,
  warnIfInterestNarrowedAfterStart,
  warnIfSendWithoutMls,
  bindDevLifecycleWarningsFromConfig,
  setModuleEngineStartedForHints,
  isModuleEngineStarted,
} = hints;

let passed = 0;
function test(name, fn) {
  try {
    fn();
    passed += 1;
  } catch (error) {
    console.error(`FAIL: ${name}`);
    throw error;
  }
}

function captureWarnings(fn) {
  const warned = [];
  const prev = console.warn;
  console.warn = (...args) => warned.push(args.join(' '));
  try {
    fn();
  } finally {
    console.warn = prev;
  }
  return warned;
}

function withDev(value, fn) {
  const had = Object.prototype.hasOwnProperty.call(globalThis, '__DEV__');
  const prev = globalThis.__DEV__;
  if (value === undefined) {
    delete globalThis.__DEV__;
  } else {
    globalThis.__DEV__ = value;
  }
  try {
    fn();
  } finally {
    if (had) {
      globalThis.__DEV__ = prev;
    } else {
      delete globalThis.__DEV__;
    }
  }
}

test('an explicit flag beats __DEV__ in both directions', () => {
  withDev(true, () => {
    assert.equal(devLifecycleWarningsEnabled(false), false);
    assert.equal(devLifecycleWarningsEnabled(true), true);
  });
  withDev(false, () => assert.equal(devLifecycleWarningsEnabled(true), true));
});

test('an unset flag follows __DEV__, and is off when __DEV__ is absent', () => {
  withDev(true, () => assert.equal(devLifecycleWarningsEnabled(undefined), true));
  withDev(false, () => assert.equal(devLifecycleWarningsEnabled(undefined), false));
  withDev(undefined, () => assert.equal(devLifecycleWarningsEnabled(undefined), false));
});

test('messages name the call and the fix', () => {
  const engineMsg = lifecycleNeedsEngineMessage('sendMessage');
  assert.match(engineMsg, /sendMessage/);
  assert.match(engineMsg, /start\(\)/);
  const listenerMsg = lifecycleNoListenersMessage();
  assert.match(listenerMsg, /no on\(\.\.\.\) listeners/);
  // Inbound messages are held and replayed, so the hint must not claim
  // they are lost.
  assert.doesNotMatch(listenerMsg, /message_received/);
});

test('the engine hint fires once per context and never once started', () => {
  bindDevLifecycleWarningsFromConfig(true);
  const warned = captureWarnings(() => {
    warnIfCallBeforeEngineStart('unit:a', false);
    warnIfCallBeforeEngineStart('unit:a', false);
    warnIfCallBeforeEngineStart('unit:b', true);
  });
  assert.equal(warned.length, 1);
  assert.match(warned[0], /unit:a/);
});

test('an explicit false silences the hint even when the module flag is on', () => {
  bindDevLifecycleWarningsFromConfig(true);
  const warned = captureWarnings(() => {
    warnIfCallBeforeEngineStart('unit:off', false, false);
    warnIfNoListenersAtStart(0, false);
  });
  assert.equal(warned.length, 0);
});

test('the no-listeners hint fires only with zero listeners', () => {
  const warned = captureWarnings(() => {
    warnIfNoListenersAtStart(1, true);
    warnIfNoListenersAtStart(0, true);
    warnIfNoListenersAtStart(0, true);
  });
  assert.equal(warned.length, 1);
});

test('the MLS hint needs a started engine with encryption on and MLS not ready', () => {
  const warned = captureWarnings(() => {
    warnIfSendWithoutMls('unit:mls', true, false, false, true); // not started
    warnIfSendWithoutMls('unit:mls', false, false, true, true); // cleartext
    warnIfSendWithoutMls('unit:mls', true, true, true, true); // ready
    warnIfSendWithoutMls('unit:mls', true, false, true, true); // the one
  });
  assert.equal(warned.length, 1);
});

test('interest narrowing: widening and equal sets are not narrowings', () => {
  assert.equal(isInterestNarrowing(['*'], ['*']), false);
  assert.equal(isInterestNarrowing(['notes/*'], ['*']), false);
  assert.equal(isInterestNarrowing(['notes/a'], ['notes/*']), false);
  assert.equal(isInterestNarrowing(['notes/*'], ['notes/*', 'todo']), false);
  assert.equal(isInterestNarrowing([], ['notes/*']), false);
  assert.equal(isInterestNarrowing([], []), false);
});

test('interest narrowing: dropping or tightening a pattern is a narrowing', () => {
  assert.equal(isInterestNarrowing(['*'], ['notes/*']), true);
  assert.equal(isInterestNarrowing(['*'], []), true);
  assert.equal(isInterestNarrowing(['notes/*'], ['notes/a']), true);
  assert.equal(isInterestNarrowing(['notes/*', 'todo'], ['notes/*']), true);
  assert.equal(isInterestNarrowing(['notes/a'], ['notes/b']), true);
});

test('the interest hint fires for a narrowing only, once per space', () => {
  const warned = captureWarnings(() => {
    warnIfInterestNarrowedAfterStart('unit-space', ['notes/*'], ['*'], true);
    warnIfInterestNarrowedAfterStart('unit-space', ['*'], ['notes/*'], true);
    warnIfInterestNarrowedAfterStart('unit-space', ['*'], ['todo'], true);
    warnIfInterestNarrowedAfterStart('unit-space', ['*'], ['todo'], false);
  });
  assert.equal(warned.length, 1);
  assert.match(warned[0], /unit-space/);
});

test('module engine state round-trips', () => {
  setModuleEngineStartedForHints(true);
  assert.equal(isModuleEngineStarted(), true);
  setModuleEngineStartedForHints(false);
  assert.equal(isModuleEngineStarted(), false);
});

test('the package entry point re-exports none of the helpers', () => {
  const index = fs.readFileSync(path.join(PACKAGE_DIR, 'src', 'index.ts'), 'utf8');
  assert.doesNotMatch(
    index,
    /export\s*\{[^}]*\}\s*from\s*['"]\.\/lifecycleHints['"]/,
    'lifecycleHints is internal: import from it, never re-export it'
  );
  assert.doesNotMatch(index, /export\s*\*\s*from\s*['"]\.\/lifecycleHints['"]/);
  for (const name of Object.keys(hints)) {
    assert.doesNotMatch(
      index,
      new RegExp(`export\\s+(?:const|function|class)\\s+${name}\\b`),
      `${name} must not be part of the public API`
    );
  }
});

console.log(`${passed} passed.`);
