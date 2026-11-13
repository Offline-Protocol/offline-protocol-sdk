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

// Sign out for good: stop the engine, release it, then wipe. Releasing the
// engine flushes its state to storage, so an engine released after the wipe
// writes that state back into the account the wipe just emptied. This must be
// the last reference: every other holder (a transport manager, a service)
// must have let go first.
public func signOut(engine: inout OfflineProtocol?, config: ProtocolConfig) throws {
    try engine?.stop()
    engine = nil
    try OfflineProtocolReferenceStorage.wipe(appId: config.appId, profile: config.profile)
}

// Verify a protocol-state backend before wiring it in (C11): green means
// supported. Here it is the built-in store, opened for a scratch account and
// wiped after. Run it off the main thread: the suite blocks on writes of
// several megabytes.
public func referenceStateStoreIsSupported(appId: String, scratchProfile: String) throws -> Bool {
    defer { try? OfflineProtocolReferenceStorage.wipe(appId: appId, profile: scratchProfile) }
    let store = try OfflineProtocolReferenceStorage.makeProtocolStateStorage(
        appId: appId,
        profile: scratchProfile
    )
    let report = try StorageConformance.run(on: store)
    for failure in report.failures {
        print("ConsumerCheck: \(failure.check): \(failure.detail)")
    }
    return report.isGreen
}

public func relay(engine: OfflineProtocol) -> TransportManager {
    InternetManager(protocol: engine, deviceId: "device", appId: "app")
}

public func address(of publicKey: [UInt8]) throws -> String {
    try deriveAddress(publicKey: publicKey)
}
