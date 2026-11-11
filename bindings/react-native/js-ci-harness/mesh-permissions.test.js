#!/usr/bin/env node
'use strict';

/**
 * Unit tests for the mesh permission helpers (no native runtime):
 * `buildAndroidMeshPermissionList`, `meshPermissionOptionsFromConfig` and
 * `requestMeshPermissions` / `requestMeshPermissionsForConfig` against a
 * stubbed `PermissionsAndroid`.
 */

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const Module = require('node:module');
const path = require('node:path');
const os = require('node:os');

const PACKAGE_DIR = path.resolve(__dirname, '..');

const P = {
  BLUETOOTH_SCAN: 'android.permission.BLUETOOTH_SCAN',
  BLUETOOTH_CONNECT: 'android.permission.BLUETOOTH_CONNECT',
  BLUETOOTH_ADVERTISE: 'android.permission.BLUETOOTH_ADVERTISE',
  ACCESS_FINE_LOCATION: 'android.permission.ACCESS_FINE_LOCATION',
  NEARBY_WIFI_DEVICES: 'android.permission.NEARBY_WIFI_DEVICES',
  ACCESS_LOCAL_NETWORK: 'android.permission.ACCESS_LOCAL_NETWORK',
};

// React Native before 0.87 has no PermissionsAndroid.PERMISSIONS.ACCESS_LOCAL_NETWORK
// (the package supports 0.70+), so the stub leaves it out: the helper must use
// the literal permission string, not the constant.
const { ACCESS_LOCAL_NETWORK: _absentBefore087, ...STUB_PERMISSIONS } = P;

// One shared stub so tests can change the device API level and the user's
// answers between calls.
const stub = {
  Platform: { OS: 'android', Version: 33 },
  PermissionsAndroid: {
    PERMISSIONS: Object.freeze(STUB_PERMISSIONS),
    RESULTS: {
      GRANTED: 'granted',
      DENIED: 'denied',
      NEVER_ASK_AGAIN: 'never_ask_again',
    },
    check: async () => true,
    requestMultiple: async () => ({}),
  },
};

const realLoad = Module._load;
Module._load = function loadWithReactNativeStub(request) {
  if (request === 'react-native') {
    return stub;
  }
  return realLoad.apply(this, arguments);
};

/**
 * Points the stub at an Android device with nothing granted yet, answering
 * each prompted permission from `answers` (default: granted). Returns the
 * list of permissions the helper prompted for.
 */
function androidDevice(apiLevel, answers = {}) {
  const prompted = [];
  stub.Platform.OS = 'android';
  stub.Platform.Version = apiLevel;
  stub.PermissionsAndroid.check = async () => false;
  stub.PermissionsAndroid.requestMultiple = async (permissions) => {
    prompted.push(...permissions);
    const out = {};
    for (const permission of permissions) {
      out[permission] = answers[permission] ?? 'granted';
    }
    return out;
  };
  return prompted;
}

function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run npm ci in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-perm-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' },
  );
  return outDir;
}

const outDir = compileSdk();
const {
  buildAndroidMeshPermissionList,
  meshPermissionOptionsFromConfig,
  requestMeshPermissions,
  requestMeshPermissionsForConfig,
  ANDROID_API_BLUETOOTH_RUNTIME,
  ANDROID_API_NEARBY_WIFI,
  ANDROID_API_LOCAL_NETWORK,
} = require(path.join(outDir, 'meshPermissions.js'));

const bleOnly31 = buildAndroidMeshPermissionList({ ble: true, wifiDirect: false }, 31);
assert.deepEqual(bleOnly31, [P.BLUETOOTH_SCAN, P.BLUETOOTH_CONNECT, P.BLUETOOTH_ADVERTISE]);

const bleOnly30 = buildAndroidMeshPermissionList({ ble: true }, 30);
assert.deepEqual(bleOnly30, [P.ACCESS_FINE_LOCATION]);

const wifi33 = buildAndroidMeshPermissionList(
  { ble: false, wifiDirect: true },
  ANDROID_API_NEARBY_WIFI,
);
assert.deepEqual(wifi33, [P.NEARBY_WIFI_DEVICES]);

const wifi30 = buildAndroidMeshPermissionList({ ble: false, wifiDirect: true }, 30);
assert.deepEqual(wifi30, [P.ACCESS_FINE_LOCATION]);

