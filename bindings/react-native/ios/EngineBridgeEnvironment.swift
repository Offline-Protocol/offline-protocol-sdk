import Foundation

/// Lifecycle and replay hooks the shared [ProtocolEngineHostBridge] needs from
/// a wrapper (Flutter plugin or React Native module). Flutter and RN fill this
/// differently; most bridge methods ignore it.
struct EngineBridgeEnvironment {
    var appState: () -> AppState
    var replayEvents: (Int, Int) -> [[String: Any]]
    var tearDownProtocol: () -> Void
    var createOnEngine: (String) throws -> Void
    var startOnEngine: () throws -> Void
    var stopOnEngine: () throws -> Void

    /// Used when a React Native `@objc` method delegates through the bridge
    /// but does not touch lifecycle (the majority of the shared surface).
    static func inert() -> EngineBridgeEnvironment {
        EngineBridgeEnvironment(
            appState: { .background },
            replayEvents: { _, _ in [] },
            tearDownProtocol: {},
            createOnEngine: { _ in },
            startOnEngine: {},
            stopOnEngine: {}
        )
    }
}
