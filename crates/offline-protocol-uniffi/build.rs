fn main() {
    // Use full UDL with all features
    uniffi::generate_scaffolding("src/offline_protocol.udl").unwrap();
}

