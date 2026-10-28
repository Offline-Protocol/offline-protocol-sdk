//! C# binding layout and docs (PR1 scaffold). Windows CI guards land in a later PR.

#[test]
fn csharp_bridges_doc_is_linked_from_the_bridge_index() {
    let readme = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/bridges/README.md"),
    )
    .unwrap_or_else(|e| panic!("cannot read docs/bridges/README.md: {e}"));
    let csharp_doc =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/bridges/csharp.md");
    assert!(
        csharp_doc.is_file(),
        "docs/bridges/csharp.md must exist: it is the C# bridge contract"
    );
    assert!(
        readme.contains("csharp.md"),
        "docs/bridges/README.md must link docs/bridges/csharp.md"
    );
}

#[test]
fn csharp_engine_runs_native_calls_on_a_dedicated_thread() {
    let engine = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../bindings/csharp/src/OfflineProtocol.Sdk/ProtocolEngine.cs"),
    )
    .unwrap_or_else(|e| panic!("cannot read ProtocolEngine.cs: {e}"));
    assert!(
        engine.contains("offline-protocol-engine")
            && engine.contains("Channel.CreateBounded")
            && engine.contains("RunEngineLoop")
            && engine.contains("ShutdownEngineLoopAfterFailedCreate")
            && engine.contains("WaitForNoOutboundWork")
            && engine.contains("TryBeginOutboundWork"),
        "ProtocolEngine must serialize native calls on a dedicated thread (CS1) and \
         free the native handle only after outbound work drains"
    );
    let config = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../bindings/csharp/src/OfflineProtocol.Sdk/ProtocolConfigJson.cs"),
    )
    .unwrap_or_else(|e| panic!("cannot read ProtocolConfigJson.cs: {e}"));
    assert!(
        config.contains("JsonSerializer.Serialize"),
        "FileStores must build JSON with System.Text.Json, not manual escaping"
    );
}
