#!/usr/bin/env node
'use strict';

/**
 * Scans a React Native app for legacy iOS/Android integration leftovers and
 * Expo setups that cannot load native modules. Run from the app root:
 *
 *   npx --no-install mesh-sdk-doctor
 *   npx --no-install mesh-sdk-doctor --root path/to/app
 *
 * `--no-install` matters: without it, npx run where the SDK is not installed
 * fetches whatever the public registry holds under this bin's name.
 *
 * Exit codes: 0 clean or warnings only, 1 at least one error, 2 bad usage.
 */

const path = require('node:path');
const { DOCS_URL, runMeshDoctorChecks, exitCodeFromFindings } = require('./mesh-doctor-checks');

const USAGE = 'Usage: mesh-sdk-doctor [--root <app dir>]';

/**
 * @param {string[]} argv
 * @returns {{ root: string } | { usageError: string }}
 */
function parseArgs(argv) {
  let root = process.cwd();
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--help' || arg === '-h') {
      return { usageError: '' };
    }
    if (arg === '--root') {
      const value = argv[i + 1];
      if (!value || value.startsWith('-')) {
        return { usageError: '--root needs a directory.' };
      }
      root = path.resolve(value);
      i += 1;
      continue;
    }
    if (arg.startsWith('--root=')) {
      const value = arg.slice('--root='.length);
      if (!value) {
        return { usageError: '--root needs a directory.' };
      }
      root = path.resolve(value);
      continue;
    }
    return { usageError: `Unknown argument: ${arg}` };
  }
  return { root };
}

const parsed = parseArgs(process.argv.slice(2));
if ('usageError' in parsed) {
  if (parsed.usageError) {
    console.error(`mesh-sdk-doctor: ${parsed.usageError}`);
    console.error(USAGE);
    process.exit(2);
  }
  console.log(USAGE);
  process.exit(0);
}

const findings = runMeshDoctorChecks(parsed.root);

if (findings.length === 0) {
  console.log(`mesh-sdk-doctor: no issues found in ${parsed.root}.`);
  console.log(`Docs: ${DOCS_URL}`);
  process.exit(0);
}

for (const f of findings) {
  const label = f.severity === 'error' ? 'ERROR' : 'WARN';
  console.log(`${label} [${f.code}] ${f.message}`);
  if (f.fix) {
    console.log(`  fix: ${f.fix}`);
  }
}

console.log('');
console.log(`Docs: ${DOCS_URL}`);

process.exit(exitCodeFromFindings(findings));
