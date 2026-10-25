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
    ///
    /// Every hook is a no-op and `appState` reports `.background`. That is
    /// safe only while no handler that reads the environment (`create`,
    /// `start`, `stop`, `destroy`, `replayEvents`, `enableTelemetry`) is in the
    /// React Native method list: React Native keeps those on the module. A
    /// method moved into that list needs the module to pass a real
    /// environment, or it does nothing and reports success.
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
