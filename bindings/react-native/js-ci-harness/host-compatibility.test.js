#!/usr/bin/env node
'use strict';

/**
 * The Expo Go line on the "not linked" error (`src/hostCompatibility.ts`).
 *
 * Pins three things:
 *
 *   1. Expo Go is recognised from either signal expo-constants exposes:
 *      `executionEnvironment === 'storeClient'` (current SDKs) and
 *      `appOwnership === 'expo'` (older ones, deprecated since).
 *   2. A bare app, where `require('expo-constants')` throws, gets the base
 *      message unchanged rather than a crash inside the error path.
 *   3. A development or native build is not told it is in Expo Go.
 */

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const Module = require('node:module');
const os = require('node:os');
const path = require('node:path');

const PACKAGE_DIR = path.resolve(__dirname, '..');

/** Compiles the one source file into a scratch directory (see README). */
function compileHostCompatibility() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc}; run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-harness-'));
  process.on('exit', () => fs.rmSync(outDir, { recursive: true, force: true }));
  execFileSync(
    process.execPath,
    [
      tsc,
      path.join(PACKAGE_DIR, 'src', 'hostCompatibility.ts'),
      '--outDir', outDir,
      '--module', 'commonjs',
      '--target', 'ES2020',
      '--strict',
      '--skipLibCheck',
      '--types', 'node',
    ],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return path.join(outDir, 'hostCompatibility.js');
}

const compiled = compileHostCompatibility();

/** What `require('expo-constants')` returns; `undefined` makes it throw. */
let expoConstants;
const realLoad = Module._load;
Module._load = function load(request, ...rest) {
  if (request === 'expo-constants') {
    if (expoConstants === undefined) {
      const err = new Error("Cannot find module 'expo-constants'");
      err.code = 'MODULE_NOT_FOUND';
      throw err;
    }
    return expoConstants;
  }
  return realLoad.call(this, request, ...rest);
};

const { formatNativeLinkingError, isRunningInExpoGo, HOST_DOCTOR_URL } = require(compiled);
const BASE = 'not linked\n';

let passed = 0;
function test(name, fn) {
  fn();
  passed += 1;
  console.log(`  ✓ ${name}`);
}

test('a bare app (no expo-constants) gets the base message unchanged', () => {
  expoConstants = undefined;
  assert.equal(isRunningInExpoGo(), false);
  assert.equal(formatNativeLinkingError(BASE), BASE);
});

test("executionEnvironment 'storeClient' is Expo Go", () => {
  expoConstants = { default: { executionEnvironment: 'storeClient', appOwnership: null } };
  assert.equal(isRunningInExpoGo(), true);
  const message = formatNativeLinkingError(BASE);
  assert.ok(message.startsWith(BASE));
  assert.match(message, /Detected Expo Go/);
  assert.ok(message.includes(HOST_DOCTOR_URL), 'the hint carries a URL, not a repo path');
});

test("the deprecated appOwnership 'expo' is Expo Go, with or without a default export", () => {
  expoConstants = { appOwnership: 'expo' };
  assert.equal(isRunningInExpoGo(), true);
  expoConstants = { default: { appOwnership: 'expo' } };
  assert.equal(isRunningInExpoGo(), true);
});

test('a development or native build is not Expo Go', () => {
  for (const executionEnvironment of ['bare', 'standalone']) {
    expoConstants = { default: { executionEnvironment, appOwnership: null } };
    assert.equal(isRunningInExpoGo(), false, executionEnvironment);
    assert.equal(formatNativeLinkingError(BASE), BASE);
  }
});

console.log(`\n${passed} passed.`);