const lan37 = buildAndroidMeshPermissionList(
  { ble: false, wifiDirect: false, localNetwork: true },
  ANDROID_API_LOCAL_NETWORK,
);
assert.deepEqual(lan37, [P.ACCESS_LOCAL_NETWORK]);
// The literal string, not PermissionsAndroid.PERMISSIONS.ACCESS_LOCAL_NETWORK
// (undefined before React Native 0.87, which the stub reproduces).
assert.equal(lan37[0], 'android.permission.ACCESS_LOCAL_NETWORK');

const lan36 = buildAndroidMeshPermissionList({ ble: false, localNetwork: true }, 36);
assert.deepEqual(lan36, []);

const combined = buildAndroidMeshPermissionList(
  { ble: true, wifiDirect: true, localNetwork: true },
  ANDROID_API_LOCAL_NETWORK,
);
assert.ok(combined.includes(P.BLUETOOTH_SCAN));
assert.ok(combined.includes(P.NEARBY_WIFI_DEVICES));
assert.ok(combined.includes(P.ACCESS_LOCAL_NETWORK));

assert.equal(ANDROID_API_BLUETOOTH_RUNTIME, 31);
assert.ok(!combined.includes(undefined), 'no undefined permission in the list');

const WIFI_DIRECT_CONFIG = {
  transports: { ble: { enabled: true }, wifiDirect: { enabled: true } },
};

async function main() {
  // The config never opts in to ACCESS_LOCAL_NETWORK: only the app knows its
  // targetSdkVersion and whether its manifest declares the permission.
  stub.Platform.OS = 'android';
  assert.deepEqual(meshPermissionOptionsFromConfig(WIFI_DIRECT_CONFIG), {
    ble: true,
    wifiDirect: true,
    localNetwork: false,
  });

  // API 37 device, LAN not opted in. An app targeting 36 or lower (or one that
  // never declared the permission) would have ACCESS_LOCAL_NETWORK denied
  // without a prompt; it must be neither requested nor counted.
  {
    const prompted = androidDevice(ANDROID_API_LOCAL_NETWORK, {
      [P.ACCESS_LOCAL_NETWORK]: 'denied',
    });
    const result = await requestMeshPermissionsForConfig(WIFI_DIRECT_CONFIG);
    assert.ok(!prompted.includes(P.ACCESS_LOCAL_NETWORK), 'LAN not requested');
    assert.deepEqual(prompted, [
      P.BLUETOOTH_SCAN,
      P.BLUETOOTH_CONNECT,
      P.BLUETOOTH_ADVERTISE,
      P.NEARBY_WIFI_DEVICES,
    ]);
    assert.equal(result.granted, true);
    assert.equal(result.platform, 'android');
    assert.ok(!(P.ACCESS_LOCAL_NETWORK in result.android));
  }

  // Same device, default options (BLE only): no LAN request either.
  {
    const prompted = androidDevice(ANDROID_API_LOCAL_NETWORK);
    const result = await requestMeshPermissions();
    assert.ok(!prompted.includes(P.ACCESS_LOCAL_NETWORK));
    assert.equal(result.granted, true);
  }

  // Opted in on API 37: requested by its literal string and counted.
  {
    const prompted = androidDevice(ANDROID_API_LOCAL_NETWORK);
    const result = await requestMeshPermissions({
      ...meshPermissionOptionsFromConfig(WIFI_DIRECT_CONFIG),
      localNetwork: true,
    });
    assert.ok(prompted.includes('android.permission.ACCESS_LOCAL_NETWORK'));
    assert.ok(!prompted.includes(undefined));
    assert.equal(result.granted, true);
    assert.equal(result.android[P.ACCESS_LOCAL_NETWORK], 'granted');
  }

  // Opted in and the user refuses: granted is false.
  {
    androidDevice(ANDROID_API_LOCAL_NETWORK, {
      [P.ACCESS_LOCAL_NETWORK]: 'never_ask_again',
    });
    const result = await requestMeshPermissions({ ble: true, localNetwork: true });
    assert.equal(result.granted, false);
    assert.equal(result.android[P.ACCESS_LOCAL_NETWORK], 'never_ask_again');
    assert.equal(result.android[P.BLUETOOTH_SCAN], 'granted');
  }

  // Opted in on an API 36 device: nothing to request for LAN.
  {
    const prompted = androidDevice(36);
    const result = await requestMeshPermissions({ ble: false, localNetwork: true });
    assert.deepEqual(prompted, []);
    assert.equal(result.granted, true);
  }

  console.log('mesh-permissions.test.js: ok');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
