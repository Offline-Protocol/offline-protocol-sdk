import OfflineProtocolSDK

// What an application writes first: build the engine, listen to it, start it,
// hand it to a transport. `start`, `startWithReferenceStorage`, `signOut` and
// `relay` are never run. They have to compile, from outside the module.

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

// A native application's start: listen, open the built-in stores, then start.
// MLS must be initialized before `start()`, and the adoption warning is the
// only signal that this account lost an earlier identity.
public func startWithReferenceStorage(config: ProtocolConfig) throws -> OfflineProtocol {
    let engine = try OfflineProtocol(config: config)
    engine.setEventCallback(callback: Listener())
    let adoption = try OfflineProtocolReferenceStorage.initializeMls(
        on: engine,
        appId: config.appId,
        profile: config.profile
    )
    if let warning = adoption.warning {
        print("ConsumerCheck: \(warning)")
    }
    try engine.start()
    return engine
}

public func signOut(engine: OfflineProtocol, config: ProtocolConfig) throws {
    try engine.stop()
    try OfflineProtocolReferenceStorage.wipe(appId: config.appId, profile: config.profile)
}

public func relay(engine: OfflineProtocol) -> TransportManager {
    InternetManager(protocol: engine, deviceId: "device", appId: "app")
}

public func address(of publicKey: [UInt8]) throws -> String {
    try deriveAddress(publicKey: publicKey)
}
