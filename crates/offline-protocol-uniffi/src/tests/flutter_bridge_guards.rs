//! Flutter binding compile lists, method-channel parity, docs, and CI wiring.
//!
//! Kept in this file (not `lib.rs`) so CI can run these guards on the fast
//! `flutter_dart` path while any production UniFFI edit in `lib.rs` triggers
//! the native jniLibs + AAR gate.

/// Parses `OfflineProtocol.nativeMethodChannelMethods` from the Dart source.
fn flutter_dart_method_channel_methods(dart: &str) -> Vec<String> {
    const MARKER: &str = "static const List<String> nativeMethodChannelMethods = [";
    let tail = dart
        .split(MARKER)
        .nth(1)
        .unwrap_or_else(|| panic!("{MARKER} not found in offline_protocol.dart"));
    let body = tail
        .split("];")
        .next()
        .unwrap_or_else(|| panic!("nativeMethodChannelMethods list not terminated"));
    body.lines()
        .filter_map(|line| {
            let line = line.trim().trim_end_matches(',');
            let inner = line.strip_prefix('\'')?.strip_suffix('\'')?;
            Some(inner.to_string())
        })
        .collect()
}

/// Method names handled in `OfflineProtocolPlugin.kt` `when (call.method)`.
fn flutter_kotlin_method_channel_methods(kt: &str) -> std::collections::BTreeSet<String> {
    const ANCHOR: &str = "when (call.method) {";
    let block = kt
        .split(ANCHOR)
        .nth(1)
        .unwrap_or_else(|| panic!("{ANCHOR} not found in OfflineProtocolPlugin.kt"));
    let block = block
        .split("else ->")
        .next()
        .unwrap_or_else(|| panic!("when (call.method) missing else arm"));
    let mut names = std::collections::BTreeSet::new();
    for line in block.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let Some((method, _)) = rest.split_once('"') else {
            continue;
        };
        if !method.is_empty() {
            names.insert(method.to_string());
        }
    }
    names
}

/// Method names handled in `OfflineProtocolPlugin.swift` `switch call.method`.
fn flutter_swift_method_channel_methods(swift: &str) -> std::collections::BTreeSet<String> {
    const ANCHOR: &str = "switch call.method {";
    let block = swift
        .split(ANCHOR)
        .nth(1)
        .unwrap_or_else(|| panic!("{ANCHOR} not found in OfflineProtocolPlugin.swift"));
    let block = block
        .split("default:")
        .next()
        .unwrap_or_else(|| panic!("switch call.method missing default arm"));
    let mut names = std::collections::BTreeSet::new();
    for line in block.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("case \"") else {
            continue;
        };
        let Some((method, _)) = rest.split_once('"') else {
            continue;
        };
        if method.contains(',') {
            continue;
        }
        names.insert(method.to_string());
    }
    names
}

fn flutter_plugin_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bindings/flutter/offline_protocol")
}

fn rn_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bindings/react-native")
}

fn read_text(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Shared bridge sources the plugin must compile: the Internet stack, the
/// local transports, event replay and foreground reconnect. Excluding one
/// builds green and ships a plugin that cannot run the transport.
const FLUTTER_SHARED_SOURCES: &[&str] = &[
    "InternetTransportBootstrap",
    "InternetManager",
    "PresenceWatchPolicy",
    "EventReplayRing",
    "ForegroundReconnectPolicy",
    "WifiDirectManager",
    "ProtocolConfigParser",
    "InitialRuntimeConfigApplier",
];

/// Every `exclude('com/offlineprotocol/X.kt')` in the plugin's Gradle build.
fn flutter_kotlin_excludes(gradle: &str) -> Vec<String> {
    gradle
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("exclude('")?;
            Some(rest.split_once('\'')?.0.to_string())
        })
        .collect()
}

// The Android compile surface is every shared source under the React Native
// tree less the KotlinCompile excludes (ADR 0025 decision 3). A name that
// matches no file is a rename nobody carried over, and the renamed file, which
// may need React, would be compiled in its place. A list of what to take must
// not come back: the Kotlin plugin ignores `java { include }`, so such a list
// drives nothing while looking like the compile surface.
#[test]
fn flutter_android_excludes_name_real_files() {
    let gradle = read_text(&flutter_plugin_dir().join("android/build.gradle"));
    let excludes = flutter_kotlin_excludes(&gradle);
    assert!(
        excludes.len() >= 3,
        "found only {} KotlinCompile excludes in the Flutter android/build.gradle; \
         the parser is broken, not the build: {excludes:?}",
        excludes.len()
    );
    let java = rn_dir().join("android/src/main/java");
    for rel in &excludes {
        assert!(
            java.join(rel).is_file(),
            "Flutter android/build.gradle excludes `{rel}`, which does not exist under \
             bindings/react-native/android/src/main/java: carry the rename over"
        );
    }
    for name in FLUTTER_SHARED_SOURCES {
        assert!(
            !excludes
                .iter()
                .any(|rel| rel.ends_with(&format!("/{name}.kt"))),
            "Flutter android/build.gradle must not exclude the shared source `{name}.kt`"
        );
    }
    let code: String = gradle
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("include it") && !code.contains("include '"),
        "Flutter android/build.gradle must not take sources by an include list: the \
         Kotlin plugin ignores it (ADR 0025 decision 3)"
    );
}

