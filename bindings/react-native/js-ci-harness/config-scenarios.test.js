#!/usr/bin/env node
'use strict';

/**
 * Config scenario presets (`src/configScenarios.ts`).
 *
 * Pins four things:
 *
 *   1. Every `forScenario` example in the presets document and the package
 *      README typechecks against the real `src/`. The examples set a single
 *      nested field such as `transports.internet.serverAddress`, which a
 *      shallow `Partial<ProtocolConfig>` refuses because `enabled` is
 *      required. A plain-JS test cannot see that, so the first version
 *      shipped examples that failed to compile in every TypeScript app.
 *   2. The merge: nested fields merge at every depth, arrays replace,
 *      `undefined` keeps the preset, a parsed `__proto__` is never copied,
 *      and presets are fresh objects.
 *   3. `forScenario` throws on a config that would start without the
 *      transport it enables, a failure `start()` reports nowhere, unless
 *      `connectInternetLater` says the app enables the relay itself.
 *   4. A preset drives the real `OfflineProtocol` to the native payloads the
 *      scenario promises, and `start()` connects exactly what the shared
 *      predicates in `transportAutoStart.ts` accept.
 *
 * See one-shot-hold.test.js for the stubbed-native-module harness pattern.
 */

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const Module = require('node:module');
const os = require('node:os');
const path = require('node:path');

const PACKAGE_DIR = path.resolve(__dirname, '..');
const REPO_DIR = path.resolve(PACKAGE_DIR, '..', '..');
const TSC = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');

/** Documents whose `forScenario` examples must compile. */
const DOCUMENTED_EXAMPLES = [
  path.join(REPO_DIR, 'docs', 'react-native-config-presets.md'),
  path.join(PACKAGE_DIR, 'README.md'),
];

