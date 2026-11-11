#!/usr/bin/env node
'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const {
  runMeshDoctorChecks,
  exitCodeFromFindings,
} = require('../scripts/mesh-doctor-checks');

function tmpRoot(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-mesh-doctor-'));
  for (const [rel, content] of Object.entries(files)) {
    const target = path.join(dir, rel);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content);
  }
  return dir;
}

{
  const root = tmpRoot({
    'package.json': JSON.stringify({
      dependencies: { expo: '52.0.0', react: '19.0.0' },
    }),
  });
  const findings = runMeshDoctorChecks(root);
  assert.ok(findings.some((f) => f.code === 'expo-go-unsupported'));
  assert.equal(exitCodeFromFindings(findings), 1);
  console.log('ok   expo without dev client is an error');
}

{
  const root = tmpRoot({
    'package.json': JSON.stringify({
      dependencies: {
        expo: '52.0.0',
        'expo-dev-client': '5.0.0',
        '@offline-protocol/mesh-sdk': '0.33.0',
      },
    }),
    'ios/Podfile': "pod 'MeshSdk', :path => '../node_modules/foo'\n",
  });
  const findings = runMeshDoctorChecks(root);
  assert.ok(findings.some((f) => f.code === 'legacy-meshsdk-pod'));
  console.log('ok   legacy MeshSdk pod line is flagged');
}

{
  const root = tmpRoot({
    'package.json': JSON.stringify({
      dependencies: { '@offline-protocol/mesh-sdk': '0.33.0' },
    }),
    'ios/Podfile': '# clean autolinking only\n',
  });
  const findings = runMeshDoctorChecks(root);
  assert.equal(findings.length, 0);
  assert.equal(exitCodeFromFindings(findings), 0);
  console.log('ok   clean project reports no issues');
}

console.log('mesh-doctor.test.js: ok');
