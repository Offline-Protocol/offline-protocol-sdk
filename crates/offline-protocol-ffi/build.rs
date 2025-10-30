use std::env;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    cbindgen::Builder::new()
        .with_crate(crate_dir)
        .with_language(cbindgen::Language::C)
        .with_pragma_once(true)
        .with_include_guard("OFFLINE_PROTOCOL_H")
        .with_documentation(true)
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file("offline_protocol.h");
}

