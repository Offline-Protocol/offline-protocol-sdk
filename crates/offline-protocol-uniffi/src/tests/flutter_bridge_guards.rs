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

// Flutter PR2 Android include list and iOS podspec must both ship the
// Internet stack (including PresenceWatchPolicy, which InternetManager uses).
#[test]
fn flutter_pr2_compile_lists_include_internet_stack() {
    let flutter = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../bindings/flutter/offline_protocol");
    let gradle = std::fs::read_to_string(flutter.join("android/build.gradle"))
        .unwrap_or_else(|e| panic!("cannot read flutter android/build.gradle: {e}"));
    let podspec = std::fs::read_to_string(flutter.join("ios/offline_protocol.podspec"))
        .unwrap_or_else(|e| panic!("cannot read offline_protocol.podspec: {e}"));
    for name in [
        "InternetTransportBootstrap",
        "InternetManager",
        "PresenceWatchPolicy",
    ] {
        assert!(
            gradle.contains(name),
            "bindings/flutter/offline_protocol/android/build.gradle must list `{name}`"
        );
        assert!(
            podspec.contains(name),
            "bindings/flutter/offline_protocol/ios/offline_protocol.podspec must list `{name}`"
        );
    }
}

// Flutter PR3 Android include list and iOS podspec must ship replay + transports.
#[test]
fn flutter_pr3_compile_lists_include_replay_and_transports() {
    let flutter = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../bindings/flutter/offline_protocol");
    let gradle = std::fs::read_to_string(flutter.join("android/build.gradle"))
        .unwrap_or_else(|e| panic!("cannot read flutter android/build.gradle: {e}"));
    let podspec = std::fs::read_to_string(flutter.join("ios/offline_protocol.podspec"))
        .unwrap_or_else(|e| panic!("cannot read offline_protocol.podspec: {e}"));
    assert!(
        gradle.contains("pr3KotlinIncludes"),
        "flutter android/build.gradle must define pr3KotlinIncludes"
    );
    assert!(
        podspec.contains("pr3_bridge_swift"),
        "flutter offline_protocol.podspec must define pr3_bridge_swift"
    );
    for name in [
        "EventReplayRing",
        "ForegroundReconnectPolicy",
        "BleTransportFacade",
        "WifiDirectManager",
        "BleManager",
    ] {
        assert!(
            gradle.contains(name) || gradle.contains("ble/**/*.kt"),
            "bindings/flutter/offline_protocol/android/build.gradle must list PR3 transport `{name}`"
        );
        assert!(
            podspec.contains(name) || podspec.contains("ble/**/*.swift"),
            "bindings/flutter/offline_protocol/ios/offline_protocol.podspec must list PR3 transport `{name}`"
        );
    }
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
        "docs/bridges/flutter.md must exist — Flutter bridge contract (PR4)"
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
fn flutter_pr3_aar_baseline_json_is_well_formed() {
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
            "baseline missing `{key}` — see bindings/flutter/baselines/README.md"
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
fn flutter_pr3_method_channel_methods_match_native_plugins() {
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
