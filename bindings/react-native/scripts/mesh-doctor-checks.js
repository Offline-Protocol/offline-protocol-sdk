'use strict';

const fs = require('node:fs');
const path = require('node:path');

/**
 * Static checks behind `mesh-sdk-doctor`. Every check reads files only; none
 * runs CocoaPods, Gradle or the app.
 *
 * Severity is a promise to the reader: `error` means the app cannot work as
 * written, and the CLI exits 1. Anything the doctor cannot prove from files
 * (whether the app is launched in Expo Go, whether a linker flag is still
 * needed) is a `warn`. A doctor that fails a working app teaches people to
 * stop running it.
 *
 * @typedef {{ severity: 'error' | 'warn', code: string, message: string, fix?: string }} Finding
 */

const DOCS_URL =
  'https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/react-native-host-doctor.md';
const UPGRADING_URL =
  'https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/UPGRADING.md#121-react-native-ios-delete-your-manual-pod-meshsdk-line-v0200';

/** Build settings the pre-0.20.0 integration told apps to set for MeshSdk. */
const LEGACY_BUILD_SETTINGS =
  /\b(OTHER_LDFLAGS|LIBRARY_SEARCH_PATHS|SWIFT_INCLUDE_PATHS|DEFINES_MODULE)\b/;
const MESH_REFERENCE = /MeshSdk|offline_protocol/i;
/** How many lines either side of a build setting may name MeshSdk. */
const LINKER_HACK_WINDOW = 5;

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
      fix: 'Run this command from your React Native app root, or pass --root <dir>.',
    });
    return findings;
  }

  let pkg;
  try {
    pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
  } catch {
    pkg = undefined;
  }
  // `null`, an array or a bare value parses but is not a manifest.
  if (pkg === null || typeof pkg !== 'object' || Array.isArray(pkg)) {
    findings.push({
      severity: 'error',
      code: 'package-json-invalid',
      message: `${pkgPath} is not a valid package manifest (expected a JSON object).`,
    });
    return findings;
  }

  // Spreading a missing or non-object section adds nothing and never throws.
  const deps = { ...pkg.dependencies, ...pkg.devDependencies };
  if (!deps['@offline-protocol/mesh-sdk']) {
    findings.push({
      severity: 'warn',
      code: 'mesh-sdk-missing',
      message: '@offline-protocol/mesh-sdk is not listed in dependencies.',
      fix: 'npm install @offline-protocol/mesh-sdk',
    });
  }

  checkExpoHost(root, deps, findings);
  checkIosPodfile(root, findings);
  checkAndroidJniLibs(root, findings);

  return findings;
}

/**
 * Expo without `expo-dev-client` is not proof of Expo Go: an app built with
 * `npx expo prebuild` / `npx expo run:ios`, or a bare app that uses Expo
 * modules, loads native modules without it. So this is a warning, and its
 * wording says which launch path fails.
 *
 * @param {string} root
 * @param {Record<string, string>} deps
 * @param {Finding[]} findings
 */
function checkExpoHost(root, deps, findings) {
  if (!deps.expo || deps['expo-dev-client']) {
    return;
  }
  const hasNativeProjects =
    fs.existsSync(path.join(root, 'ios')) || fs.existsSync(path.join(root, 'android'));
  findings.push({
    severity: 'warn',
    code: 'expo-without-dev-client',
    message: hasNativeProjects
      ? 'Expo is present without expo-dev-client. Native builds (npx expo run:ios / run:android) work; launching in Expo Go will not load this SDK.'
      : 'Expo is present without expo-dev-client or native ios/ and android/ projects, so the app is probably launched in Expo Go, which cannot load this SDK.',
    fix: `Use a development build (expo-dev-client) or npx expo run:ios / run:android. See ${DOCS_URL}`,
  });
}

