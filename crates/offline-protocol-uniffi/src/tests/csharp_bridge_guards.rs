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

// The typed config and the local API wrappers build their JSON through
// System.Text.Json and the source-generated contexts, and the engine hands
// each generated client out exactly once.
#[test]
fn csharp_typed_config_and_local_api_use_generated_json() {
    let config = read("bindings/csharp/src/OfflineProtocol.Sdk/Config/ProtocolConfigJson.cs");
    assert!(
        config.contains("JsonSerializer.Serialize") && config.contains("MinimalConsumer"),
        "ProtocolConfigJson must serialize typed ProtocolConfig (C6)"
    );
    let params = read("bindings/csharp/src/OfflineProtocol.Sdk/LocalApi/LocalApiParams.cs");
    assert!(
        params.contains("LocalApiJsonContext.Default")
            && params.contains("SendMessagePayload")
            && params.contains("ArrayBufferWriter"),
        "LocalApiParams must serialize via source-generated JSON contexts"
    );
    let state_codec =
        read("bindings/csharp/src/OfflineProtocol.Sdk/LocalApi/LocalApiStateCodec.cs");
    assert!(
        state_codec.contains("ProtocolEngineException")
            && state_codec.contains("JsonValueKind.String")
            && state_codec.contains("InvalidResponse"),
        "get_state parsing must require a JSON string ProtocolState and surface InvalidResponse"
    );
    let engine = read("bindings/csharp/src/OfflineProtocol.Sdk/ProtocolEngine.cs");
    assert!(
        engine.contains("partial class ProtocolEngine")
            && engine.contains("MeshServicesClient Services")
            && engine.contains("DataStoreClient Data")
            && engine.contains("LazyThreadSafetyMode.ExecutionAndPublication"),
        "ProtocolEngine must be partial, expose generated Services/Data clients, \
         and publish each client exactly once via Lazy<T>"
    );
    let generated = "bindings/csharp/src/OfflineProtocol.Sdk/Generated";
    assert!(
        repo()
            .join(generated)
            .join("LocalApiClients.g.cs")
            .is_file(),
        "Generated local API clients must live in LocalApiClients.g.cs (single compilation unit)"
    );
    for legacy in [
        "ProtocolEngine.LocalApi.g.cs",
        "MeshServicesClient.g.cs",
        "DataStoreClient.g.cs",
        "LocalApiStatic.g.cs",
    ] {
        assert!(
            !repo().join(generated).join(legacy).is_file(),
            "{generated}/{legacy} must not exist: generated C# uses LocalApiClients.g.cs \
             and ProtocolEngine.LocalApi.part*.g.cs"
        );
    }
    let priority = read("bindings/csharp/src/OfflineProtocol.Sdk/Model/MessagePriority.cs");
    assert!(
        priority.contains("Medium") && priority.contains("Critical"),
        "MessagePriority must mirror the UDL enum"
    );
}

#[test]
fn csharp_generated_local_api_is_current() {
    let status = std::process::Command::new("python3")
        .arg("tools/gen_csharp_local_api.py")
        .arg("--check")
        .current_dir(repo())
        .status()
        .unwrap_or_else(|e| panic!("cannot run gen_csharp_local_api.py --check: {e}"));
    assert!(
        status.success(),
        "bindings/csharp Generated/*.g.cs or local_api_methods.txt is stale; \
         run tools/gen_csharp_local_api.py"
    );
    let pinned = |rel: &str| -> usize {
        read(rel)
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("{rel} must be an integer: {e}"))
    };
    let manifest = read("bindings/csharp/local_api_methods.txt");
    let line_count = manifest.lines().filter(|l| !l.is_empty()).count();
    assert_eq!(
        line_count,
        pinned("bindings/csharp/local_api_method_count.txt"),
        "local_api_methods.txt line count must match tools/gen_csharp_local_api.py output"
    );
    let generated = repo().join("bindings/csharp/src/OfflineProtocol.Sdk/Generated");
    let part_count = std::fs::read_dir(&generated)
        .unwrap_or_else(|e| panic!("cannot read Generated/: {e}"))
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("ProtocolEngine.LocalApi.part"))
        })
        .count();
    assert_eq!(
        part_count,
        pinned("bindings/csharp/local_api_engine_part_count.txt"),
        "ProtocolEngine.LocalApi.part*.g.cs count must match the generator pin"
    );
    let clients = read("bindings/csharp/src/OfflineProtocol.Sdk/Generated/LocalApiClients.g.cs");
    assert!(
        clients.contains("UDL_SHA256 =") && clients.contains("gen_csharp_local_api.py"),
        "LocalApiClients.g.cs must carry the UDL digest header from the generator"
    );
    let part_one = read(
        "bindings/csharp/src/OfflineProtocol.Sdk/Generated/ProtocolEngine.LocalApi.part01.g.cs",
    );
    assert!(
        part_one.contains("UDL_SHA256 =") && part_one.contains("partial class ProtocolEngine"),
        "engine part files must carry the UDL digest and extend ProtocolEngine"
    );
}
