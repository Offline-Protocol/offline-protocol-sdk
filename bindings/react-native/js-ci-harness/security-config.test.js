#!/usr/bin/env node
/**
 * Behavioral tests for the JS-layer marshalling of the security configuration
 * (`src/index.ts`).
 *
 * `controlFreshnessEnforced` is the lever an app reaches for mid-incident: the
 * control-frame freshness check is judged against the device's own clock, so a
 * fleet whose clocks are wrong refuses every honest peer and takes its own
 * control plane down until enforcement is switched off. A lever is only a
 * lever if the value an app sets actually arrives, which is what this file
 * pins.
 *
 * Two failure shapes, and the assertions here check for both:
 *
 *  - **A value the bridge drops.** `types.ts` documents a top-level
 *    `controlFreshnessEnforced` alongside the nested `security` home, because
 *    a field reached for in an incident lands one level too high often enough
 *    to be worth honouring. The native parsers honour both spellings; if this
 *    layer forwards only one, the other is documented and dead, and an app
 *    turning enforcement off during an outage sees nothing happen.
 *  - **A value the bridge invents.** A section materialised with a literal
 *    makes the Rust default unreachable, and nothing reports it. That is why
 *    absence is asserted as hard as presence.
 *
 * The sibling Rust guard `every_bridge_reads_the_security_config_section` pins
 * that both native parsers read the section this file proves JS sends. Both
 * halves are required: a payload nobody parses and a parser nobody feeds are
 * the same bug from opposite ends.
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
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-security-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

let nativeCalls = [];

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      return (...args) => {
        nativeCalls.push({ method, args });
        return Promise.resolve();
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

let OfflineProtocol;

const newSdk = (config = {}) =>
  new OfflineProtocol({ appId: 'harness', profile: 'harness-profile', ...config });

function payloadOf(method) {
  const matches = nativeCalls.filter((c) => c.method === method);
  assert.equal(matches.length, 1, `expected exactly one ${method} call, saw ${matches.length}`);
  return JSON.parse(matches[0].args[0]);
}

// ---------------------------------------------------------------------------
// The create-time security section
// ---------------------------------------------------------------------------

test('the nested security section reaches native', async () => {
  const sdk = newSdk({ security: { controlFreshnessEnforced: false } });
  await sdk.start();

  assert.deepEqual(payloadOf('create').security, { controlFreshnessEnforced: false });
});

test('an unset security section is absent from the create payload', async () => {
  // Absence is the assertion. A bridge that sent `{ controlFreshnessEnforced:
  // true }` here would restate the Rust default, and a default restated in
  // four languages is a default that can never be changed again.
  const sdk = newSdk({});
  await sdk.start();

  assert.equal(
    'security' in payloadOf('create'),
    false,
    'an unconfigured security section must not be materialised by the bridge'
  );
});

test('enforcement left on explicitly still crosses the bridge', async () => {
  // Explicitly on is not the same as unset: an app that pins the value must
  // keep it if the core default ever moves.
  const sdk = newSdk({ security: { controlFreshnessEnforced: true } });
  await sdk.start();

  assert.deepEqual(payloadOf('create').security, { controlFreshnessEnforced: true });
});

test('the top-level spelling reaches native too', async () => {
  // The documented escape hatch, and the reason it exists: this is the switch
  // an app flips while its control plane is down, and it is written one level
  // too high often enough that the native parsers accept both spellings. They
  // never see it if this layer drops it first.
  const sdk = newSdk({ controlFreshnessEnforced: false });
  await sdk.start();

  assert.deepEqual(
    payloadOf('create').security,
    { controlFreshnessEnforced: false },
    'a top-level controlFreshnessEnforced must be normalised into the section'
  );
});

test('the nested spelling wins when an app sets both', async () => {
  const sdk = newSdk({
    security: { controlFreshnessEnforced: false },
    controlFreshnessEnforced: true,
  });
  await sdk.start();

  assert.deepEqual(payloadOf('create').security, { controlFreshnessEnforced: false });
});

test('an empty security section stays absent rather than crossing empty', async () => {
  // `sanitize` drops undefined entries and then the whole object, so an app
  // that passes `security: {}` must be indistinguishable from one that passed
  // nothing. A materialised `{}` would be harmless today and a place for a
  // literal to be added tomorrow.
  const sdk = newSdk({ security: {} });
  await sdk.start();

  assert.equal('security' in payloadOf('create'), false);
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
