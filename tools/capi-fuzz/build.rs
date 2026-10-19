//! The FFI crate's scaffolding, generated from its interface definition as
//! that crate's own build script does, for the copy of its source this
//! package compiles (see `Cargo.toml`).

fn main() {
    let udl = "../../crates/offline-protocol-uniffi/src/offline_protocol.udl";
    println!("cargo:rerun-if-changed={udl}");
    uniffi::generate_scaffolding(udl)
        .expect("failed to generate UniFFI scaffolding from offline_protocol.udl");
}