/**
 * Drops whole-line Ruby comments, so a `# pod 'MeshSdk'` kept as a note after
 * the upgrade is not reported as a live line. Trailing comments are left in
 * place: cutting at `#` would also cut `#{...}` interpolation.
 *
 * @param {string} text
 * @returns {string[]}
 */
function podfileCodeLines(text) {
  const lines = [];
  let inBlockComment = false;
  for (const line of text.split(/\r?\n/)) {
    if (/^=begin\b/.test(line)) {
      inBlockComment = true;
      continue;
    }
    if (inBlockComment) {
      if (/^=end\b/.test(line)) {
        inBlockComment = false;
      }
      continue;
    }
    lines.push(/^\s*#/.test(line) ? '' : line);
  }
  return lines;
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
  const lines = podfileCodeLines(fs.readFileSync(podfile, 'utf8'));
  const code = lines.join('\n');

  if (/^\s*pod\s+['"]MeshSdk['"]/im.test(code)) {
    findings.push({
      severity: 'error',
      code: 'legacy-meshsdk-pod',
      message: "Podfile still declares pod 'MeshSdk' by hand.",
      fix: `Delete the line; autolinking supplies MeshSdk since 0.20.0. See ${UPGRADING_URL}`,
    });
  }

  if (/offline_protocol_uniffi_(sim|device)\b/i.test(code)) {
    findings.push({
      severity: 'error',
      code: 'legacy-uniffi-archives',
      message:
        'Podfile names the offline_protocol_uniffi_sim or offline_protocol_uniffi_device archive, which no longer exist.',
      fix: `Delete those flags; the podspec links the XCFramework. See ${UPGRADING_URL}`,
    });
  }

  // A build setting alone is ordinary (`-ObjC`), and MeshSdk alone is ordinary
  // (a comment). Only the two close together look like the old hook.
  const linkerHack = lines.some((line, i) => {
    if (!LEGACY_BUILD_SETTINGS.test(line)) {
      return false;
    }
    const from = Math.max(0, i - LINKER_HACK_WINDOW);
    return lines.slice(from, i + LINKER_HACK_WINDOW + 1).some((l) => MESH_REFERENCE.test(l));
  });
  if (linkerHack) {
    findings.push({
      severity: 'warn',
      code: 'legacy-linker-hacks',
      message:
        'Podfile appears to set build settings (OTHER_LDFLAGS, LIBRARY_SEARCH_PATHS, SWIFT_INCLUDE_PATHS or DEFINES_MODULE) for MeshSdk.',
      fix: `Delete post_install code that configures MeshSdk; the podspec sets what it needs. See ${UPGRADING_URL}`,
    });
  }
}

/**
 * The package ships its own `libuniffi_offline_protocol.so` (the name the
 * generated Kotlin loads). A `liboffline_protocol_uniffi.so` under the app's
 * own jniLibs is a leftover from copying the cargo output by hand: never
 * loaded, and shipped in the APK anyway.
 *
 * @param {string} root
 * @param {Finding[]} findings
 */
function checkAndroidJniLibs(root, findings) {
  const jniLibs = path.join(root, 'android', 'app', 'src', 'main', 'jniLibs');
  let abis;
  try {
    abis = fs.readdirSync(jniLibs, { withFileTypes: true }).filter((d) => d.isDirectory());
  } catch {
    return;
  }
  const stale = abis
    .filter((d) => fs.existsSync(path.join(jniLibs, d.name, 'liboffline_protocol_uniffi.so')))
    .map((d) => d.name);
  if (stale.length > 0) {
    findings.push({
      severity: 'warn',
      code: 'android-stale-jnilib',
      message: `android/app/src/main/jniLibs holds liboffline_protocol_uniffi.so (${stale.join(', ')}), which nothing loads.`,
      fix: 'Delete it; @offline-protocol/mesh-sdk ships libuniffi_offline_protocol.so for every ABI.',
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
  DOCS_URL,
  runMeshDoctorChecks,
  exitCodeFromFindings,
};
