//! C# binding guards: the contract doc, the hand-mirrored status enum, and the CI job.
//!
//! `cargo test` does not run the C# (C9). These hold the parts of the binding that
//! drift silently: a value the C# copies from Rust, and the job that does run it.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    // A Windows checkout may carry CRLF; the guards read lines, not bytes.
    std::fs::read_to_string(repo().join(rel))
        .unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
        .replace("\r\n", "\n")
}

/// `Name = N,` members of the enum whose declaration starts with `header`, in order.
fn enum_members(source: &str, header: &str, file: &str) -> Vec<(String, i64)> {
    let body = source
        .split_once(header)
        .unwrap_or_else(|| panic!("`{header}` not found in {file}"))
        .1;
    let body = body
        .split_once('{')
        .unwrap_or_else(|| panic!("`{header}` has no body in {file}"))
        .1
        .split_once('}')
        .unwrap_or_else(|| panic!("`{header}` body not terminated in {file}"))
        .0;
    body.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//") && !line.starts_with('['))
        .filter_map(|line| {
            let (name, value) = line.trim_end_matches(',').split_once('=')?;
            let value = value.trim().parse().ok()?;
            Some((name.trim().to_string(), value))
        })
        .collect()
}

#[test]
fn csharp_bridges_doc_is_linked_from_the_bridge_index() {
    assert!(
        repo().join("docs/bridges/csharp.md").is_file(),
        "docs/bridges/csharp.md must exist: it is the C# bridge contract"
    );
    assert!(
        read("docs/bridges/README.md").contains("(csharp.md)"),
        "docs/bridges/README.md must link docs/bridges/csharp.md"
    );
}

// The C# enum is a copy of the C status values (C5). A host switches on the
// number, so a renumbered or missing member reads one status as another with
// no build failing.
#[test]
fn the_csharp_status_enum_is_the_c_apis() {
    const RUST: &str = "crates/offline-protocol-uniffi/src/capi/ffi.rs";
    const CSHARP: &str = "bindings/csharp/src/OfflineProtocol.Sdk/Native/OfflineProtocolStatus.cs";
    let rust = enum_members(&read(RUST), "pub enum OfflineProtocolStatus", RUST);
    let csharp = enum_members(&read(CSHARP), "public enum OfflineProtocolStatus", CSHARP);
    assert!(
        rust.len() >= 6,
        "parsed only {} members from {RUST}; the parser is broken, not the enum: {rust:?}",
        rust.len()
    );
    assert_eq!(
        csharp, rust,
        "{CSHARP} must list the C API's OfflineProtocolStatus members with the same values"
    );
}

// The C# is compiled and run only by its CI job (C9). A filter that misses
// the binding, or a job the aggregate check does not wait on, turns a broken
// binding green.
#[test]
fn the_csharp_binding_has_a_ci_job_the_result_waits_on() {
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.contains("'bindings/csharp/**'"),
        "ci.yml must have a change filter on bindings/csharp/**"
    );
    assert!(
        ci.contains("csharp: ${{ steps.gate.outputs.csharp }}"),
        "ci.yml must export the csharp gate output"
    );
    assert!(
        ci.contains("needs.changes.outputs.csharp == 'true'"),
        "ci.yml must have a job gated on the csharp filter"
    );
    assert!(
        ci.contains("      - csharp-binding\n"),
        "the CI Result job must list csharp-binding in its needs"
    );
    assert!(
        ci.contains("dotnet run --project bindings/csharp/consumer-check"),
        "the C# job must run the consumer check against the built library"
    );
}

#[test]
fn csharp_telemetry_config_omits_os_and_reads_the_section() {
    let telemetry =
        read("bindings/csharp/src/OfflineProtocol.Sdk/Telemetry/TelemetryConfigJson.cs");
    assert!(
        !telemetry.contains("\"os\"") && telemetry.contains("os_major"),
        "TelemetryConfigJson must fill os_major and never serialize os from the app"
    );
    for field in [
        "api_key",
        "app_id",
        "app_version",
        "debug",
        "flush_interval_ms",
        "max_batch_bytes",
        "max_buffered_records",
        "include_device_id",
        "scrub_ids",
        "mls_verbosity",
        "metrics_cadence_ms",
        "routing_diagnostic",
        "mls_sampling_bypass",
    ] {
        assert!(
            telemetry.contains(field),
            "TelemetryConfigJson must mention `{field}`"
        );
    }
    let platform = read("bindings/csharp/src/OfflineProtocol.Sdk/Telemetry/TelemetryPlatform.cs");
    assert!(
        platform.contains("OsMajor"),
        "TelemetryPlatform must supply os_major for C12"
    );
}

// Native callbacks run on the engine thread (CS1); the bridge must hand events
// to a thread of its own, behind a bound, and the engine must surface what the
// bridge drops and what the app's handler throws. Reads the public surface and
// the mechanism the contract doc names, not private field names.
#[test]
fn csharp_events_are_delivered_off_the_engine_thread() {
    let events = read("bindings/csharp/src/OfflineProtocol.Sdk/Native/EventCallbackBridge.cs");
    assert!(
        events.contains("Channel.CreateBounded") && events.contains("new Thread("),
        "EventCallbackBridge must queue on a bounded channel and deliver on its own thread"
    );
    let engine = read("bindings/csharp/src/OfflineProtocol.Sdk/ProtocolEngine.cs");
    for surface in ["DroppedEventDeliveryCount", "EventHandlerFailureCount"] {
        assert!(
            engine.contains(&format!("public long {surface}")),
            "ProtocolEngine must expose `{surface}` (docs/bridges/csharp.md, Events)"
        );
    }
}