/// The quoted `'BridgeRN/X'` entries of `rn_only_swift` in the podspec.
fn flutter_podspec_excludes(podspec: &str) -> std::collections::BTreeSet<String> {
    let body = podspec
        .split("rn_only_swift = [")
        .nth(1)
        .and_then(|tail| tail.split(']').next())
        .unwrap_or_else(|| panic!("rn_only_swift = [ ... ] not found in offline_protocol.podspec"));
    body.lines()
        .filter_map(|line| {
            let line = line.trim().trim_end_matches(',');
            let inner = line.strip_prefix('\'')?.strip_suffix('\'')?;
            Some(inner.strip_prefix("BridgeRN/")?.to_string())
        })
        .collect()
}

/// The file names `sync_bridge_rn.sh` leaves out of the copy, less the two
/// that are not bridge sources.
fn flutter_sync_excludes(script: &str) -> std::collections::BTreeSet<String> {
    script
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("--exclude ")?;
            Some(rest.trim_end_matches('\\').trim().to_string())
        })
        .filter(|name| name != "tests/" && name != "Package.swift")
        .collect()
}

// The iOS compile surface is `BridgeRN/**/*.swift` less `rn_only_swift`, and
// BridgeRN is written by sync_bridge_rn.sh, which leaves out its own list. The
// two lists say the same thing in two languages, so they must agree, and each
// name must be a source that exists, for the reason the Android guard gives.
#[test]
fn flutter_ios_exclude_lists_agree_and_name_real_files() {
    let ios = flutter_plugin_dir().join("ios");
    let podspec = read_text(&ios.join("offline_protocol.podspec"));
    let script = read_text(&ios.join("sync_bridge_rn.sh"));
    let from_podspec = flutter_podspec_excludes(&podspec);
    let from_script = flutter_sync_excludes(&script);
    assert!(
        from_podspec.len() >= 2,
        "found only {} entries in rn_only_swift; the parser is broken: {from_podspec:?}",
        from_podspec.len()
    );
    assert_eq!(
        from_podspec, from_script,
        "offline_protocol.podspec rn_only_swift and sync_bridge_rn.sh --exclude must list \
         the same React-only sources"
    );
    let rn_ios = rn_dir().join("ios");
    for name in &from_podspec {
        assert!(
            rn_ios.join(name).is_file(),
            "the Flutter podspec excludes `{name}`, which does not exist under \
             bindings/react-native/ios: carry the rename over"
        );
    }
    for name in FLUTTER_SHARED_SOURCES {
        assert!(
            !from_podspec.contains(&format!("{name}.swift")),
            "the Flutter podspec must not exclude the shared source `{name}.swift`"
        );
    }
    assert!(
        podspec.contains("'BridgeRN/**/*.swift'"),
        "the Flutter podspec must take every shared source by glob (ADR 0025 decision 3)"
    );
}

// A version, an SDK level or a deployment target written down in the plugin is
// right on the day it is made and drifts with nothing failing (ADR 0025
// decision 5). The podspec and the Gradle build read them from the React
// Native package; the pubspec cannot, so the release gate checks it.
#[test]
fn flutter_native_builds_read_what_the_react_native_package_declares() {
    let plugin = flutter_plugin_dir();
    let podspec = read_text(&plugin.join("ios/offline_protocol.podspec"));
    let gradle = read_text(&plugin.join("android/build.gradle"));
    for needle in [
        "s.version          = rn_package['version']",
        "s.platform         = :ios, ios_deployment_target",
    ] {
        assert!(
            podspec.contains(needle),
            "offline_protocol.podspec must read `{needle}` from the React Native package"
        );
    }
    for needle in [
        "version rnPackage.version",
        "compileSdk sdkLevel('compileSdk')",
        "minSdkVersion sdkLevel('minSdkVersion')",
        "moduleDependency('net.java.dev.jna:jna')",
        "moduleDependency('androidx.security:security-crypto')",
        "moduleDependency('com.squareup.okhttp3:okhttp')",
    ] {
        assert!(
            gradle.contains(needle),
            "Flutter android/build.gradle must read `{needle}` from the React Native module"
        );
    }
    let release = read_text(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.github/workflows/release.yml"),
    );
    assert!(
        release.contains("bindings/flutter/offline_protocol/pubspec.yaml"),
        "release.yml must gate the Flutter pubspec version against the tag"
    );
}

