#!/usr/bin/env node
'use strict';

/**
 * Unit tests for mesh address shape helpers (no native runtime).
 */

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');

const PACKAGE_DIR = path.resolve(__dirname, '..');
const VALID = 'off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn';

function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run npm ci in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-addr-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' },
  );
  return outDir;
}

const outDir = compileSdk();
const {
  looksLikeMeshAddress,
  MESH_ADDRESS_ENCODED_LEN,
  devPeerAddressWarningsEnabled,
  peerAddressMisshapeMessage,
} = require(path.join(outDir, 'peerAddressHints.js'));

assert.equal(MESH_ADDRESS_ENCODED_LEN, 44);
assert.ok(looksLikeMeshAddress(VALID));
assert.ok(!looksLikeMeshAddress('user456'));
assert.ok(!looksLikeMeshAddress('bob'));
assert.ok(!looksLikeMeshAddress(VALID.toUpperCase()));
assert.ok(!looksLikeMeshAddress(`${VALID}x`));
assert.ok(!looksLikeMeshAddress('off2qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn'));

assert.equal(devPeerAddressWarningsEnabled(false), false);
assert.equal(devPeerAddressWarningsEnabled(true), true);

const msg = peerAddressMisshapeMessage('user456', 'sendMessage');
assert.match(msg, /sendMessage/);
assert.match(msg, /user456/);
assert.match(msg, /identity-for-integrators/);

console.log('6 passed.');
