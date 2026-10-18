//! Which profile each shipped library is built with (ADR 0029).
//!
//! The iOS and Android libraries are built with `mobile`, which is `release`
//! optimised for size and still unwinding. Nothing in a build fails when a
//! command keeps `--release` or a profile gains `panic = "abort"`: the first
//! ships a library twice the size, the second turns every `catch_unwind` in
//! the SDK into the end of the host application's process. Until 0.29.0 the
//! documents said the mobile library aborted on a panic while every build
//! used `release`, and nothing caught either half.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The `key = value` lines of one `[section]` of a manifest, as written.
fn section(text: &str, name: &str) -> Option<Vec<(String, String)>> {
    let header = format!("[{name}]");
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if line == header {
                found = Some(Vec::new());
            } else if found.is_some() {
                break;
            }
            continue;
        }
        if let (Some(keys), Some((k, v))) = (found.as_mut(), line.split_once('=')) {
            if !line.starts_with('#') {
                keys.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
    }
    found
}

/// Shell commands with their `\` continuations joined.
fn commands(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        match line.strip_suffix('\\') {
            Some(head) => {
                current.push_str(head);
                current.push(' ');
            }
            None => {
                current.push_str(line);
                out.push(std::mem::take(&mut current));
            }
        }
    }
    out
}

fn builds_of_the_library(text: &str) -> Vec<String> {
    commands(text)
        .into_iter()
        .filter(|c| c.contains("cargo build") && c.contains("offline-protocol-uniffi"))
        .collect()
}

#[test]
fn the_mobile_profile_optimises_for_size_and_unwinds() {
    let manifest = read("Cargo.toml");
    let mobile = section(&manifest, "profile.mobile").expect("the workspace defines `mobile`");
    let get = |key: &str| {
        mobile
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("inherits"), Some("\"release\""));
    assert_eq!(get("opt-level"), Some("\"s\""));
    assert!(
        matches!(get("panic"), None | Some("\"unwind\"")),
        "`panic = \"abort\"` in the mobile profile ends the host app's process on any \
         panic: the event handler isolation, the telemetry sink dispatch, the document \
         import and UniFFI's panic-to-exception conversion are all `catch_unwind`"
    );
    let release = section(&manifest, "profile.release").expect("the workspace defines `release`");
    assert!(
        release.iter().any(|(k, v)| k == "lto" && v == "true"),
        "`mobile` inherits its link-time optimisation from `release`"
    );
    assert!(
        section(&manifest, "profile.minisize").is_none(),
        "`minisize` was never used by a build, and its `panic = \"abort\"` is what the \
         documents wrongly said shipped; the mobile libraries use `mobile`"
    );
}

#[test]
fn the_mobile_libraries_are_built_with_the_mobile_profile() {
    for path in [
        "bindings/react-native/scripts/build-uniffi-ios.sh",
        "bindings/react-native/scripts/build-uniffi-android.sh",
    ] {
        let builds = builds_of_the_library(&read(path));
        assert!(!builds.is_empty(), "{path} no longer builds the library");
        for build in builds {
            assert!(
                build.contains("--profile mobile"),
                "{path}: a mobile library is built with `--profile mobile`: {build}"
            );
        }
    }

    // The release workflow builds iOS through the script above, Android
    // itself (the build without the C binding), and the desktop libraries.
    let release = read(".github/workflows/release.yml");
    assert!(release.contains("run: bash scripts/build-uniffi-ios.sh"));
    let builds = builds_of_the_library(&release);
    let (android, desktop): (Vec<_>, Vec<_>) = builds
        .into_iter()
        .partition(|b| b.contains("--no-default-features"));
    assert_eq!(android.len(), 1, "one Android build: {android:?}");
    assert!(
        android[0].contains("--profile mobile"),
        "the Android release build uses `--profile mobile`: {}",
        android[0]
    );
    assert!(desktop.len() >= 3, "the desktop builds: {desktop:?}");
    for build in desktop {
        assert!(
            build.contains("--release"),
            "the desktop libraries and the wheels are built with `release`, for \
             speed over size (ADR 0029): {build}"
        );
    }

    // CI's one iOS build on a pull request, for the Swift package job.
    let ci = read(".github/workflows/ci.yml");
    let simulator = ci
        .find("TARGET=aarch64-apple-ios-sim")
        .expect("the Swift package job builds a simulator slice");
    let build = builds_of_the_library(&ci[simulator..])
        .into_iter()
        .next()
        .expect("a build follows the target choice");
    assert!(
        build.contains("--profile mobile"),
        "CI builds the iOS library the release ships: {build}"
    );
}
