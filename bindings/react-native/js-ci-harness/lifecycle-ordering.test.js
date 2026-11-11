#!/usr/bin/env node
'use strict';

/**
 * Behavioral tests for dev lifecycle ordering warnings on OfflineProtocol.
 */

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const Module = require('node:module');
const os = require('node:os');
const path = require('node:path');

const PACKAGE_DIR = path.resolve(__dirname, '..');
const VALID = 'off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn';

function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run npm ci in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-life-order-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' },
  );
  return outDir;
}

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      if (method === 'isMlsInitialized') {
        return () => Promise.resolve(true);
      }
      return (..._args) => Promise.resolve(method === 'send' ? 'msg-1' : undefined);
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

const outDir = compileSdk();
const { OfflineProtocol, DataStore } = require(path.join(outDir, 'index.js'));

const realConsole = { warn: console.warn };
let captured = [];

function captureConsole() {
  captured = [];
  console.warn = (...args) => captured.push(args.join(' '));
}

function releaseConsole() {
  console.warn = realConsole.warn;
}

async function main() {
  captureConsole();
  const sdk = new OfflineProtocol({
    appId: 'harness',
    profile: 'harness-profile',
    devLifecycleWarnings: true,
    encryption: { enabled: false },
  });
  await sdk.sendMessage({ recipient: VALID, content: 'hi' });
  assert.ok(
    captured.some((w) => w.includes('sendMessage') && w.includes('start()')),
    'send before start warns'
  );

  captured = [];
  const store = new DataStore();
  await store.mapSet('s', 'd', 'c', 'k', { kind: 'text', value: 'x' });
  assert.ok(
    captured.some((w) => w.includes('mapSet')),
    'data mutation before start warns'
  );

  captured = [];
  await sdk.start();
  assert.ok(
    captured.some((w) => w.includes('no on')),
    'start without listeners warns'
  );

  releaseConsole();
  console.log('3 passed.');
}

main().catch((err) => {
  releaseConsole();
  throw err;
});
