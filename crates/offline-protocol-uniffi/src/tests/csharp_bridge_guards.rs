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