#[test]
fn flutter_bridges_doc_is_linked_from_the_bridge_index() {
    let readme = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/bridges/README.md"),
    )
    .unwrap_or_else(|e| panic!("cannot read docs/bridges/README.md: {e}"));
    let flutter_doc =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/bridges/flutter.md");
    assert!(
        flutter_doc.is_file(),
        "docs/bridges/flutter.md must exist: it is the Flutter bridge contract"
    );
    assert!(
        readme.contains("flutter.md"),
        "docs/bridges/README.md must link docs/bridges/flutter.md"
    );
    let body = std::fs::read_to_string(&flutter_doc)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", flutter_doc.display()));
    for marker in ["F2", "F3", "F4", "measure-plugin-aar-size.sh"] {
        assert!(
            body.contains(marker),
            "docs/bridges/flutter.md must document `{marker}`"
        );
    }
}

#[test]
fn flutter_binding_ci_job_is_wired_in_workflow() {
    let ci = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/ci.yml"),
    )
    .unwrap_or_else(|e| panic!("cannot read .github/workflows/ci.yml: {e}"));
    for needle in [
        "flutter-binding:",
        "flutter_dart:",
        "flutter_native:",
        "flutter_lock:",
        "Decide Flutter native",
        "flutter_native_gate",
        "flutter_bridge_guards.rs",
        "bindings/flutter/scripts/ci-build-android-jni-arm64.sh",
        "measure-plugin-aar-size.sh --check",
        "bindings/flutter/offline_protocol",
        "bindings/flutter/offline_protocol/android/**",
        "gradle/actions/wrapper-validation@",
        "bindings/react-native/android/**",
        "Cargo.lock",
        "dorny/paths-filter@",
        "flutter-version:",
    ] {
        assert!(
            ci.contains(needle),
            ".github/workflows/ci.yml Flutter Binding job must reference `{needle}`"
        );
    }
}

#[test]
fn flutter_aar_baseline_json_is_well_formed() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../bindings/flutter/baselines/offline-protocol-release-aar.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let value: serde_json::Value =
        serde_json::from_str(&raw).expect("offline-protocol-release-aar.json must be JSON");
    let obj = value.as_object().expect("baseline must be a JSON object");
    for key in ["artifact", "bytes", "tolerance_bytes"] {
        assert!(
            obj.contains_key(key),
            "baseline missing `{key}`; see bindings/flutter/baselines/README.md"
        );
    }
    let bytes = obj["bytes"]
        .as_u64()
        .expect("bytes must be a positive integer");
    assert!(bytes > 0, "baseline bytes must be > 0");
    let tol = obj["tolerance_bytes"]
        .as_u64()
        .expect("tolerance_bytes must be a non-negative integer");
    assert!(
        tol > 0,
        "tolerance_bytes must be > 0 for regression headroom"
    );
}

#[test]
fn flutter_method_channel_methods_match_native_plugins() {
    let flutter = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../bindings/flutter/offline_protocol");
    let dart = std::fs::read_to_string(flutter.join("lib/src/offline_protocol.dart"))
        .unwrap_or_else(|e| panic!("cannot read offline_protocol.dart: {e}"));
    let kt = std::fs::read_to_string(
        flutter
            .join("android/src/main/kotlin/com/offlineprotocol/flutter/OfflineProtocolPlugin.kt"),
    )
    .unwrap_or_else(|e| panic!("cannot read OfflineProtocolPlugin.kt: {e}"));
    let swift = std::fs::read_to_string(flutter.join("ios/Classes/OfflineProtocolPlugin.swift"))
        .unwrap_or_else(|e| panic!("cannot read OfflineProtocolPlugin.swift: {e}"));

    let dart_methods = flutter_dart_method_channel_methods(&dart);
    assert!(
        !dart_methods.is_empty(),
        "nativeMethodChannelMethods must list at least one method"
    );
    let dart_set: std::collections::BTreeSet<_> = dart_methods.iter().cloned().collect();
    assert_eq!(
        dart_methods.len(),
        dart_set.len(),
        "nativeMethodChannelMethods must not contain duplicates: {dart_methods:?}"
    );

    let kotlin = flutter_kotlin_method_channel_methods(&kt);
    let swift_methods = flutter_swift_method_channel_methods(&swift);

    assert_eq!(
        kotlin, dart_set,
        "OfflineProtocolPlugin.kt when (call.method) must match Dart nativeMethodChannelMethods"
    );
    assert_eq!(
        swift_methods, dart_set,
        "OfflineProtocolPlugin.swift switch call.method must match Dart nativeMethodChannelMethods"
    );
}
