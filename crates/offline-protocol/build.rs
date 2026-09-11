//! Bakes the telemetry ingest endpoint into the crate at build time.
//!
//! The endpoint is a compile-time constant on purpose: the pipe has no runtime
//! endpoint setting, so an application cannot be pointed at a look-alike by a
//! configuration value, and a build that wants a different ingest (a staging
//! server, the live integration test) says so in its environment rather than
//! in code. Only `https://` is accepted; a plaintext endpoint would put every
//! bearer token on the wire in the clear, and refusing at build time is
//! cheaper than discovering it in a packet capture.

use std::env;

const DEFAULT_ENDPOINT: &str = "https://analytics.offlineprotocol.com/v1/events";

fn main() {
    println!("cargo:rerun-if-env-changed=OFFLINE_TELEMETRY_ENDPOINT");
    let endpoint = env::var("OFFLINE_TELEMETRY_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
    assert!(
        endpoint.starts_with("https://"),
        "OFFLINE_TELEMETRY_ENDPOINT must start with https:// (got {endpoint:?}); \
         the telemetry pipe never sends a bearer token over plaintext"
    );
    println!("cargo:rustc-env=OFFLINE_TELEMETRY_ENDPOINT={endpoint}");
}
