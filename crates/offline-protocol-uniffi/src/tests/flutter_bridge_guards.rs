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

fn flutter_method_channel_manifest() -> Vec<String> {
    let path = flutter_plugin_dir().join("method_channel_methods.txt");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Method names dispatched in `FlutterEngineChannel.kt` (`"method" ->`).
fn flutter_kotlin_method_channel_methods(kt: &str) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    for line in kt.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let Some((method, tail)) = rest.split_once('"') else {
            continue;
        };
        if tail.trim_start().starts_with("->") && !method.is_empty() {
            names.insert(method.to_string());
        }
    }
    names
}

/// Method names dispatched in `FlutterEngineChannel.swift` (`case "method":`).
fn flutter_swift_method_channel_methods(swift: &str) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    let mut rest = swift;
    while let Some(idx) = rest.find("case \"") {
        rest = &rest[idx + 6..];
        let Some(end) = rest.find('"') else {
            break;
        };
        let method = &rest[..end];
        if !method.is_empty() && !method.contains(',') {
            names.insert(method.to_string());
        }
        rest = &rest[end + 1..];
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
    "InboundEventBuffer",
    "ForegroundReconnectPolicy",
    "WifiDirectManager",
    "ProtocolConfigParser",
    "InitialRuntimeConfigApplier",
    "ProtocolBackgroundProcessor",
    "ProtocolEngineHost",
    "ProtocolEngineHostBridge",
    "TransportBridge",
    "ReticulumManager",
    "NostrManager",
    "NostrQueryTracker",
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
// decision 5). The podspec and Gradle read them from the React Native package
// in the monorepo, or from pub_bridge_version.json when the pub assemble ships
// a vendored bridge; release gates the pubspec version.
#[test]
fn flutter_native_builds_read_what_the_react_native_package_declares() {
    let plugin = flutter_plugin_dir();
    let podspec = read_text(&plugin.join("ios/offline_protocol.podspec"));
    let gradle = read_text(&plugin.join("android/build.gradle"));
    for needle in [
        "s.version          = package_version",
        "s.platform         = :ios, ios_deployment_target",
        "pub_bridge_version.json",
        "MeshSdk.podspec",
    ] {
        assert!(
            podspec.contains(needle),
            "offline_protocol.podspec must pin `{needle}` from RN or pub metadata"
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

/// The text of `kt` from the companion object onwards, with `//` comments
/// dropped, so a declaration counts only where it is code.
fn kotlin_companion_code(kt: &str) -> String {
    kt.split("companion object {")
        .nth(1)
        .unwrap_or_else(|| panic!("companion object not found in OfflineProtocolPlugin.kt"))
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Flutter creates a new plugin instance for every engine. With an engine
// thread per instance, a new engine's `create` runs while the previous one's
// queued teardown is still blocked on transport I/O: two cores under one
// identity. One thread per process orders every call behind that teardown
// (docs/bridges/flutter.md, F2).
#[test]
fn flutter_engine_thread_is_shared_by_every_plugin_instance() {
    let plugin = flutter_plugin_dir();
    let kt = read_text(
        &plugin
            .join("android/src/main/kotlin/com/offlineprotocol/flutter/OfflineProtocolPlugin.kt"),
    );
    let (instance, _) = kt
        .split_once("companion object {")
        .unwrap_or_else(|| panic!("companion object not found in OfflineProtocolPlugin.kt"));
    assert!(
        !instance.contains("val engineExecutor"),
        "OfflineProtocolPlugin.kt declares engineExecutor per instance; it must live in \
         the companion object, one per process"
    );
    assert!(
        kotlin_companion_code(&kt).contains("val engineExecutor"),
        "OfflineProtocolPlugin.kt must declare engineExecutor in its companion object"
    );

    let swift = read_text(&plugin.join("ios/Classes/OfflineProtocolPlugin.swift"));
    let declarations: Vec<&str> = swift
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//") && line.contains("engineQueue = DispatchQueue("))
        .collect();
    assert_eq!(
        declarations,
        ["private static let engineQueue = DispatchQueue("],
        "OfflineProtocolPlugin.swift must declare engineQueue once, as a static let"
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
    for needle in [
        "flutter-binding-ios:",
        "ci-typecheck-flutter-ios-bridge.sh",
        "flutter_ios:",
    ] {
        assert!(
            ci.contains(needle),
            ".github/workflows/ci.yml must wire Flutter iOS bridge CI (`{needle}`)"
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

/// The bridge chapter, not the changelog: CHANGELOG.md holds the current
/// release only and is archived at every cut, so a guard on it fails the
/// release after this one.
#[test]
fn flutter_docs_describe_ios_bridge_ci() {
    let bridge_doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/bridges/flutter.md"),
    )
    .unwrap_or_else(|e| panic!("cannot read docs/bridges/flutter.md: {e}"));
    assert!(
        !bridge_doc.contains("iOS pod compilation is not gated in CI yet"),
        "docs/bridges/flutter.md must describe Flutter Binding (iOS bridge), not claim iOS is unchecked"
    );
    assert!(
        bridge_doc.contains("Flutter Binding (iOS bridge)")
            && bridge_doc.contains("ci-typecheck-flutter-ios-bridge.sh"),
        "docs/bridges/flutter.md must name the iOS CI job and its script"
    );
}

#[test]
fn flutter_android_hosts_mesh_foreground_keep_alive() {
    let flutter = flutter_plugin_dir();
    let gradle = std::fs::read_to_string(flutter.join("android/build.gradle"))
        .unwrap_or_else(|e| panic!("cannot read android/build.gradle: {e}"));
    assert!(
        !gradle.contains("MeshForegroundService.kt"),
        "Flutter must compile MeshForegroundService, not exclude it"
    );
    let plugin_kt = std::fs::read_to_string(
        flutter
            .join("android/src/main/kotlin/com/offlineprotocol/flutter/OfflineProtocolPlugin.kt"),
    )
    .unwrap_or_else(|e| panic!("cannot read OfflineProtocolPlugin.kt: {e}"));
    assert!(
        plugin_kt.contains("FlutterMeshForegroundKeepAlive.start")
            && plugin_kt.contains("FlutterMeshForegroundKeepAlive.stop"),
        "Flutter start/stop must drive the shared mesh foreground service"
    );
    // Destroy, detach and re-create stop the service after every transport,
    // as stop() and React Native do: while the sockets close it is what keeps
    // a backgrounded process out of the cached state.
    let teardown = plugin_kt
        .split("private fun tearDownProtocol(")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .unwrap_or_else(|| panic!("OfflineProtocolPlugin.kt must define tearDownProtocol"));
    let service = teardown
        .find("FlutterMeshForegroundKeepAlive.stop")
        .unwrap_or_else(|| panic!("tearDownProtocol must stop the mesh foreground service"));
    let last_transport = teardown
        .find("nostrManager?.stop()")
        .unwrap_or_else(|| panic!("tearDownProtocol must stop the Nostr manager"));
    assert!(
        last_transport < service,
        "tearDownProtocol must stop the foreground service after the transports"
    );
    assert!(
        plugin_kt.contains("StickyEventDispatcher")
            && plugin_kt.contains("sendStickyOneShotEvent")
            && plugin_kt.contains("mesh_stopped_by_user")
            && plugin_kt.contains("SupersededLatchPolicy.EVENT_TYPE"),
        "Flutter must sticky-deliver one-shot events like React Native"
    );
    assert!(
        plugin_kt.contains("holdInboundEventIfBuffered")
            && plugin_kt.contains("INBOUND_EVENT_BUFFER_CAPACITY = 256")
            && plugin_kt.contains("message_received")
            && plugin_kt.contains("inboundEvents.flush()"),
        "Flutter Android must buffer inbound core events like React Native"
    );
    assert!(
        std::fs::read_to_string(
            flutter.join(
                "android/src/main/kotlin/com/offlineprotocol/flutter/FlutterEngineChannel.kt"
            ),
        )
        .unwrap_or_else(|e| panic!("cannot read FlutterEngineChannel.kt: {e}"))
        .contains("discardOneShotEvent"),
        "Flutter enableTransport must discard held internet_session_superseded"
    );
    assert!(
        !gradle.contains("StickyEventDispatcher.kt"),
        "Flutter must compile StickyEventDispatcher for one-shot redelivery"
    );
    let manifest = std::fs::read_to_string(flutter.join("android/src/main/AndroidManifest.xml"))
        .unwrap_or_else(|e| panic!("cannot read plugin AndroidManifest.xml: {e}"));
    assert!(
        manifest.contains("MeshForegroundService"),
        "Flutter plugin manifest must declare MeshForegroundService"
    );
    let flutter_ios_plugin =
        std::fs::read_to_string(flutter.join("ios/Classes/OfflineProtocolPlugin.swift"))
            .unwrap_or_else(|e| panic!("cannot read OfflineProtocolPlugin.swift: {e}"));
    assert!(
        flutter_ios_plugin.contains("restateInternetSupersededIfNeeded()")
            && flutter_ios_plugin.contains("supersedeRestatementJson()"),
        "Flutter iOS must restate internet_session_superseded on foreground like React Native"
    );
    assert!(
        flutter_ios_plugin.contains("holdInboundEventIfBuffered")
            && flutter_ios_plugin.contains("flushInboundEvents()")
            && flutter_ios_plugin.contains("InboundEventBuffer()"),
        "Flutter iOS must buffer inbound core events like React Native"
    );
    assert!(
        !std::fs::read_to_string(flutter.join("ios/offline_protocol.podspec"))
            .unwrap_or_default()
            .contains("InboundEventBuffer.swift"),
        "Flutter must compile InboundEventBuffer.swift, not exclude it"
    );
    let ci_script = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../bindings/flutter/scripts/ci-typecheck-flutter-ios-bridge.sh"),
    )
    .unwrap_or_else(|e| panic!("cannot read ci-typecheck-flutter-ios-bridge.sh: {e}"));
    assert!(
        !ci_script.contains("ReticulumManager.swift") && !ci_script.contains("NostrManager.swift"),
        "Flutter iOS typecheck must compile Reticulum/Nostr managers with ProtocolEngineHost"
    );
}

#[test]
fn flutter_method_channel_methods_match_native_plugins() {
    let flutter = flutter_plugin_dir();
    let manifest: std::collections::BTreeSet<String> =
        flutter_method_channel_manifest().into_iter().collect();
    assert!(
        manifest.len() >= 170,
        "method_channel_methods.txt must list the full channel surface"
    );

    let dart = std::fs::read_to_string(flutter.join("lib/src/offline_protocol.dart"))
        .unwrap_or_else(|e| panic!("cannot read offline_protocol.dart: {e}"));
    let dart_methods = flutter_dart_method_channel_methods(&dart);
    let dart_set: std::collections::BTreeSet<_> = dart_methods.iter().cloned().collect();
    assert_eq!(
        dart_set, manifest,
        "Dart nativeMethodChannelMethods must match method_channel_methods.txt"
    );

    let kt = std::fs::read_to_string(
        flutter.join("android/src/main/kotlin/com/offlineprotocol/flutter/FlutterEngineChannel.kt"),
    )
    .unwrap_or_else(|e| panic!("cannot read FlutterEngineChannel.kt: {e}"));
    let swift = std::fs::read_to_string(flutter.join("ios/Classes/FlutterEngineChannel.swift"))
        .unwrap_or_else(|e| panic!("cannot read FlutterEngineChannel.swift: {e}"));

    let kotlin = flutter_kotlin_method_channel_methods(&kt);
    let swift_methods = flutter_swift_method_channel_methods(&swift);

    assert_eq!(
        kotlin, manifest,
        "FlutterEngineChannel.kt dispatch must match method_channel_methods.txt"
    );
    assert_eq!(
        swift_methods, manifest,
        "FlutterEngineChannel.swift dispatch must match method_channel_methods.txt"
    );
}

/// The plugin prefers native libraries a release stages into it, and falls
/// back to the React Native build otherwise. The choice must test for the
/// library, never for its directory: the libraries are ignored repository-wide,
/// so a framework holding only an `Info.plist`, or an ABI directory with no
/// library in it, passes a directory test and the fallback never runs. The iOS
/// app then fails at the first UniFFI symbol; the Android AAR ships with no
/// native code.
#[test]
fn flutter_staged_native_artifacts_are_chosen_by_the_library_not_the_directory() {
    let flutter = flutter_plugin_dir();
    let podspec = std::fs::read_to_string(flutter.join("ios/offline_protocol.podspec"))
        .unwrap_or_else(|e| panic!("cannot read offline_protocol.podspec: {e}"));
    assert!(
        podspec.contains("ios-arm64/liboffline_protocol_uniffi.a")
            && podspec.contains("File.file?(staged_lib) ? staged_xcf : rn_xcf"),
        "offline_protocol.podspec must vendor the staged XCFramework only when its device \
         library is present"
    );
    assert!(
        !podspec.contains("File.directory?(staged_xcf)"),
        "offline_protocol.podspec must not choose the staged XCFramework by its directory"
    );
    let gradle = std::fs::read_to_string(flutter.join("android/build.gradle"))
        .unwrap_or_else(|e| panic!("cannot read android/build.gradle: {e}"));
    assert!(
        gradle.contains("it.name == 'libuniffi_offline_protocol.so'")
            && gradle.contains("jniLibs.srcDirs(stagedHasLibrary ? stagedJni : rnJni)"),
        "android/build.gradle must use the staged jniLibs only when a library is present"
    );
    assert!(
        !gradle.contains("stagedJni.list()"),
        "android/build.gradle must not choose the staged jniLibs by listing the directory"
    );
    let ignore = read_text(&flutter.join("ios/.gitignore"));
    assert!(
        ignore.lines().any(|l| l.trim() == "Frameworks/"),
        "ios/.gitignore must keep the staged XCFramework out of the repository"
    );
    // In the monorepo every `pod install` copies the shared bridge again, so an
    // edit to a shared source reaches the pod. Only the pub tree, which ships
    // BridgeRN and has no React Native package to copy from, skips the copy.
    // Keyed on BridgeRN existing instead, the first sync would be the last.
    assert!(
        podspec.contains("unless File.file?(pub_meta_path)\n    sync_script"),
        "offline_protocol.podspec must skip prepare_command only in the pub layout"
    );
    assert!(
        !podspec.contains("bridge_generated"),
        "offline_protocol.podspec must not skip prepare_command because BridgeRN exists"
    );
}

/// The body of one job in a workflow file: from its key to the next line at
/// job indentation (two spaces), which is the next job's key or the comment
/// above it. A comment inside a job is indented further and stays in.
fn workflow_job<'a>(workflow: &'a str, file: &str, name: &str) -> &'a str {
    let header = format!("\n  {name}:\n");
    let start = workflow
        .find(&header)
        .unwrap_or_else(|| panic!("{file} has no `{name}` job"))
        + 1;
    let body = start + header.len() - 1;
    let end = workflow[body..]
        .match_indices('\n')
        .map(|(i, _)| body + i + 1)
        .find(|&line| {
            let rest = &workflow[line..];
            rest.starts_with("  ") && !rest[2..].starts_with([' ', '\n'])
        })
        .unwrap_or(workflow.len());
    &workflow[start..end]
}

/// pub.dev packaging is scripted and documented; no ad-hoc copies.
#[test]
fn flutter_pub_distribution_path_is_documented_and_pinned() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root");
    let assemble = read_text(&repo.join("scripts/assemble-flutter-pub-package.sh"));
    assert!(
        assemble.contains("pub.dev"),
        "assemble-flutter-pub-package.sh must describe the pub.dev tree"
    );
    assert!(
        assemble.contains("bridge-rn"),
        "assemble script must vendor the Android bridge for pub consumers"
    );
    assert!(
        assemble.contains("ios/pub_bridge_version.json"),
        "assemble script must write iOS pub_bridge_version.json for pub consumers"
    );
    assert!(
        assemble.contains("ios_deployment_target"),
        "assemble script must pin ios_deployment_target from MeshSdk.podspec"
    );
    // Pub hides files through ignore rules no layout check sees: the
    // repository's .gitignore when the tree sits inside a git work tree, and
    // any .gitignore shipped in the tree (the plugin's own ios/.gitignore hides
    // BridgeRN and Frameworks). The script refuses the first and deletes the
    // second; the fixture test's dry-run proves the archive.
    assert!(
        assemble.contains("inside a git work tree")
            && assemble.contains("-name .gitignore -type f -delete"),
        "assemble-flutter-pub-package.sh must refuse a git work tree and ship no .gitignore"
    );
    assert!(
        assemble.contains("sync_bridge_rn.sh") && assemble.contains("ios-deployment-target.sh"),
        "assemble-flutter-pub-package.sh must reuse sync_bridge_rn.sh and the one \
         deployment-target parser rather than a copy of either"
    );
    // A working tree holds what builds and IDEs leave in it, hidden only by
    // the .gitignore files the package cannot ship. A pub.dev version is
    // permanent, so the package is the tracked files plus the staged natives.
    assert!(
        assemble.contains("ls-files -z --cached")
            && !assemble.contains("\"$PLUGIN/\" \"$OUTPUT/\""),
        "assemble-flutter-pub-package.sh must copy tracked files, never the plugin directory"
    );
    // A maintainer's jniLibs can hold a library from an older build under
    // another name. The package takes each library by name and refuses any
    // native file it does not expect, so nothing stale reaches a permanent
    // pub.dev version, and both XCFramework slices must be there.
    assert!(
        assemble.contains("unexpected native file")
            && assemble.contains("ios-arm64_x86_64-simulator"),
        "assemble-flutter-pub-package.sh must refuse unexpected native files and require \
         the simulator slice"
    );
    let fixture = read_text(&repo.join("scripts/tests/test-assemble-flutter-pub-package.sh"));
    assert!(
        fixture.contains("a stale library beside the real one stays out of the package")
            && fixture.contains("stray native file refused")
            && fixture.contains("missing simulator slice refused"),
        "the fixture test must prove stale and stray native files stay out of the package"
    );
    assert!(
        fixture.contains("flutter pub publish --dry-run"),
        "the fixture test must dry-run publish the assembled tree"
    );
    assert!(
        fixture.contains("untracked files in the source tree stay out of the package"),
        "the fixture test must prove untracked files stay out of the package"
    );
    let verify = read_text(&repo.join("scripts/verify-flutter-pub-podspec.sh"));
    assert!(
        verify.contains("pod ipc spec"),
        "verify-flutter-pub-podspec.sh must evaluate the podspec via pod ipc spec"
    );
    let release = read_text(&repo.join(".github/workflows/release.yml"));
    assert!(
        release.contains("verify-flutter-pub-podspec.sh"),
        "release.yml must verify the Flutter pub podspec after assemble"
    );
    let job = |name: &str| workflow_job(&release, "release.yml", name);
    // `gem install` into the runner's system Ruby puts `pod` in a user
    // directory that is not on PATH; setup-ruby puts its gem bin on PATH.
    let ci = read_text(&repo.join(".github/workflows/ci.yml"));
    for (file, body) in [
        (
            "ci.yml native-package-scripts",
            workflow_job(&ci, "ci.yml", "native-package-scripts"),
        ),
        (
            "release.yml flutter-pub-package",
            job("flutter-pub-package"),
        ),
    ] {
        let ruby = body.find("ruby/setup-ruby@");
        let pods = body.find("gem install cocoapods");
        assert!(
            matches!((ruby, pods), (Some(r), Some(p)) if r < p),
            "{file} must set up Ruby before installing CocoaPods, or `pod` is not on PATH"
        );
    }
    let assemble_job = job("flutter-pub-package");
    assert!(
        assemble_job.contains("$RUNNER_TEMP/flutter-pub-package")
            && assemble_job.contains("include-hidden-files: true"),
        "flutter-pub-package must assemble outside the checkout and upload the tree with \
         its .pubignore"
    );
    // The package is a release artifact like the Swift package and the
    // Android library: a tree that fails to assemble must stop npm and
    // crates.io, or a version ships that pub.dev can never carry.
    for gate in ["release", "publish-crates"] {
        let needs = job(gate)
            .lines()
            .find(|l| l.trim_start().starts_with("needs: ["))
            .unwrap_or_else(|| panic!("release.yml `{gate}` has no one-line needs list"));
        assert!(
            needs.contains(" flutter-pub-package,") || needs.contains(" flutter-pub-package]"),
            "release.yml `{gate}` must need flutter-pub-package"
        );
    }
    // Both platforms ship bindings generated with this release's libraries:
    // the iOS ones arrive with ios-libs, the Kotlin ones from android-bindings.
    assert!(
        assemble_job.contains("android-bindings]")
            && assemble_job.contains("name: android-uniffi-bindings"),
        "flutter-pub-package must ship the Kotlin bindings android-bindings generated"
    );
    let publish_job = job("publish-flutter-pub");
    assert!(
        publish_job.contains("id-token: write")
            && publish_job.contains("audience=https%3A%2F%2Fpub.dev")
            && !publish_job.contains("secrets."),
        "publish-flutter-pub must publish through pub.dev automated publishing (OIDC), \
         with no stored token"
    );
    let doc = std::fs::read_to_string(repo.join("docs/flutter-distribution.md"))
        .unwrap_or_else(|e| panic!("docs/flutter-distribution.md: {e}"));
    assert!(
        doc.contains("flutter pub add offline_protocol"),
        "flutter-distribution.md must name the pub.dev install command"
    );
    let gradle = read_text(&flutter_plugin_dir().join("android/build.gradle"));
    assert!(
        gradle.contains("file('bridge-rn')"),
        "Flutter android/build.gradle must support the pub assemble layout"
    );
    let podspec = read_text(&flutter_plugin_dir().join("ios/offline_protocol.podspec"));
    assert!(
        podspec.contains("pub_bridge_version.json"),
        "offline_protocol.podspec must read pub_bridge_version.json in the pub layout"
    );
    assert!(
        podspec.contains("ios_deployment_target"),
        "offline_protocol.podspec must read ios_deployment_target from pub_bridge_version.json"
    );
}
