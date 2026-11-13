#!/usr/bin/env node
'use strict';

/**
 * `mesh-sdk-doctor` (`scripts/mesh-doctor-checks.js`, `scripts/mesh-doctor.js`).
 *
 * Pins two things:
 *
 *   1. Every finding code fires on the setup it names.
 *   2. Setups that work are not errors: a clean app, an Expo app built
 *      natively without expo-dev-client, a Podfile that keeps the old pod line
 *      as a comment, and a Podfile that sets an unrelated linker flag. A false
 *      error exits 1 on a working app.
 *
 * The CLI cases run the real script, so the exit codes and `--root` parsing
 * are covered too.
 */

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const {
  runMeshDoctorChecks,
  exitCodeFromFindings,
} = require('../scripts/mesh-doctor-checks');

const CLI = path.resolve(__dirname, '..', 'scripts', 'mesh-doctor.js');
const SDK_DEP = { '@offline-protocol/mesh-sdk': '0.33.0' };

const roots = [];
process.on('exit', () => {
  for (const dir of roots) {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

function tmpRoot(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-mesh-doctor-'));
  roots.push(dir);
  for (const [rel, content] of Object.entries(files)) {
    const target = path.join(dir, rel);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content);
  }
  return dir;
}

function manifest(dependencies) {
  return JSON.stringify({ dependencies });
}

function codes(findings) {
  return findings.map((f) => `${f.severity}:${f.code}`).sort();
}

function runCli(args, cwd) {
  return spawnSync(process.execPath, [CLI, ...args], { cwd, encoding: 'utf8' });
}

let passed = 0;
function test(name, fn) {
  fn();
  passed += 1;
  console.log(`  ✓ ${name}`);
}

test('a clean app reports nothing and exits 0', () => {
  const root = tmpRoot({
    'package.json': manifest(SDK_DEP),
    'ios/Podfile': "# clean autolinking only\nuse_react_native!\n",
  });
  const findings = runMeshDoctorChecks(root);
  assert.deepEqual(findings, []);
  assert.equal(exitCodeFromFindings(findings), 0);
});

test('Expo with neither expo-dev-client nor native projects warns about Expo Go', () => {
  const root = tmpRoot({ 'package.json': manifest({ ...SDK_DEP, expo: '52.0.0' }) });
  const findings = runMeshDoctorChecks(root);
  assert.deepEqual(codes(findings), ['warn:expo-without-dev-client']);
  assert.match(findings[0].message, /probably launched in Expo Go/);
  assert.equal(exitCodeFromFindings(findings), 0);
});

test('an Expo app built natively without expo-dev-client is a warning, not an error', () => {
  const root = tmpRoot({
    'package.json': manifest({ ...SDK_DEP, expo: '52.0.0' }),
    'app.json': JSON.stringify({ expo: { name: 'x' } }),
    'ios/Podfile': 'use_expo_modules!\n',
  });
  const findings = runMeshDoctorChecks(root);
  assert.deepEqual(codes(findings), ['warn:expo-without-dev-client']);
  assert.match(findings[0].message, /Native builds .* work/);
  assert.equal(exitCodeFromFindings(findings), 0);
});

test('Expo with expo-dev-client in devDependencies reports nothing', () => {
  const root = tmpRoot({
    'package.json': JSON.stringify({
      dependencies: { ...SDK_DEP, expo: '52.0.0' },
      devDependencies: { 'expo-dev-client': '5.0.0' },
    }),
  });
  assert.deepEqual(runMeshDoctorChecks(root), []);
});

test("a live pod 'MeshSdk' line is an error", () => {
  const root = tmpRoot({
    'package.json': manifest(SDK_DEP),
    'ios/Podfile': "target 'App' do\n  pod 'MeshSdk', :path => '../node_modules/foo'\nend\n",
  });
  const findings = runMeshDoctorChecks(root);
  assert.deepEqual(codes(findings), ['error:legacy-meshsdk-pod']);
  assert.equal(exitCodeFromFindings(findings), 1);
});

test('a commented-out pod line, or one inside =begin/=end, is not reported', () => {
  const root = tmpRoot({
    'package.json': manifest(SDK_DEP),
    'ios/Podfile':
      "  # pod 'MeshSdk', :path => '../x'  (removed in 0.20.0)\n" +
      "  # flags = '-loffline_protocol_uniffi_sim'\n" +
      "=begin\npod 'MeshSdk'\nconfig.build_settings['OTHER_LDFLAGS'] = '-loffline_protocol_uniffi_sim'\n=end\n",
  });
  assert.deepEqual(runMeshDoctorChecks(root), []);
});

test('the removed uniffi archives are an error', () => {
  const root = tmpRoot({
    'package.json': manifest(SDK_DEP),
    'ios/Podfile': "flags = '-loffline_protocol_uniffi_device'\n",
  });
  assert.deepEqual(codes(runMeshDoctorChecks(root)), ['error:legacy-uniffi-archives']);
});

test('a post_install hook configuring MeshSdk is a linker-hack warning', () => {
  const root = tmpRoot({
    'package.json': manifest(SDK_DEP),
    'ios/Podfile':
      'post_install do |installer|\n' +
      '  installer.pods_project.targets.each do |t|\n' +
      "    next unless t.name == 'MeshSdk'\n" +
      "    t.build_configurations.each { |c| c.build_settings['DEFINES_MODULE'] = 'YES' }\n" +
      '  end\nend\n',
  });
  assert.deepEqual(codes(runMeshDoctorChecks(root)), ['warn:legacy-linker-hacks']);
});

test('an unrelated linker flag far from any MeshSdk mention is not reported', () => {
  const root = tmpRoot({
    'package.json': manifest(SDK_DEP),
    'ios/Podfile':
      'use_react_native!  # MeshSdk is autolinked\n' +
      '\n'.repeat(10) +
      "post_install do |i|\n  config.build_settings['OTHER_LDFLAGS'] = '-ObjC'\nend\n",
  });
  assert.deepEqual(runMeshDoctorChecks(root), []);
});

test('a stale liboffline_protocol_uniffi.so under jniLibs is a warning naming its ABIs', () => {
  const root = tmpRoot({
    'package.json': manifest(SDK_DEP),
    'android/app/src/main/jniLibs/arm64-v8a/liboffline_protocol_uniffi.so': '',
    'android/app/src/main/jniLibs/x86_64/libother.so': '',
  });
  const findings = runMeshDoctorChecks(root);
  assert.deepEqual(codes(findings), ['warn:android-stale-jnilib']);
  assert.match(findings[0].message, /\(arm64-v8a\)/);
});

test('a missing SDK dependency is a warning', () => {
  const root = tmpRoot({ 'package.json': manifest({ react: '19.0.0' }) });
  assert.deepEqual(codes(runMeshDoctorChecks(root)), ['warn:mesh-sdk-missing']);
});

test('a missing, malformed or non-object package.json is an error, never a crash', () => {
  const odd = tmpRoot({
    'package.json': JSON.stringify({ dependencies: SDK_DEP, devDependencies: 'oops' }),
  });
  assert.deepEqual(runMeshDoctorChecks(odd), [], 'a non-object section is ignored');
  assert.deepEqual(codes(runMeshDoctorChecks(tmpRoot({}))), ['error:no-package-json']);
  for (const body of ['{', 'null', '[]', '"x"']) {
    const findings = runMeshDoctorChecks(tmpRoot({ 'package.json': body }));
    assert.deepEqual(codes(findings), ['error:package-json-invalid'], `package.json = ${body}`);
  }
});

test('the CLI exits 1 on an error and 0 on warnings only, and prints the docs URL', () => {
  const broken = tmpRoot({
    'package.json': manifest(SDK_DEP),
    'ios/Podfile': "pod 'MeshSdk'\n",
  });
  const errorRun = runCli(['--root', broken], os.tmpdir());
  assert.equal(errorRun.status, 1, errorRun.stderr);
  assert.match(errorRun.stdout, /ERROR \[legacy-meshsdk-pod\]/);
  assert.match(errorRun.stdout, /https:\/\/github\.com\/.*react-native-host-doctor\.md/);

  const warned = tmpRoot({ 'package.json': manifest({ ...SDK_DEP, expo: '52.0.0' }) });
  const warnRun = runCli([`--root=${warned}`], os.tmpdir());
  assert.equal(warnRun.status, 0, warnRun.stderr);
  assert.match(warnRun.stdout, /WARN \[expo-without-dev-client\]/);
});

test('the CLI defaults to the working directory', () => {
  const clean = tmpRoot({ 'package.json': manifest(SDK_DEP) });
  const run = runCli([], clean);
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /no issues found/);
});

test('the CLI refuses --root without a value and unknown arguments with exit 2', () => {
  for (const args of [['--root'], ['--root', '--verbose'], ['--root='], ['--verbose']]) {
    const run = runCli(args, os.tmpdir());
    assert.equal(run.status, 2, `args ${JSON.stringify(args)}: ${run.stdout}`);
    assert.match(run.stderr, /Usage: mesh-sdk-doctor/);
  }
});

console.log(`\n${passed} passed.`);