/// Every `.cs` file under the C# SDK's source tree, recursively.
fn csharp_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()))
    {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            csharp_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "cs") {
            out.push(path);
        }
    }
}

// `GCHandle` is a struct. `Free()` on a `readonly` field frees a defensive copy,
// so the field's `IsAllocated` stays true and a second release path frees the
// handle again, which can release another object's handle. The event bridge
// shipped exactly that: the C API calls `drop` on a refused install and the
// managed side then released it too. Nothing in the compiler or the tests sees it.
#[test]
fn no_csharp_gchandle_is_held_in_a_readonly_field() {
    let mut files = Vec::new();
    csharp_sources(&repo().join("bindings/csharp/src"), &mut files);
    assert!(
        !files.is_empty(),
        "no C# sources found under bindings/csharp/src"
    );
    for file in files {
        let source = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        for (n, line) in source.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//") {
                continue;
            }
            assert!(
                !(line.contains("readonly") && line.contains("GCHandle ")),
                "{}:{}: a GCHandle field must not be readonly; free it once behind an \
                 Interlocked flag instead",
                file.display(),
                n + 1
            );
        }
    }
}

const GENERATED: &str = "bindings/csharp/src/OfflineProtocol.Sdk/Generated";

/// Names of the `public` members declared in `source`: the identifier before the first `(`.
fn public_member_names(source: &str) -> std::collections::BTreeSet<String> {
    source
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("public "))
        .filter_map(|line| {
            let head = line.split_once('(')?.0;
            if head.contains('=') {
                return None;
            }
            head.split_whitespace().last().map(str::to_string)
        })
        .collect()
}

fn generated_engine_sources() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = std::fs::read_dir(repo().join(GENERATED))
        .unwrap_or_else(|e| panic!("cannot read {GENERATED}: {e}"))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("ProtocolEngine.") && name.ends_with(".g.cs"))
        .map(|name| {
            let text = read(&format!("{GENERATED}/{name}"));
            (name, text)
        })
        .collect();
    files.sort();
    files
}

// A generated wrapper beside a hand-written method of the same name differs only in its
// parameters, so the compiler accepts both and a zero-argument call becomes ambiguous or
// binds to the wrapper. poll_event did this: the wrapper went through the JSON dispatcher
// and lost the null an empty queue returns. tools/gen_csharp_local_api.py refuses it too;
// this holds the committed files on every platform `cargo test` runs on.
#[test]
fn no_generated_csharp_method_shadows_a_hand_written_one() {
    let hand = public_member_names(&read(
        "bindings/csharp/src/OfflineProtocol.Sdk/ProtocolEngine.cs",
    ));
    assert!(
        hand.contains("PollEventAsync") && hand.contains("CreateAsync"),
        "the member scan must find the hand-written engine methods: {hand:?}"
    );
    let generated = generated_engine_sources();
    assert!(
        generated.len() >= 2,
        "{GENERATED} must hold the generated ProtocolEngine partials"
    );
    for (name, text) in generated {
        let clash: Vec<_> = public_member_names(&text)
            .intersection(&hand)
            .cloned()
            .collect();
        assert!(
            clash.is_empty(),
            "{name} shadows hand-written ProtocolEngine members {clash:?}; \
             add them to SKIP_DECLARATIONS in tools/gen_csharp_local_api.py"
        );
    }
}

// Every wire name the C# binding generates a wrapper for must be one the C API dispatches,
// or the wrapper compiles and fails at runtime with MethodNotFound.
#[test]
fn every_generated_csharp_method_is_dispatched_by_the_c_api() {
    let dispatch = read("crates/offline-protocol-uniffi/src/capi/generated.rs");
    let manifest = read("bindings/csharp/local_api_methods.txt");
    let names: Vec<&str> = manifest.lines().filter(|l| !l.is_empty()).collect();
    assert!(names.len() > 100, "local_api_methods.txt looks truncated");
    for wire in names {
        // A prefixed name is matched without its prefix, inside the object's own arm.
        let method = wire.rsplit('.').next().unwrap_or(wire);
        assert!(
            dispatch.contains(&format!("\"{method}\" =>")),
            "the C# binding wraps {wire}, which the C API does not dispatch"
        );
    }
}

// C6: the SDK ships no configuration values of its own. A literal here becomes a default
// the interface definition can change underneath; the consumer check owns its example.
#[test]
fn the_csharp_sdk_carries_no_protocol_config_values() {
    let root = repo().join("bindings/csharp/src/OfflineProtocol.Sdk");
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{dir:?}: {e}")) {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "cs") {
                let text = std::fs::read_to_string(&path).expect("read C# source");
                assert!(
                    !text.contains("new ProtocolConfig") && !text.contains("AppId = \""),
                    "{path:?} constructs a ProtocolConfig; the SDK must not ship one (C6)"
                );
            }
        }
    }
}

// The generator's --check runs in the C# job, which every change to the binding, the UDL
// or the generator triggers.
#[test]
fn the_csharp_job_checks_the_generated_sources() {
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.contains("python3 tools/gen_csharp_local_api.py --check"),
        "the C# job must run the generator with --check"
    );
    for input in [
        "'tools/gen_csharp_local_api.py'",
        "'bindings/python/scripts/generate_local_api_table.py'",
    ] {
        assert!(
            ci.contains(input),
            "the csharp change filter must include the generator input {input}"
        );
    }
}
