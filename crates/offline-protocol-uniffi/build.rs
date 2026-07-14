fn main() {
    uniffi::generate_scaffolding("src/offline_protocol.udl")
        .expect("failed to generate UniFFI scaffolding from offline_protocol.udl");
}
