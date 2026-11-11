#!/usr/bin/env node
'use strict';

/**
 * Unit tests for `buildAndroidMeshPermissionList` (no native runtime).
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

const realLoad = Module._load;
Module._load = function loadWithReactNativeStub(request) {
  if (request === 'react-native') {
    return {
      Platform: { OS: 'android', Version: 33 },
      PermissionsAndroid: {
        PERMISSIONS: P,
        RESULTS: {
          GRANTED: 'granted',
          DENIED: 'denied',
          NEVER_ASK_AGAIN: 'never_ask_again',
        },
        check: async () => true,
        requestMultiple: async () => ({}),
      },
    };
  }
  return realLoad.apply(this, arguments);
};

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

console.log('mesh-permissions.test.js: ok');
