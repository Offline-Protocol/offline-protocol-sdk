'use strict';

const fs = require('node:fs');
const path = require('node:path');

/**
 * @typedef {{ severity: 'error' | 'warn', code: string, message: string, fix?: string }} Finding
 */

/**
 * @param {string} root App project root (contains package.json).
 * @returns {Finding[]}
 */
function runMeshDoctorChecks(root) {
  /** @type {Finding[]} */
  const findings = [];

  const pkgPath = path.join(root, 'package.json');
  if (!fs.existsSync(pkgPath)) {
    findings.push({
      severity: 'error',
      code: 'no-package-json',
      message: `No package.json at ${root}`,
      fix: 'Run this command from your React Native app root.',
    });
    return findings;
  }

  let pkg;
  try {
    pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
  } catch {
    findings.push({
      severity: 'error',
      code: 'package-json-invalid',
      message: 'package.json is not valid JSON.',
    });
    return findings;
  }

  const deps = { ...pkg.dependencies, ...pkg.devDependencies };
  const meshDep =
    deps['@offline-protocol/mesh-sdk'] ?? deps['offline-protocol-mesh-sdk'];
  if (!meshDep) {
    findings.push({
      severity: 'warn',
      code: 'mesh-sdk-missing',
      message: '@offline-protocol/mesh-sdk is not listed in dependencies.',
      fix: 'npm install @offline-protocol/mesh-sdk',
    });
  }

  checkExpoHost(root, pkg, deps, findings);
  checkIosPodfile(root, findings);
  checkAndroidGradle(root, findings);

  return findings;
}

/**
 * @param {string} root
 * @param {Record<string, string>} pkg
 * @param {Record<string, string>} deps
 * @param {Finding[]} findings
 */
function checkExpoHost(root, pkg, deps, findings) {
  const hasExpo = Boolean(deps.expo);
  const hasDevClient =
    Boolean(deps['expo-dev-client']) ||
    Boolean(pkg.dependencies?.['expo-dev-client']);

  if (hasExpo && !hasDevClient) {
    findings.push({
      severity: 'error',
      code: 'expo-go-unsupported',
      message:
        'Expo is present but expo-dev-client is not. Expo Go cannot load this SDK (native module).',
      fix:
        'Add a development build (expo-dev-client) and run a custom dev client, or use bare React Native. See docs/react-native-host-doctor.md.',
    });
  }

  for (const name of ['app.json', 'app.config.js', 'app.config.ts']) {
    const p = path.join(root, name);
    if (!fs.existsSync(p) || name !== 'app.json') {
      continue;
    }
    try {
      const app = JSON.parse(fs.readFileSync(p, 'utf8'));
      if (app.expo && !hasDevClient) {
        findings.push({
          severity: 'warn',
          code: 'expo-config-without-dev-client',
          message:
            'app.json declares an Expo project without expo-dev-client in package.json.',
          fix:
            'Use EAS dev client or prebuild + native run; Expo Go will fail at runtime.',
        });
      }
    } catch {
      /* ignore */
    }
  }
}

/**
 * @param {string} root
 * @param {Finding[]} findings
 */
function checkIosPodfile(root, findings) {
  const podfile = path.join(root, 'ios', 'Podfile');
  if (!fs.existsSync(podfile)) {
    return;
  }
  const text = fs.readFileSync(podfile, 'utf8');

  if (/pod\s+['"]MeshSdk['"]/i.test(text)) {
    findings.push({
      severity: 'error',
      code: 'legacy-meshsdk-pod',
      message: 'Podfile still contains a manual pod \'MeshSdk\' line.',
      fix:
        'Remove the manual pod line. Autolinking supplies MeshSdk since 0.20.0. See docs/UPGRADING.md §12.1.',
    });
  }

  if (/offline_protocol_uniffi_(sim|device)/i.test(text)) {
    findings.push({
      severity: 'error',
      code: 'legacy-uniffi-archives',
      message:
        'Podfile references offline_protocol_uniffi_sim or offline_protocol_uniffi_device archives.',
      fix:
        'Remove manual linker flags; the XCFramework is selected by CocoaPods. See docs/UPGRADING.md §12.1.',
    });
  }

  if (
    /LIBRARY_SEARCH_PATHS|OTHER_LDFLAGS/i.test(text) &&
    /MeshSdk|offline_protocol/i.test(text)
  ) {
    findings.push({
      severity: 'warn',
      code: 'legacy-linker-hacks',
      message:
        'Podfile post_install may still manipulate linker settings for MeshSdk.',
      fix:
        'Delete post_install hooks that set OTHER_LDFLAGS or LIBRARY_SEARCH_PATHS for MeshSdk.',
    });
  }
}

/**
 * @param {string} root
 * @param {Finding[]} findings
 */
function checkAndroidGradle(root, findings) {
  const appGradle = path.join(root, 'android', 'app', 'build.gradle');
  if (!fs.existsSync(appGradle)) {
    return;
  }
  const text = fs.readFileSync(appGradle, 'utf8');
  if (/liboffline_protocol_uniffi\.so/i.test(text) && !/libuniffi_offline_protocol/i.test(text)) {
    findings.push({
      severity: 'warn',
      code: 'android-jni-name',
      message:
        'Gradle may copy liboffline_protocol_uniffi.so; the loader expects libuniffi_offline_protocol.so.',
      fix: 'Rename jniLibs per bindings/react-native README Android section or use autolinking.',
    });
  }
}

/**
 * @param {Finding[]} findings
 * @returns {number} Exit code (0 ok, 1 if any error)
 */
function exitCodeFromFindings(findings) {
  return findings.some((f) => f.severity === 'error') ? 1 : 0;
}

module.exports = {
  runMeshDoctorChecks,
  exitCodeFromFindings,
};
