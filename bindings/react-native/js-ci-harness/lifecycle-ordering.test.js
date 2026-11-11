#!/usr/bin/env node
'use strict';

/**
 * Behavioral tests for dev lifecycle ordering warnings on OfflineProtocol,
 * DataStore and MeshServices, over a stubbed native module.
 *
 * The first scenario is the one that matters most: a correctly ordered app,
 * with a transport auto-enabled by start() and a listener that sends while
 * start() is still running, sees no lifecycle hint at all. start() once set
 * its started flag after the transport auto-enables, so it warned
 * "call start() first" about its own enableTransport call on every launch.
 *
 * The hints dedupe per key for the whole process, so each scenario uses
 * calls no earlier scenario has warned about.
 */

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const Module = require('node:module');
const os = require('node:os');
const path = require('node:path');

const PACKAGE_DIR = path.resolve(__dirname, '..');
const VALID = 'off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn';

/** Matches every lifecycle hint, and nothing else start() logs. */
const LIFECYCLE_HINT =
  /call await protocol\.start\(\) before this API|start\(\) with no on\(\.\.\.\) listeners|the data store is not open yet|narrowed the interest|encryption is enabled but MLS is not initialized/;

function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-life-order-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

const stub = {
  mlsInitialized: true,
  /** Called from the native enableTransport, as a listener reacting mid-start would. */
  onEnableTransport: null,
};

const nativeModule = new Proxy(
  {},
  {
    get(_target, method) {
      if (typeof method !== 'string') return undefined;
      if (method === 'isMlsInitialized') {
        return () => Promise.resolve(stub.mlsInitialized);
      }
      if (method === 'enableTransport') {
        return async () => {
          if (stub.onEnableTransport) await stub.onEnableTransport();
          return undefined;
        };
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
const { OfflineProtocol, DataStore, MeshServices } = require(path.join(outDir, 'index.js'));

const realWarn = console.warn;
const realLog = console.log;
let captured = [];

function hints() {
  return captured.filter((w) => LIFECYCLE_HINT.test(w));
}

function capture() {
  captured = [];
  console.warn = (...args) => captured.push(args.map(String).join(' '));
  console.log = () => {};
}

function release() {
  console.warn = realWarn;
  console.log = realLog;
}

function config(extra = {}) {
  return {
    appId: 'harness',
    profile: 'harness-profile',
    devLifecycleWarnings: true,
    ...extra,
  };
}

let passed = 0;
async function test(name, fn) {
  capture();
  try {
    await fn();
    passed += 1;
  } catch (error) {
    release();
    console.error(`FAIL: ${name}`);
    throw error;
  } finally {
    release();
  }
}

async function main() {
  const store = new DataStore();
  const services = new MeshServices();

  await test('a correctly ordered app sees no lifecycle hint', async () => {
    const sdk = new OfflineProtocol(
      config({
        transports: { internet: { enabled: true, serverAddress: 'wss://relay.invalid' } },
      })
    );
    sdk.on('all', () => {});
    await store.setInterest('ordered', ['notes/*']);
    // A listener that reacts to discovery while start() is still awaiting
    // its transport auto-enables.
    stub.onEnableTransport = () => sdk.establishSecureSession(VALID);
    try {
      await sdk.start();
    } finally {
      stub.onEnableTransport = null;
    }
    await sdk.sendMessage({ recipient: VALID, content: 'hi' });
    await sdk.enableTransport('nostr');
    await store.mapSet('ordered', 'd', 'c', 'k', { kind: 'text', value: 'x' });
    // Widening after start is the supported post-start use.
    await store.setInterest('ordered', ['*']);
    await services.registerService('svc', '1.0.0');
    assert.deepEqual(hints(), []);
    await sdk.destroy();
  });

  const sdk = new OfflineProtocol(config());

  await test('calls before start() warn', async () => {
    await sdk.sendMessage({ recipient: VALID, content: 'hi' });
    await store.mapSet('s', 'd', 'c', 'k', { kind: 'text', value: 'x' });
    await services.sendServiceRequest(VALID, 'svc', 'body');
    const got = hints();
    assert.ok(got.some((w) => w.includes('sendMessage') && w.includes('start()')), got.join('\n'));
    assert.ok(got.some((w) => w.includes('mapSet')), got.join('\n'));
    assert.ok(got.some((w) => w.includes('sendServiceRequest')), got.join('\n'));
  });

  await test('setInterest before start() is held and does not warn', async () => {
    await store.setInterest('s', ['*']);
    assert.deepEqual(hints(), []);
  });

  await test('start() without listeners warns', async () => {
    await sdk.start();
    assert.ok(hints().some((w) => w.includes('no on(...) listeners')));
  });

  await test('after start(), widening is silent and narrowing warns', async () => {
    await store.setInterest('s', ['*']);
    assert.deepEqual(hints(), []);
    await store.setInterest('s', ['notes/*']);
    await store.setInterest('s', ['notes/a']);
    const got = hints();
    assert.equal(got.length, 1, got.join('\n'));
    assert.match(got[0], /setInterest\("s"\)/);
  });

  await test('after stop(), engine calls warn again', async () => {
    await sdk.stop();
    await sdk.sendConnectionRequest({ recipient: VALID, senderName: 'me' });
    assert.ok(hints().some((w) => w.includes('sendConnectionRequest')));
  });

  await test('an explicit false silences every hint in a dev build', async () => {
    await sdk.destroy();
    globalThis.__DEV__ = true;
    try {
      const quiet = new OfflineProtocol(config({ devLifecycleWarnings: false }));
      await quiet.disableTransport('ble');
      await store.textInsert('q', 'd', 'c', 0, 'x');
      await quiet.start();
      assert.deepEqual(hints(), []);
      await quiet.destroy();
    } finally {
      delete globalThis.__DEV__;
    }
  });

  await test('MLS hint follows readiness, including a later successful init', async () => {
    stub.mlsInitialized = false;
    const secure = new OfflineProtocol(config());
    secure.on('all', () => {});
    await secure.start();
    await secure.sendMessage({ recipient: VALID, content: 'hi' });
    assert.ok(hints().some((w) => w.includes('sendMessage') && /MLS/.test(w)));

    stub.mlsInitialized = true;
    captured = [];
    await secure.initializeMlsWithSecureStorage();
    await secure.forwardMessage({ originalMessageJson: '{}', newRecipient: VALID });
    assert.deepEqual(hints(), []);
    await secure.destroy();
  });

  console.log(`${passed} passed.`);
}

main().catch((err) => {
  release();
  console.error(err);
  process.exit(1);
});