function requireTsc() {
  if (!fs.existsSync(TSC)) {
    throw new Error(`TypeScript not found at ${TSC}: run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
}

function compileSdk() {
  requireTsc();
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-scenario-'));
  execFileSync(
    process.execPath,
    [TSC, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

/** Every ```typescript or ```ts fence in `file` that calls `forScenario`. */
function forScenarioExamples(file) {
  const text = fs.readFileSync(file, 'utf8');
  const blocks = [];
  const fence = /```(?:typescript|ts)\n([\s\S]*?)```/g;
  let match;
  while ((match = fence.exec(text)) !== null) {
    if (match[1].includes('forScenario(')) {
      blocks.push(match[1]);
    }
  }
  return blocks;
}

/**
 * Typechecks each example as its own module, with the package name mapped
 * to `src/index.ts`. Returns tsc's output, or null when it is clean.
 */
function typecheckExamples(blocks) {
  requireTsc();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-scenario-docs-'));
  try {
    const files = blocks.map((block, i) => {
      const file = path.join(dir, `example-${i}.ts`);
      fs.writeFileSync(file, block);
      return file;
    });
    fs.writeFileSync(
      path.join(dir, 'tsconfig.json'),
      JSON.stringify({
        extends: path.join(PACKAGE_DIR, 'tsconfig.json'),
        compilerOptions: {
          noEmit: true,
          rootDir: path.parse(dir).root,
          noUnusedLocals: false,
          typeRoots: [path.join(PACKAGE_DIR, 'node_modules', '@types')],
          baseUrl: dir,
          paths: { '@offline-protocol/mesh-sdk': [path.join(PACKAGE_DIR, 'src', 'index.ts')] },
        },
        include: [],
        files,
      })
    );
    try {
      execFileSync(process.execPath, [TSC, '-p', dir], { encoding: 'utf8' });
      return null;
    } catch (error) {
      return `${error.stdout ?? ''}${error.stderr ?? ''}` || String(error);
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

// ---------------------------------------------------------------------------
// The native stub
// ---------------------------------------------------------------------------

/** Every native call the SDK made, in order: `{ method, args }`. */
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

// ---------------------------------------------------------------------------
// Scaffolding
// ---------------------------------------------------------------------------

const realConsole = { log: console.log, warn: console.warn, error: console.error };
let captured = { warn: [], error: [] };

function captureConsole() {
  captured = { warn: [], error: [] };
  console.log = () => {};
  console.warn = (...args) => captured.warn.push(args.join(' '));
  console.error = (...args) => captured.error.push(args.join(' '));
}

function releaseConsole() {
  Object.assign(console, realConsole);
}

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

let sdk;

const ids = { appId: 'a', profile: 'p' };
const RELAY = { transports: { internet: { serverAddress: 'wss://relay.example/ws' } } };
const NOSTR = { transports: { nostr: { relayUrls: ['wss://n.example'] } } };

// ---------------------------------------------------------------------------
// 1. The documented examples compile
// ---------------------------------------------------------------------------

test('every documented forScenario example typechecks', () => {
  const blocks = DOCUMENTED_EXAMPLES.flatMap(forScenarioExamples);
  assert.ok(blocks.length >= 4, `expected the documented examples, found ${blocks.length}`);
  const errors = typecheckExamples(blocks);
  assert.equal(errors, null, `a documented example does not compile:\n${errors}`);
});

test('the typecheck refuses a wrong example (negative control)', () => {
  const errors = typecheckExamples([
    "import { forScenario } from '@offline-protocol/mesh-sdk';\n" +
      "forScenario('localBle', { appId: 'a', profile: 'p', overrides: { appId: 'b' } });\n",
  ]);
  assert.notEqual(errors, null, 'appId in overrides must not typecheck');
});

// ---------------------------------------------------------------------------
// 2. Presets and the merge
// ---------------------------------------------------------------------------

test('the scenario ids are the five documented ones', () => {
  assert.deepEqual(
    [...sdk.CONFIG_SCENARIO_IDS],
    ['localBle', 'proximityMesh', 'internetRelay', 'nostrMesh', 'developmentCleartext']
  );
});

test('localBle turns on BLE alone and leaves DORS to the core', () => {
  const config = sdk.forScenario('localBle', ids);
  assert.equal(config.appId, 'a');
  assert.equal(config.profile, 'p');
  assert.equal(config.transports.ble.enabled, true);
  for (const key of ['wifiDirect', 'internet', 'nostr', 'reticulum']) {
    assert.equal(config.transports[key].enabled, false, key);
  }
  assert.equal(config.dors, undefined);
  assert.equal(config.encryption, undefined, 'encryption stays on the core default');
});

test('proximityMesh turns on BLE and Wi-Fi Direct only', () => {
  const config = sdk.forScenario('proximityMesh', ids);
  assert.equal(config.transports.ble.enabled, true);
  assert.equal(config.transports.wifiDirect.enabled, true);
  for (const key of ['internet', 'nostr', 'reticulum']) {
    assert.equal(config.transports[key].enabled, false, key);
  }
});

test('a nested override keeps the preset fields beside it', () => {
  const config = sdk.forScenario('internetRelay', { ...ids, overrides: RELAY });
  assert.deepEqual(config.transports.internet, {
    enabled: true,
    serverAddress: 'wss://relay.example/ws',
  });
  assert.equal(config.transports.ble.enabled, true);
  assert.equal(config.dors.preferOnline, true);
});

test('nostrMesh carries the relay URLs and keeps encryption on', () => {
  const config = sdk.forScenario('nostrMesh', { ...ids, overrides: NOSTR });
  assert.equal(config.transports.nostr.enabled, true);
  assert.deepEqual(config.transports.nostr.relayUrls, ['wss://n.example']);
  assert.equal(config.encryption, undefined);
});

test('an override can turn a preset transport off', () => {
  const config = sdk.forScenario('proximityMesh', {
    ...ids,
    overrides: { transports: { wifiDirect: { enabled: false } } },
  });
  assert.equal(config.transports.wifiDirect.enabled, false);
  assert.equal(config.transports.ble.enabled, true);
});

test('merging reaches below the second level', () => {
  const config = sdk.forScenario('localBle', {
    ...ids,
    overrides: { encryption: { pendingQueue: { pendingTtlMs: 1000 } } },
  });
  assert.deepEqual(config.encryption, { pendingQueue: { pendingTtlMs: 1000 } });
});

test('arrays replace, never merge element by element', () => {
  const config = sdk.forScenario('nostrMesh', {
    ...ids,
    overrides: {
      transports: { nostr: { relayUrls: ['wss://only.example'] } },
    },
  });
  assert.deepEqual(config.transports.nostr.relayUrls, ['wss://only.example']);
});

test('an undefined override keeps the preset value', () => {
  const config = sdk.forScenario('internetRelay', {
    ...ids,
    overrides: {
      transports: { internet: { serverAddress: 'wss://r/ws', enabled: undefined } },
      dors: undefined,
    },
  });
  assert.equal(config.transports.internet.enabled, true);
  assert.equal(config.dors.preferOnline, true);
});

test('a parsed __proto__ key is never copied', () => {
  const overrides = JSON.parse(
    '{"__proto__":{"polluted":1},"transports":{"__proto__":{"polluted":1}}}'
  );
  const config = sdk.forScenario('localBle', { ...ids, overrides });
  assert.equal(Object.getPrototypeOf(config), Object.prototype);
  assert.equal(Object.getPrototypeOf(config.transports), Object.prototype);
  assert.equal(config.polluted, undefined);
  assert.equal(config.transports.polluted, undefined);
  assert.equal({}.polluted, undefined);
});

test('neither the preset nor the overrides are modified', () => {
  const overrides = { transports: { internet: { serverAddress: 'wss://r/ws' } } };
  const snapshot = JSON.stringify(overrides);
  const config = sdk.forScenario('internetRelay', { ...ids, overrides });
  config.transports.internet.serverAddress = 'changed';
  config.transports.ble.enabled = false;
  assert.equal(JSON.stringify(overrides), snapshot);
  assert.equal(sdk.scenarioPreset('internetRelay').transports.ble.enabled, true);
  assert.equal(sdk.forScenario('internetRelay', { ...ids, overrides }).transports.ble.enabled, true);
});

// ---------------------------------------------------------------------------
// 3. What forScenario refuses
// ---------------------------------------------------------------------------

test('internetRelay without a server throws', () => {
  assert.throws(() => sdk.forScenario('internetRelay', ids), /serverAddress/);
});

test('nostrMesh without relays throws', () => {
  assert.throws(() => sdk.forScenario('nostrMesh', ids), /relayUrls/);
  assert.throws(
    () =>
      sdk.forScenario('nostrMesh', {
        ...ids,
        overrides: { transports: { nostr: { relayUrls: [] } } },
      }),
    /relayUrls/
  );
});

test('Nostr with encryption off throws', () => {
  assert.throws(
    () => sdk.forScenario('developmentCleartext', { ...ids, overrides: {
      transports: { nostr: { enabled: true, relayUrls: ['wss://n.example'] } },
    } }),
    /encryption/
  );
});

test('enabling internet by override without a server throws too', () => {
  assert.throws(
    () =>
      sdk.forScenario('localBle', {
        ...ids,
        overrides: { transports: { internet: { enabled: true } } },
      }),
    /serverAddress/
  );
});

test('a transport entry with no enabled throws', () => {
  assert.throws(
    () =>
      sdk.forScenario('internetRelay', {
        ...ids,
        overrides: {
          transports: {
            internet: { serverAddress: 'wss://r/ws' },
            reticulum: { daemonAddress: 'localhost:4242' },
          },
        },
      }),
    /transports\.reticulum has no 'enabled'/
  );
});

test('a null transport entry throws the named error, not a TypeError', () => {
  const overrides = JSON.parse('{"transports":{"internet":null}}');
  assert.throws(
    () => sdk.forScenario('localBle', { ...ids, overrides }),
    /transports\.internet has no 'enabled'/
  );
});

test('internetRelay without a server is accepted with connectInternetLater', () => {
  const config = sdk.forScenario('internetRelay', { ...ids, connectInternetLater: true });
  assert.deepEqual(config.transports.internet, { enabled: true });
  assert.equal('connectInternetLater' in config, false, 'the flag is a parameter, not config');
});

test('the missing-server error names both ways out', () => {
  assert.throws(() => sdk.forScenario('internetRelay', ids), /connectInternetLater/);
});

test('connectInternetLater with a server throws', () => {
  assert.throws(
    () => sdk.forScenario('internetRelay', { ...ids, overrides: RELAY, connectInternetLater: true }),
    /contradict/
  );
});

test('connectInternetLater with internet off throws', () => {
  assert.throws(
    () => sdk.forScenario('localBle', { ...ids, connectInternetLater: true }),
    /needs the internet transport enabled/
  );
  assert.throws(
    () => sdk.forScenario('nostrMesh', { ...ids, overrides: NOSTR, connectInternetLater: true }),
    /needs the internet transport enabled/
  );
});

test('appId or profile inside overrides throws instead of being dropped', () => {
  assert.throws(
    () => sdk.forScenario('localBle', { ...ids, overrides: { appId: 'other' } }),
    /pass appId as a parameter/
  );
  assert.throws(
    () => sdk.forScenario('localBle', { ...ids, overrides: { profile: 'other' } }),
    /pass profile as a parameter/
  );
});

test('a cleartext config warns; an encrypted one does not', () => {
  const config = sdk.forScenario('developmentCleartext', ids);
  assert.equal(config.encryption.enabled, false);
  assert.equal(captured.warn.length, 1);
  assert.match(captured.warn[0], /encryption OFF/);

  sdk.forScenario('localBle', { ...ids, overrides: { encryption: { enabled: false } } });
  assert.equal(captured.warn.length, 2, 'an override that disables encryption warns too');

  sdk.forScenario('localBle', ids);
  assert.equal(captured.warn.length, 2);
});

// ---------------------------------------------------------------------------
// 4. A preset through the real engine wrapper
// ---------------------------------------------------------------------------

/** The JSON payload of the single call to `method`, parsed. */
function payloadOf(method) {
  const matches = nativeCalls.filter((c) => c.method === method);
  assert.equal(matches.length, 1, `expected exactly one ${method} call, saw ${matches.length}`);
  return JSON.parse(matches[0].args[0]);
}

test('internetRelay reaches native and connects the relay at start()', async () => {
  const protocol = new sdk.OfflineProtocol(
    sdk.forScenario('internetRelay', { ...ids, overrides: RELAY })
  );
  await protocol.start();

  const create = payloadOf('create');
  assert.equal(create.bleEnabled, true);
  assert.equal(create.internetEnabled, true);
  assert.equal(create.preferOnline, true);
  assert.equal(create.encryptionEnabled, true, 'encryption stays on');
  assert.equal(create.requireEncryption, true, 'and fail-closed');

  const enables = nativeCalls.filter((c) => c.method === 'enableTransport');
  assert.deepEqual(
    enables.map((c) => [c.args[0], c.args[1].serverAddress]),
    [['internet', 'wss://relay.example/ws']]
  );
});

test('developmentCleartext reaches native as the fully disabled posture', async () => {
  const protocol = new sdk.OfflineProtocol(sdk.forScenario('developmentCleartext', ids));
  await protocol.start();

  const create = payloadOf('create');
  assert.equal(create.encryptionEnabled, false);
  assert.equal(create.requireEncryption, false, 'else the core refuses the config');
  assert.equal(
    nativeCalls.some((c) => c.method === 'initializeMlsWithSecureStorage'),
    false
  );
});

test('connectInternetLater registers internet at create and leaves the connect to the app', async () => {
  const protocol = new sdk.OfflineProtocol(
    sdk.forScenario('internetRelay', { ...ids, connectInternetLater: true })
  );
  await protocol.start();

  assert.equal(payloadOf('create').internetEnabled, true);
  assert.equal(
    nativeCalls.some((c) => c.method === 'enableTransport' && c.args[0] === 'internet'),
    false,
    'start() must not connect a relay it has no server for'
  );
});

test('start() connects exactly the transports the shared predicates accept', async () => {
  // forScenario validates against these predicates; this pins start() to them,
  // so the two cannot drift apart.
  const cases = [
    { internet: { enabled: true, serverAddress: 'wss://r/ws' } },
    { internet: { enabled: true } },
    { internet: { enabled: true, serverAddress: '' } },
    { internet: { enabled: false, serverAddress: 'wss://r/ws' } },
    { nostr: { enabled: true, relayUrls: ['wss://n'] } },
    { nostr: { enabled: true, relayUrls: [] } },
    { nostr: { enabled: true } },
    { nostr: { enabled: false, relayUrls: ['wss://n'] } },
  ];
  for (const transports of cases) {
    nativeCalls = [];
    const protocol = new sdk.OfflineProtocol({ ...ids, transports: { ble: { enabled: true }, ...transports } });
    await protocol.start();
    const enabled = new Set(
      nativeCalls.filter((c) => c.method === 'enableTransport').map((c) => c.args[0])
    );
    const label = JSON.stringify(transports);
    assert.equal(
      enabled.has('internet'),
      sdk.internetAutoConnects(transports.internet),
      `internet: ${label}`
    );
    assert.equal(enabled.has('nostr'), sdk.nostrAutoConnects(transports.nostr), `nostr: ${label}`);
  }
});

(async () => {
  const outDir = compileSdk();
  try {
    sdk = {
      ...require(path.join(outDir, 'configScenarios.js')),
      ...require(path.join(outDir, 'transportAutoStart.js')),
      OfflineProtocol: require(path.join(outDir, 'index.js')).OfflineProtocol,
    };

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
