//! Writes `crates/offline-protocol-uniffi/src/capi/generated.rs` from
//! `offline_protocol.udl`.
//!
//! ```text
//! cargo run --manifest-path tools/capi-dispatch-gen/Cargo.toml            # write
//! cargo run --manifest-path tools/capi-dispatch-gen/Cargo.toml -- --check # compare
//! ```
//!
//! The generator itself lives in the FFI crate (`capi/generator.rs`), where
//! `the_generated_c_dispatch_is_current` runs it on every `cargo test`. This
//! binary compiles that one file on its own, so it runs even while the FFI
//! crate does not compile, which is exactly the state a rename in the
//! definition leaves it in.

use std::path::PathBuf;
use std::process::ExitCode;

#[path = "../../../crates/offline-protocol-uniffi/src/capi/generator.rs"]
#[allow(dead_code)]
mod generator;

fn main() -> ExitCode {
    let check = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--check") => true,
        Some(other) => {
            eprintln!("usage: capi-dispatch-gen [--check] (got {other:?})");
            return ExitCode::from(2);
        }
    };
    let crate_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/offline-protocol-uniffi");
    let udl_path = crate_dir.join("src/offline_protocol.udl");
    let out_path = crate_dir.join(generator::OUTPUT);
    let udl = match std::fs::read_to_string(&udl_path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("cannot read {}: {e}", udl_path.display());
            return ExitCode::FAILURE;
        }
    };
    let fresh = generator::generate(&udl);
    if check {
        let current = std::fs::read_to_string(&out_path).unwrap_or_default();
        if current == fresh {
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "{} is stale against the interface definition",
            out_path.display()
        );
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::write(&out_path, fresh) {
        eprintln!("cannot write {}: {e}", out_path.display());
        return ExitCode::FAILURE;
    }
    println!("wrote {}", out_path.display());
    ExitCode::SUCCESS
}
