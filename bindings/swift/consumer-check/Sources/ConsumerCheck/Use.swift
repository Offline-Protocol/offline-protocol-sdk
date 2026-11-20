import OfflineProtocolSDK
import UIKit

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
        appLabel: config.appLabel,
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
    try OfflineProtocolReferenceStorage.wipe(appLabel: config.appLabel, profile: config.profile)
}

// Verify a protocol-state backend before wiring it in (C11): green means
// supported. Here it is the built-in store, opened for a scratch account and
// wiped after. Run it off the main thread: the suite blocks on writes of
// several megabytes.
// Code written before 0.35 named the app label `appId`. The deprecated
// aliases keep it compiling; deprecated itself, so the check stays quiet.
@available(*, deprecated, message: "compiles the pre-0.35 names of the app label")
public func labelUnderItsOldName(config: ProtocolConfig) -> String {
    var options = SendMessageOptions()
    options.appId = config.appId
    _ = try? OfflineProtocolReferenceStorage.makeSecureStorage(appId: config.appId, profile: config.profile)
    return options.appId ?? config.appId
}

/// Lab-only config factory (compile-checked; do not use in production apps).
public func labCleartextConfig(appLabel: String, profile: String) -> ProtocolConfig {
    OfflineProtocolDevelopmentCleartextConfig.make(appLabel: appLabel, profile: profile)
}

public func referenceStateStoreIsSupported(appLabel: String, scratchProfile: String) throws -> Bool {
    defer { try? OfflineProtocolReferenceStorage.wipe(appLabel: appLabel, profile: scratchProfile) }
    let store = try OfflineProtocolReferenceStorage.makeProtocolStateStorage(
        appLabel: appLabel,
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

// MARK: - Telemetry (C12)
//
// On the native path the application is the binding: it fills the platform
// fields and owns the lifecycle. Never run. This mirrors
// `applicationDidEnterBackground` and `applicationWillEnterForeground` in the
// React Native module (bindings/react-native/ios/OfflineProtocolModule.swift),
// which is not in the package.

/// The platform fields come from the device, never from app configuration.
public func telemetryConfig(apiKey: String, appId: String, appVersion: String?) -> TelemetryConfig {
    TelemetryConfig(
        apiKey: apiKey,
        appId: appId,
        os: .ios,
        osMajor: UInt16(clamping: ProcessInfo.processInfo.operatingSystemVersion.majorVersion),
        appVersion: appVersion,
        debug: nil,
        flushIntervalMs: nil,
        maxBatchBytes: nil,
        maxBufferedRecords: nil,
        includeDeviceId: nil,
        scrubIds: nil,
        mlsVerbosity: nil,
        metricsCadenceMs: nil,
        routingDiagnostic: nil,
        mlsSamplingBypass: nil
    )
}

/// Forwards the application's background and foreground edges to the
/// telemetry pipe. Create it on the main thread; `invalidate()` it before the
/// engine is released.
public final class TelemetryLifecycleForwarder: NSObject {
    private let engine: OfflineProtocol
    // One serial queue for both edges, so `.active` cannot land while the
    // background flush is still running and rotate the session out from
    // under it.
    private let queue = DispatchQueue(label: "consumer.telemetry")
    // Main-confined: armed by the background notification, ended by the
    // expiration handler or a hop back to main.
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid

    public init(engine: OfflineProtocol) {
        self.engine = engine
        super.init()
        let center = NotificationCenter.default
        center.addObserver(
            self,
            selector: #selector(didEnterBackground),
            name: UIApplication.didEnterBackgroundNotification,
            object: nil
        )
        center.addObserver(
            self,
            selector: #selector(willEnterForeground),
            name: UIApplication.willEnterForegroundNotification,
            object: nil
        )
    }

    /// The state to seed `enableTelemetry` with. Main thread only.
    public var appState: AppState {
        UIApplication.shared.applicationState == .background ? .background : .active
    }

    public func invalidate() {
        NotificationCenter.default.removeObserver(self)
    }

    @objc private func didEnterBackground() {
        // Supersede an assertion left from an earlier edge; its own
        // completion then matches nothing and does nothing.
        endBackgroundTask(backgroundTask)
        let task = UIApplication.shared.beginBackgroundTask(withName: "consumer.telemetryFlush") {
            self.endBackgroundTask(self.backgroundTask)
        }
        backgroundTask = task
        let engine = self.engine
        // Off main, because the flush blocks for up to three seconds, and
        // inside the assertion, because without it iOS may suspend the
        // process before the uploader gets a turn and the batch waits for the
        // next launch.
        queue.async {
            engine.notifyAppState(state: .background)
            _ = engine.flushTelemetryBlocking(deadlineMs: 3000)
            DispatchQueue.main.async { self.endBackgroundTask(task) }
        }
    }

    @objc private func willEnterForeground() {
        let engine = self.engine
        // The first foreground after a background opens the next session.
        queue.async { engine.notifyAppState(state: .active) }
    }

    private func endBackgroundTask(_ id: UIBackgroundTaskIdentifier) {
        guard id != .invalid, backgroundTask == id else { return }
        UIApplication.shared.endBackgroundTask(id)
        backgroundTask = .invalid
    }
}

/// Watch the lifecycle, then enable seeded from it. Main thread only.
public func enableTelemetry(
    on engine: OfflineProtocol,
    apiKey: String,
    appId: String,
    appVersion: String?
) throws -> TelemetryLifecycleForwarder {
    let lifecycle = TelemetryLifecycleForwarder(engine: engine)
    try engine.enableTelemetry(
        config: telemetryConfig(apiKey: apiKey, appId: appId, appVersion: appVersion),
        appState: lifecycle.appState
    )
    return lifecycle
}
