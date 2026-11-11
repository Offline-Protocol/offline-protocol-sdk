#!/usr/bin/env node
'use strict';

/**
 * Scans a React Native app for legacy iOS/Android integration leftovers and
 * unsupported Expo Go setups (G5). Run from the app root:
 *
 *   npx mesh-sdk-doctor
 *   # or, in this monorepo:
 *   node node_modules/@offline-protocol/mesh-sdk/scripts/mesh-doctor.js
 */

const path = require('node:path');
const { runMeshDoctorChecks, exitCodeFromFindings } = require('./mesh-doctor-checks');

function parseRoot(argv) {
  const idx = argv.indexOf('--root');
  if (idx !== -1 && argv[idx + 1]) {
    return path.resolve(argv[idx + 1]);
  }
  return process.cwd();
}

const root = parseRoot(process.argv.slice(2));
const findings = runMeshDoctorChecks(root);

if (findings.length === 0) {
  console.log('mesh-doctor: no issues found.');
  console.log('Docs: https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/react-native-host-doctor.md');
  process.exit(0);
}

for (const f of findings) {
  const label = f.severity === 'error' ? 'ERROR' : 'WARN';
  console.log(`${label} [${f.code}] ${f.message}`);
  if (f.fix) {
    console.log(`  → ${f.fix}`);
  }
}

console.log('');
console.log('See docs/react-native-host-doctor.md and docs/UPGRADING.md §12.1.');

process.exit(exitCodeFromFindings(findings));
