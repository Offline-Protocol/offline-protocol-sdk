fn main() {
    uniffi::generate_scaffolding("src/offline_protocol.udl").unwrap();
}
