import OfflineProtocolSDK

// What an application writes first: build the engine, listen to it, start it,
// hand it to a transport. `start` and `relay` are never run. They have to
// compile, from outside the module.

public final class Listener: EventCallback, @unchecked Sendable {
    public init() {}

    public func onEvent(eventJson: String) {}
}

public func start(config: ProtocolConfig) throws -> OfflineProtocol {
    let engine = try OfflineProtocol(config: config)
    engine.setEventCallback(callback: Listener())
    try engine.start()
    return engine
}

public func referenceStorageSmokeConfig() -> ProtocolConfig {
    ConfigScenarios.forScenario(.localBle, appId: "consumer-check", profile: "default")
}

/// Native path: event callback, reference storage, then `start()` (G7).
public func bootstrapWithReferenceStorage(config: ProtocolConfig) throws -> OfflineProtocol {
    let engine = try OfflineProtocol(config: config)
    engine.setEventCallback(callback: Listener())
    try OfflineProtocolReferenceStorage.initializeMls(
        on: engine,
        appId: config.appId,
        profile: config.profile
    )
    try engine.start()
    return engine
}

public func relay(engine: OfflineProtocol) -> TransportManager {
    InternetManager(protocol: engine, deviceId: "device", appId: "app")
}

public func address(of publicKey: [UInt8]) throws -> String {
    try deriveAddress(publicKey: publicKey)
}
