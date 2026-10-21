//
// ProtocolEngineHost.swift
//
// Non-React host for the Offline Protocol engine, shared by the Flutter plugin
// and any other wrapper that owns an engine (ADR 0025). Mirrors
// android/ ProtocolEngineHost.kt: keep the construction order, the teardown
// order and the transport-enable rules in sync. No React import: the Swift
// package assembly refuses one, which is the point of the file.
//
// Holds the protocol handle, the transport managers, mesh services, the data
// store and the background process scheduler. The wrapper supplies the event
// callback and a diagnostic sink; it never reaches into the managers' wiring.
//
// Threading: every mutating call is made from the wrapper's serial engine
// queue. The handles are still guarded by a lock, because the process
// scheduler and the transport callbacks read them from their own queues, and
// an unsynchronised read of a reference-typed property is a memory-safety race.
//

import Foundation

/// A failure the host raises itself, as opposed to one the core reports.
/// `localizedDescription` is the message, so a wrapper that maps unrecognised
/// errors to a generic code still shows the reason.
struct HostError: LocalizedError {
    let message: String

    var errorDescription: String? { message }

    static func illegalState(_ message: String) -> HostError { HostError(message: message) }
    static func invalidArgument(_ message: String) -> HostError { HostError(message: message) }
}

/// Runs teardown steps in order and keeps going when one throws, so a failure
/// in one transport never leaves the next one running. The first failure is
/// reported to the caller after every step has had its turn.
final class TeardownSequence {
    struct Failure {
        let step: String
        let cause: Error
    }

    private(set) var failures: [Failure] = []

    func step(_ name: String, _ body: () throws -> Void) {
        do {
            try body()
        } catch {
            failures.append(Failure(step: name, cause: error))
        }
    }

    func firstFailure() -> Error? {
        failures.first?.cause
    }
}

/// A value read from more than one queue. A class, so the wrapped property of
/// the owning class can be read and written without a `mutating` accessor.
@propertyWrapper
final class HostGuarded<Value> {
    private let lock = NSLock()
    private var value: Value

    init(wrappedValue: Value) {
        value = wrappedValue
    }

    init() where Value: ExpressibleByNilLiteral {
        value = nil
    }

    var wrappedValue: Value {
        get {
            lock.lock()
            defer { lock.unlock() }
            return value
        }
        set {
            lock.lock()
            value = newValue
            lock.unlock()
        }
    }
}

final class ProtocolEngineHost: TransportManagerDelegate {
    /// `(level, message, context)`, with level one of `info`, `warning`, `error`.
    typealias DiagnosticHandler = (_ level: String, _ message: String, _ context: [String: Any]) -> Void

    @HostGuarded var protocolInstance: OfflineProtocol?
    @HostGuarded var meshServices: MeshServices?
    @HostGuarded var dataStore: DataStore?
    @HostGuarded var bleManager: BleManager?
    @HostGuarded var internetManager: InternetManager?
    @HostGuarded var wifiDirectManager: WifiDirectManager?
    @HostGuarded var reticulumManager: ReticulumManager?
    @HostGuarded var nostrManager: NostrManager?
    @HostGuarded var currentConfig: ProtocolConfig?

    /// How the Internet manager's relay frames reach the host's event stream.
    /// Kept so a manager created on demand by `enableTransport` is wired the
    /// same way as the one `create` builds.
    @HostGuarded private var emitEventJson: ((String) -> Void)?

    private let onDiagnostic: DiagnosticHandler

    private(set) lazy var backgroundProcessor = ProtocolBackgroundProcessor(
        protocolProvider: { [weak self] in self?.protocolInstance },
        onDiagnostic: { [weak self] level, message, context in
            self?.onDiagnostic(level, message, context)
        }
    )

    init(onDiagnostic: @escaping DiagnosticHandler) {
        self.onDiagnostic = onDiagnostic
    }

    // MARK: - Lifecycle

    /// Builds the core and every transport manager the config enables, wires
    /// the callbacks and starts the process scheduler. Does not start the core.
    ///
    /// Reticulum and Nostr are hosted: the managers are the shared ones the
    /// React Native module drives. They are created here and configured and
    /// started by `enableTransport`, exactly as on Android.
    @discardableResult
    func create(
        configJson: String,
        eventCallback: EventCallback,
        emitEventJson emitter: ((String) -> Void)?
    ) throws -> ProtocolConfig {
        let parsed = try ProtocolConfigParser.parse(configJson)
        let config = parsed.config
        let proto = try OfflineProtocol(config: config)
        currentConfig = config
        emitEventJson = emitter
        onDiagnostic("info", "Protocol core created", [
            "appId": config.appId,
            "userId": config.profile,
            "bleEnabled": config.bleEnabled,
            "wifiDirectEnabled": config.wifiDirectEnabled,
            "internetEnabled": config.internetEnabled,
            "reticulumEnabled": config.reticulumEnabled,
            "nostrEnabled": config.nostrEnabled,
        ])
        proto.setEventCallback(callback: eventCallback)
        InitialRuntimeConfigApplier.apply(proto: proto, rawConfig: parsed.raw) { [weak self] level, message, context in
            self?.onDiagnostic(level, message, context ?? [:])
        }
        protocolInstance = proto
        // Constructed unconditionally, like the React Native module: the store
        // is inert until the config enables it, and every method answers
        // DataDisabled until then.
        meshServices = try MeshServices(protocol: proto)
        dataStore = try DataStore(protocol: proto)

        if config.bleEnabled {
            bleManager = makeBleManager(proto: proto, config: config)
            onDiagnostic("info", "BLE manager initialized", ["userId": config.profile])
        } else {
            onDiagnostic("warning", "BLE disabled in configuration", ["userId": config.profile])
        }
        if config.internetEnabled {
            internetManager = makeInternetManager(proto: proto, config: config)
            onDiagnostic("info", "Internet manager initialized", ["userId": config.profile])
        } else {
            onDiagnostic("info", "Internet disabled in configuration", ["userId": config.profile])
        }
        if config.reticulumEnabled {
            reticulumManager = makeReticulumManager(proto: proto, userId: config.profile)
            onDiagnostic("info", "Reticulum manager initialized", ["userId": config.profile])
        } else {
            onDiagnostic("info", "Reticulum disabled in configuration", ["userId": config.profile])
        }
        if config.nostrEnabled {
            nostrManager = makeNostrManager(proto: proto)
            onDiagnostic("info", "Nostr manager initialized", ["userId": config.profile])
        } else {
            onDiagnostic("info", "Nostr disabled in configuration", ["userId": config.profile])
        }
        wireTransportCallbacks(proto)
        backgroundProcessor.startProcessScheduler()
        onDiagnostic("info", "Protocol process scheduler started", [:])
        return config
    }

    /// Stops the scheduler, every transport and the core, in that order. A
    /// failing step never skips the ones after it; the first failure is thrown
    /// once all have run.
    func stopTransportsAndProtocol() throws {
        let teardown = TeardownSequence()
        teardown.step("process scheduler") { backgroundProcessor.stopProcessScheduler() }
        stopTransports(into: teardown)
        teardown.step("protocol core") {
            try protocolInstance?.stop()
            onDiagnostic("info", "Protocol stopped", [:])
        }
        reportFailures(teardown)
        if let failure = teardown.firstFailure() { throw failure }
    }

    /// Stops everything and releases every handle. Never throws: this runs
    /// before a re-create and on detach, where a stop failure has nobody to
    /// act on it. Failures are reported through the diagnostic sink.
    func tearDown() {
        backgroundProcessor.stopProcessScheduler()
        let teardown = TeardownSequence()
        stopTransports(into: teardown)
        teardown.step("protocol core") { try protocolInstance?.stop() }
        reportFailures(teardown)
        releaseHandlesAfterStop()
    }

    /// Drops the handles after the core has stopped. The data store holds a
    /// strong reference to the engine: left set, every data call after
    /// teardown would reach a stopped engine instead of failing with
    /// "DataStore not initialized", and keep it alive.
    func releaseHandlesAfterStop() {
        bleManager = nil
        internetManager = nil
        wifiDirectManager = nil
        reticulumManager = nil
        nostrManager = nil
        dataStore = nil
        protocolInstance = nil
        meshServices = nil
        currentConfig = nil
        emitEventJson = nil
    }

    func startProtocolCore() throws {
        try requireProtocol().start()
        onDiagnostic("info", "Protocol core started", [:])
    }

    func startBleTransportIfPresent() throws {
        guard let manager = bleManager, manager.state != .running else { return }
        try manager.start()
    }

    func pause() throws {
        backgroundProcessor.stopProcessScheduler()
        // Every manager the host can hold, as Android's `pause` names the same
        // five: a manager missing from this list is a transport whose
        // `isPaused` can never become true.
        let teardown = TeardownSequence()
        teardown.step("BLE manager") { bleManager?.pause() }
        teardown.step("Internet manager") { internetManager?.pause() }
        teardown.step("WiFi Direct manager") { wifiDirectManager?.pause() }
        teardown.step("Reticulum manager") { reticulumManager?.pause() }
        teardown.step("Nostr manager") { nostrManager?.pause() }
        teardown.step("protocol") { try protocolInstance?.pause() }
        if let failure = teardown.firstFailure() { throw failure }
    }

    func resume() throws {
        // Core first, so a transport never hands work to a still-paused core.
        let teardown = TeardownSequence()
        teardown.step("protocol") { try protocolInstance?.resume() }
        teardown.step("BLE manager") { bleManager?.resume() }
        teardown.step("Internet manager") { internetManager?.resume() }
        teardown.step("WiFi Direct manager") { wifiDirectManager?.resume() }
        teardown.step("Reticulum manager") { reticulumManager?.resume() }
        teardown.step("Nostr manager") { nostrManager?.resume() }
        teardown.step("process scheduler") {
            if protocolInstance?.getState() == .running {
                backgroundProcessor.startProcessScheduler()
            }
        }
        if let failure = teardown.firstFailure() { throw failure }
    }

    private func stopTransports(into teardown: TeardownSequence) {
        teardown.step("BLE manager") {
            bleManager?.stop()
            onDiagnostic("info", "BLE manager stopped", [:])
        }
        teardown.step("Internet manager") {
            internetManager?.stop()
            onDiagnostic("info", "Internet manager stopped", [:])
        }
        teardown.step("WiFi Direct manager") {
            wifiDirectManager?.stop()
            onDiagnostic("info", "WiFi Direct manager stopped", [:])
        }
        teardown.step("Reticulum manager") {
            reticulumManager?.stop()
            onDiagnostic("info", "Reticulum manager stopped", [:])
        }
        teardown.step("Nostr manager") {
            nostrManager?.stop()
            onDiagnostic("info", "Nostr manager stopped", [:])
        }
    }

    private func reportFailures(_ teardown: TeardownSequence) {
        for failure in teardown.failures {
            onDiagnostic("error", "Teardown step failed", [
                "step": failure.step,
                "message": failure.cause.localizedDescription,
                "exception": String(describing: type(of: failure.cause)),
            ])
        }
    }

    // MARK: - Transports

    /// Creates the transport on demand if the config did not, configures it
    /// from `config` and starts it. A transport that is already running or
    /// connecting is stopped first, so a re-enable is a clean restart.
    func enableTransport(type: String, config: [String: Any]?) throws {
        let proto = try requireProtocol()
        switch type.lowercased() {
        case "internet":
            let manager = try ensureInternetManager(proto: proto)
            if manager.state == .running || manager.state == .starting {
                manager.stop()
            }
            try InternetTransportBootstrap.configureAndStart(manager: manager, config: config ?? [:])
            onDiagnostic("info", "Internet transport enabled", [:])
        case "wifidirect", "wifi_direct":
            let manager: WifiDirectManager
            if let existing = wifiDirectManager {
                manager = existing
            } else {
                manager = WifiDirectManager(protocol: proto, deviceId: currentConfig?.profile ?? "unknown")
                manager.delegate = self
                wifiDirectManager = manager
                onDiagnostic("info", "WiFi Direct manager created on demand", [:])
            }
            if manager.state == .running {
                manager.stop()
            }
            wireTransportCallbacks(proto)
            try manager.start()
            onDiagnostic("info", "WiFi Direct transport enabled", [:])
        case "ble":
            let manager: BleManager
            if let existing = bleManager {
                manager = existing
            } else {
                manager = makeBleManager(proto: proto, config: currentConfig)
                bleManager = manager
                onDiagnostic("info", "BLE manager created on demand", [:])
            }
            wireTransportCallbacks(proto)
            if manager.state != .running {
                try manager.start()
                onDiagnostic("info", "BLE transport enabled", [:])
            }
        case "reticulum":
            let manager: ReticulumManager
            if let existing = reticulumManager {
                manager = existing
            } else {
                manager = makeReticulumManager(proto: proto, userId: currentConfig?.profile ?? "unknown")
                reticulumManager = manager
                onDiagnostic("info", "Reticulum manager created on demand", [:])
            }
            wireTransportCallbacks(proto)
            if manager.state == .running {
                manager.stop()
            }
            try configureAndStartReticulum(manager, config: config)
        case "nostr":
            let manager: NostrManager
            if let existing = nostrManager {
                manager = existing
            } else {
                manager = makeNostrManager(proto: proto)
                nostrManager = manager
                onDiagnostic("info", "Nostr manager created on demand", [:])
            }
            wireTransportCallbacks(proto)
            if manager.state == .running {
                manager.stop()
            }
            try configureAndStartNostr(manager, config: config)
        default:
            throw HostError.invalidArgument("Unsupported transport type: \(type)")
        }
    }

    func enableTransport(type: String, configJson: String?) throws {
        var object: [String: Any]?
        if let configJson = configJson, !configJson.isEmpty {
            guard let data = configJson.data(using: .utf8),
                  let parsed = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
                throw HostError.invalidArgument("Invalid transport config JSON")
            }
            object = parsed
        }
        try enableTransport(type: type, config: object)
    }

    /// Hands the core the callback that wakes each transport when it has
    /// something to send: an accelerant for the process tick, never a
    /// replacement. Safe to call again; each registration replaces the last.
    /// The callbacks resolve the manager through the host when they fire, so a
    /// manager replaced later is the one that is woken.
    func wireTransportCallbacks(_ proto: OfflineProtocol) {
        if bleManager != nil {
            proto.setBleTransportCallback(callback: HostBleTransportCallback(host: self))
            onDiagnostic("info", "BLE transport callback wired", [:])
        }
        if currentConfig?.wifiDirectEnabled == true || wifiDirectManager != nil {
            proto.setWifiDirectTransportCallback(callback: HostWifiDirectTransportCallback(host: self))
            onDiagnostic("info", "WiFi Direct transport callback registered", [:])
        }
        if reticulumManager != nil {
            proto.setReticulumTransportCallback(callback: HostReticulumTransportCallback(host: self))
            onDiagnostic("info", "Reticulum transport callback wired", [:])
        }
        if nostrManager != nil {
            // For Nostr this always lands before the transport exists (it is
            // built during the identity rebuild), so the core stores it and
            // installs it there. Polling remains the floor.
            proto.setNostrTransportCallback(callback: HostNostrTransportCallback(host: self))
            onDiagnostic("info", "Nostr transport callback wired", [:])
        }
    }

    private func makeBleManager(proto: OfflineProtocol, config: ProtocolConfig?) -> BleManager {
        let manager = BleManager(
            protocol: proto,
            deviceId: config?.profile ?? "unknown",
            appId: config?.appId ?? ""
        )
        manager.delegate = self
        return manager
    }

    private func makeInternetManager(proto: OfflineProtocol, config: ProtocolConfig) -> InternetManager {
        let manager = InternetManager(protocol: proto, deviceId: config.profile, appId: config.appId)
        manager.delegate = self
        if let emit = emitEventJson {
            InternetTransportBootstrap.attachEmitters(manager: manager, emitEventJson: emit)
        }
        return manager
    }

    private func makeReticulumManager(proto: OfflineProtocol, userId: String) -> ReticulumManager {
        let manager = ReticulumManager(protocol: proto, deviceId: userId)
        manager.delegate = self
        return manager
    }

    private func makeNostrManager(proto: OfflineProtocol) -> NostrManager {
        let manager = NostrManager(protocol: proto)
        manager.delegate = self
        return manager
    }

    /// The manager's identity must be the real user id: the control-op
    /// translator filters self out of member deltas and gates LeaveGroup on
    /// it, so a placeholder would silently corrupt relay group state.
    private func ensureInternetManager(proto: OfflineProtocol) throws -> InternetManager {
        if let manager = internetManager {
            return manager
        }
        guard let config = currentConfig else {
            throw HostError.illegalState("Cannot enable Internet transport before initialize(config)")
        }
        let manager = makeInternetManager(proto: proto, config: config)
        internetManager = manager
        onDiagnostic("info", "Internet manager created on demand", [:])
        return manager
    }

    private func configureAndStartReticulum(_ manager: ReticulumManager, config: [String: Any]?) throws {
        let daemonAddress = (config?["daemonAddress"] as? String)
            ?? (config?["daemon_address"] as? String)
            ?? "localhost:4242"
        let autoReconnect = (config?["autoReconnect"] as? Bool) ?? true
        let maxRetries = (config?["maxReconnectAttempts"] as? NSNumber)?.intValue ?? 0
        manager.configure(daemonAddress: daemonAddress, autoReconnect: autoReconnect, maxReconnectAttempts: maxRetries)
        try manager.start()
        onDiagnostic("info", "Reticulum transport enabled", [
            "daemonAddress": daemonAddress,
            "autoReconnect": autoReconnect,
        ])
    }

    private func configureAndStartNostr(_ manager: NostrManager, config: [String: Any]?) throws {
        let relayUrls = ((config?["relayUrls"] as? [String]) ?? (config?["relay_urls"] as? [String]) ?? [])
            .filter { !$0.isEmpty }
        guard !relayUrls.isEmpty else {
            throw HostError.invalidArgument("Nostr transport requires at least one relay URL")
        }
        let autoReconnect = (config?["autoReconnect"] as? Bool) ?? true
        let maxRetries = (config?["maxReconnectAttempts"] as? NSNumber)?.intValue ?? 0
        let connectionTimeout = (config?["connectionTimeout"] as? NSNumber)?.doubleValue ?? 30.0
        manager.configure(
            relayUrls: relayUrls,
            autoReconnect: autoReconnect,
            maxReconnectAttempts: maxRetries,
            connectionTimeout: connectionTimeout
        )
        try manager.start()
        onDiagnostic("info", "Nostr transport enabled", ["relayCount": relayUrls.count])
    }

    // MARK: - TransportManagerDelegate

    func transportManager(_ manager: TransportManager, didChangeState state: TransportState) {
        onDiagnostic("info", "\(manager.transportName) state changed", [
            "transport": manager.transportId,
            "state": String(describing: state),
        ])
    }

    func transportManager(_ manager: TransportManager, didEncounterError error: Error) {
        onDiagnostic("error", "\(manager.transportName) error", [
            "transport": manager.transportId,
            "error": error.localizedDescription,
        ])
    }

    func transportManager(
        _ manager: TransportManager,
        didEmitDiagnostic level: String,
        message: String,
        context: [String: Any]
    ) {
        var enriched = context
        enriched["transport"] = manager.transportId
        onDiagnostic(level, message, enriched)
    }

    // MARK: - Telemetry

    func enableTelemetry(config: [String: Any]?, initialAppState: AppState) throws {
        try requireProtocol().enableTelemetry(config: parseTelemetryConfig(config), appState: initialAppState)
    }

    func disableTelemetry() throws {
        try requireProtocol().disableTelemetry()
    }

    func flushTelemetry() {
        protocolInstance?.flushTelemetry()
    }

    func telemetryStats() -> TelemetryStats? {
        protocolInstance?.telemetryStats()
    }

    func endTelemetrySession() {
        protocolInstance?.endTelemetrySession()
    }

    func setTelemetryEnabled(_ enabled: Bool) {
        protocolInstance?.setTelemetryEnabled(enabled: enabled)
    }

    func telemetryInstallId() throws -> String? {
        try requireProtocol().telemetryInstallId()
    }

    /// The platform fields are filled here, never read from the map: the
    /// ingest keys its per-platform aggregates on them, and an app cannot set
    /// them wrong if it cannot set them. The two required strings are
    /// forwarded as given, an absent one as "": the core validates and names
    /// the refused field. An unrecognised `mlsVerbosity` falls back to the Rust
    /// default rather than silently looking like it took effect.
    func parseTelemetryConfig(_ map: [String: Any]?) -> TelemetryConfig {
        let m = map ?? [:]
        let verbosity: MlsVerbosity?
        if let raw = m["mlsVerbosity"] as? String {
            switch raw.lowercased() {
            case "off": verbosity = .off
            case "diagnostic": verbosity = .diagnostic
            case "lifecycle": verbosity = .lifecycle
            default:
                onDiagnostic(
                    "warning",
                    "telemetry: unknown mlsVerbosity '\(raw)', expected 'off', 'lifecycle' or 'diagnostic'",
                    [:]
                )
                verbosity = nil
            }
        } else {
            verbosity = nil
        }
        let osMajor = UInt16(clamping: ProcessInfo.processInfo.operatingSystemVersion.majorVersion)
        return TelemetryConfig(
            apiKey: m["apiKey"] as? String ?? "",
            appId: m["appId"] as? String ?? "",
            os: .ios,
            osMajor: osMajor,
            appVersion: m["appVersion"] as? String,
            debug: m["debug"] as? Bool,
            flushIntervalMs: optionalNonNegativeUInt64(m, "flushIntervalMs"),
            maxBatchBytes: optionalNonNegativeUInt64(m, "maxBatchBytes"),
            maxBufferedRecords: optionalNonNegativeUInt32(m, "maxBufferedRecords"),
            includeDeviceId: m["includeDeviceId"] as? Bool,
            scrubIds: m["scrubIds"] as? Bool,
            mlsVerbosity: verbosity,
            metricsCadenceMs: optionalNonNegativeUInt64(m, "metricsCadenceMs"),
            routingDiagnostic: m["routingDiagnostic"] as? Bool,
            mlsSamplingBypass: m["mlsSamplingBypass"] as? Bool
        )
    }

    /// Counters cross as doubles, as Android's map does; values above 2^53
    /// lose precision, which no device reaches.
    func telemetryStatsMap(_ stats: TelemetryStats) -> [String: Any] {
        var map: [String: Any] = [
            "buffered": Double(stats.buffered),
            "sentEvents": Double(stats.sentEvents),
            "acceptedEvents": Double(stats.acceptedEvents),
            "dropped": Double(stats.dropped),
            "sessionId": stats.sessionId,
        ]
        if let lastError = stats.lastError { map["lastError"] = lastError }
        if let lastFlush = stats.lastFlushAtMs { map["lastFlushAtMs"] = Double(lastFlush) }
        return map
    }

    /// A config number the app supplied, refused if it is negative or not
    /// finite. A negative `NSNumber` does not clamp through `uint64Value`: on
    /// the x86 simulator it wraps to a huge value and an interval flush
    /// silently never fires again.
    private func optionalNonNegativeNumber(_ map: [String: Any], _ key: String) -> Double? {
        guard let number = map[key] as? NSNumber else { return nil }
        let value = number.doubleValue
        guard value.isFinite, value >= 0 else {
            onDiagnostic(
                "warning",
                "telemetry: ignoring value for '\(key)' (\(value)), expected a non-negative number",
                [:]
            )
            return nil
        }
        return value
    }

    /// `Double(UInt64.max)` rounds above `UInt64.max`, so converting it back
    /// traps; clamping well below it costs nothing.
    private func optionalNonNegativeUInt64(_ map: [String: Any], _ key: String) -> UInt64? {
        guard let value = optionalNonNegativeNumber(map, key) else { return nil }
        return UInt64(min(value, Double(UInt64(1) << 62)))
    }

    private func optionalNonNegativeUInt32(_ map: [String: Any], _ key: String) -> UInt32? {
        guard let value = optionalNonNegativeNumber(map, key) else { return nil }
        return UInt32(min(value, Double(UInt32.max)))
    }

    // MARK: - Messaging

    func sendMessage(recipient: String, content: String, priority: Int, replyToMsg: String?) throws -> String {
        try requireProtocol().sendMessage(
            recipient: recipient,
            content: content,
            priority: Self.messagePriority(fromInt: priority),
            replyToMsg: replyToMsg
        )
    }

    func sendMessageRich(recipient: String, content: String, options: SendMessageOptions) throws -> String {
        try requireProtocol().sendMessageRich(recipient: recipient, content: content, options: options)
    }

    func receiveMessage() -> String? {
        protocolInstance?.receiveMessage()
    }

    // MARK: - Data store

    private func requireDataStore() throws -> DataStore {
        guard let store = dataStore else {
            throw HostError.illegalState("DataStore not initialized")
        }
        return store
    }

    func dataCreateDoc(spaceId: String, docId: String) throws {
        try requireDataStore().createDoc(spaceId: spaceId, docId: docId)
    }

    func dataDeleteDoc(spaceId: String, docId: String) throws {
        try requireDataStore().deleteDoc(spaceId: spaceId, docId: docId)
    }

    func dataRemoveDoc(spaceId: String, docId: String) throws {
        try requireDataStore().removeDoc(spaceId: spaceId, docId: docId)
    }

    func dataRemoveSpace(spaceId: String) throws {
        try requireDataStore().removeSpace(spaceId: spaceId)
    }

    func dataFetchAttachmentFrom(spaceId: String, peerId: String, hash: String) throws {
        try requireDataStore().fetchAttachmentFrom(spaceId: spaceId, peerId: peerId, hash: hash)
    }

    func dataSetInterest(spaceId: String, patterns: [String]) throws {
        try requireDataStore().setInterest(spaceId: spaceId, patterns: patterns)
    }

    func dataListDocs(spaceId: String) throws -> String {
        try Self.jsonString(try requireDataStore().listDocs(spaceId: spaceId))
    }

    func dataListSpaces() throws -> String {
        try Self.jsonString(try requireDataStore().listSpaces())
    }

    func dataMapSet(spaceId: String, docId: String, collection: String, key: String, valueJson: String) throws {
        try requireDataStore().mapSet(
            spaceId: spaceId, docId: docId, collection: collection, key: key, valueJson: valueJson
        )
    }

    func dataMapDelete(spaceId: String, docId: String, collection: String, key: String) throws {
        try requireDataStore().mapDelete(spaceId: spaceId, docId: docId, collection: collection, key: key)
    }

    func dataMapGetJson(spaceId: String, docId: String, collection: String, key: String) throws -> String? {
        try requireDataStore().mapGetJson(spaceId: spaceId, docId: docId, collection: collection, key: key)
    }

    func dataListPush(spaceId: String, docId: String, collection: String, valueJson: String) throws {
        try requireDataStore().listPush(
            spaceId: spaceId, docId: docId, collection: collection, valueJson: valueJson
        )
    }

    func dataListDelete(spaceId: String, docId: String, collection: String, index: Double, count: Double) throws {
        try requireDataStore().listDelete(
            spaceId: spaceId,
            docId: docId,
            collection: collection,
            index: try Self.uint32(index, "index"),
            count: try Self.uint32(count, "count")
        )
    }

    func dataListLen(spaceId: String, docId: String, collection: String) throws -> Double {
        Double(try requireDataStore().listLen(spaceId: spaceId, docId: docId, collection: collection))
    }

    func dataTextInsert(spaceId: String, docId: String, collection: String, position: Double, text: String) throws {
        try requireDataStore().textInsert(
            spaceId: spaceId,
            docId: docId,
            collection: collection,
            position: try Self.uint32(position, "position"),
            text: text
        )
    }

    func dataTextDelete(spaceId: String, docId: String, collection: String, position: Double, count: Double) throws {
        try requireDataStore().textDelete(
            spaceId: spaceId,
            docId: docId,
            collection: collection,
            position: try Self.uint32(position, "position"),
            count: try Self.uint32(count, "count")
        )
    }

    func dataTextValue(spaceId: String, docId: String, collection: String) throws -> String {
        try requireDataStore().textValue(spaceId: spaceId, docId: docId, collection: collection)
    }

    func dataCounterIncrement(spaceId: String, docId: String, collection: String, amount: Double) throws {
        try requireDataStore().counterIncrement(
            spaceId: spaceId, docId: docId, collection: collection, amount: amount
        )
    }

    func dataCounterValue(spaceId: String, docId: String, collection: String) throws -> Double {
        try requireDataStore().counterValue(spaceId: spaceId, docId: docId, collection: collection)
    }

    func dataDocJson(spaceId: String, docId: String) throws -> String {
        try requireDataStore().docJson(spaceId: spaceId, docId: docId)
    }

    func dataExportRaw(spaceId: String, docId: String) throws -> Data {
        try requireDataStore().exportRaw(spaceId: spaceId, docId: docId)
    }

    func dataFlush(spaceId: String, docId: String) throws {
        try requireDataStore().flush(spaceId: spaceId, docId: docId)
    }

    func dataFlushAll() throws {
        try requireDataStore().flushAll()
    }

    func dataDocSize(spaceId: String, docId: String) throws -> Double {
        Double(try requireDataStore().docSize(spaceId: spaceId, docId: docId))
    }

    func dataAttachmentHash(bytesBase64: String) throws -> String {
        try requireDataStore().attachmentHash(data: try Self.base64Data(bytesBase64, "bytesBase64"))
    }

    func dataFetchAttachment(spaceId: String, hash: String) throws {
        try requireDataStore().fetchAttachment(spaceId: spaceId, hash: hash)
    }

    func dataProvideAttachment(spaceId: String, peerId: String, hash: String, bytesBase64: String) throws {
        try requireDataStore().provideAttachment(
            spaceId: spaceId,
            peerId: peerId,
            hash: hash,
            data: try Self.base64Data(bytesBase64, "bytesBase64")
        )
    }

    func dataDeclineAttachment(spaceId: String, peerId: String, hash: String) throws {
        try requireDataStore().declineAttachment(spaceId: spaceId, peerId: peerId, hash: hash)
    }

    func dataWipeAll() throws {
        try requireDataStore().wipeAll()
    }

    // MARK: - Mesh / MLS groups

    func meshCreateGroup(name: String) throws -> [String: Any] {
        Self.groupInfoMap(try requireProtocol().createGroup(groupName: name))
    }

    func meshInviteToGroup(groupId: String, inviteeUserId: String) throws {
        try requireProtocol().inviteToGroup(groupId: groupId, inviteeUserId: inviteeUserId)
    }

    func meshSendGroupMessage(
        groupId: String,
        content: String,
        priority: String?,
        replyToMsg: String?
    ) throws -> [String] {
        try requireProtocol().sendGroupMessage(
            groupId: groupId,
            content: content,
            priority: priority.flatMap { Self.messagePriority(fromString: $0) },
            replyToMsg: replyToMsg
        )
    }

    func meshForwardMessageToGroup(
        originalMessageJson: String,
        groupId: String,
        priority: String?
    ) throws -> [String] {
        try requireProtocol().forwardMessageToGroup(
            originalMessageJson: originalMessageJson,
            groupId: groupId,
            priority: priority.flatMap { Self.messagePriority(fromString: $0) }
        )
    }

    func meshRemoveFromGroup(groupId: String, memberId: String) throws {
        try requireProtocol().removeFromGroup(groupId: groupId, memberId: memberId)
    }

    func meshLeaveGroup(groupId: String) throws {
        try requireProtocol().leaveGroup(groupId: groupId)
    }

    func meshListGroups() throws -> [String] {
        try requireProtocol().listGroups()
    }

    func meshGetGroupInfo(groupId: String) throws -> [String: Any]? {
        try requireProtocol().getGroupInfo(groupId: groupId).map { Self.groupInfoMap($0) }
    }

    func meshGroupRichReadiness(groupId: String) throws -> [String: Any] {
        let readiness = try requireProtocol().groupRichReadiness(groupId: groupId)
        return [
            "ready": readiness.ready,
            "unknownMembers": readiness.unknownMembers,
        ]
    }

    func meshGroupRelaySyncState(groupId: String) throws -> String {
        switch try requireProtocol().groupRelaySyncState(groupId: groupId) {
        case .synced: return "synced"
        case .pending: return "pending"
        case .unsynced: return "unsynced"
        }
    }

    func meshRequestGroupRelayRegistration(groupId: String) throws -> Bool {
        try requireProtocol().requestGroupRelayRegistration(groupId: groupId)
    }

    func meshSetMemberRole(groupId: String, userId: String, role: String) throws {
        try requireProtocol().setMemberRole(groupId: groupId, userId: userId, role: role)
    }

    func meshGetMemberRole(groupId: String, userId: String) throws -> String? {
        try requireProtocol().getMemberRole(groupId: groupId, userId: userId)
    }

    func meshGetGroupRoles(groupId: String) throws -> [String: String] {
        try requireProtocol().getGroupRoles(groupId: groupId)
    }

    func meshRenameGroup(groupId: String, newName: String) throws {
        try requireProtocol().renameGroup(groupId: groupId, newName: newName)
    }

    // MARK: - MLS storage

    /// Opens the Keychain-backed MLS store and the app-container protocol
    /// state store for this account and hands both to the core.
    func initializeMlsWithSecureStorage() throws {
        let proto = try requireProtocol()
        guard let config = currentConfig else {
            throw HostError.illegalState("Protocol config not initialized")
        }
        let accountNamespace = StorageNamespace.account(appId: config.appId, profile: config.profile)
        let secureStorage = try MlsSecureStorage(accountNamespace: accountNamespace)
        let protocolStateStorage = try AppContainerProtocolStateStorage(accountNamespace: accountNamespace)
        try proto.initializeMls(secureStorage: secureStorage, protocolStateStorage: protocolStateStorage)
        switch secureStorage.legacyAdoption {
        case .conflict?:
            onDiagnostic(
                "error",
                "Legacy secure store belongs to another account; this account starts from a fresh MLS "
                    + "identity and cannot decrypt its previous sessions. Its pre-split delivery state is "
                    + "unreachable too, so it also comes up with an empty outbox and an empty block list: "
                    + "every previously blocked peer is unblocked",
                [:]
            )
        case .claimUnverified?:
            onDiagnostic(
                "error",
                "Could not record this account's claim on the legacy secure store, so it was not adopted: "
                    + "another account could otherwise inherit the same MLS identity. This account starts "
                    + "from a fresh identity and comes up with an empty outbox and an empty block list: "
                    + "every previously blocked peer is unblocked. The credential store is failing writes; "
                    + "retrying on a healthy store adopts normally",
                [:]
            )
        case .adopt?, .resume?, .none:
            break
        }
        onDiagnostic("info", "MLS initialized with split Keychain and app-container storage", [:])
    }

    // MARK: - Accessors and helpers

    func requireProtocol() throws -> OfflineProtocol {
        guard let proto = protocolInstance else {
            throw HostError.illegalState("Protocol not initialized")
        }
        return proto
    }

    func requireMeshServices() throws -> MeshServices {
        guard let services = meshServices else {
            throw HostError.illegalState("MeshServices not initialized")
        }
        return services
    }

    static func groupInfoMap(_ info: MlsGroupInfo) -> [String: Any] {
        [
            "groupId": info.groupId,
            "name": info.name ?? NSNull(),
            "members": info.members,
            "epoch": Double(info.epoch),
            "isSession": info.isSession,
            "createdAtMs": Double(info.createdAtMs),
            "lastActivityMs": Double(info.lastActivityMs),
        ]
    }

    static func messagePriority(fromInt priority: Int) -> MessagePriority {
        switch priority {
        case 0: return .low
        case 1: return .medium
        case 2: return .high
        case 3: return .critical
        default: return .medium
        }
    }

    static func messagePriority(fromString priority: String) -> MessagePriority? {
        switch priority.lowercased() {
        case "low": return .low
        case "medium": return .medium
        case "high": return .high
        case "critical": return .critical
        default: return nil
        }
    }

    /// A count or position from Dart arrives as a double. Narrowing a negative,
    /// fractional or out-of-range one traps and aborts the app, so it is
    /// refused by name instead.
    static func uint32(_ value: Double, _ field: String) throws -> UInt32 {
        guard value.isFinite, value >= 0, value <= Double(UInt32.max) else {
            throw HostError.invalidArgument("'\(field)' must be a non-negative integer that fits in 32 bits")
        }
        return UInt32(value)
    }

    /// Tolerant of line breaks, as Android's `Base64.DEFAULT` is.
    static func base64Data(_ value: String, _ field: String) throws -> Data {
        guard let data = Data(base64Encoded: value, options: .ignoreUnknownCharacters) else {
            throw HostError.invalidArgument("'\(field)' is not valid base64")
        }
        return data
    }

    static func jsonString(_ array: [String]) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: array, options: [])
        guard let json = String(data: data, encoding: .utf8) else {
            throw HostError.illegalState("Failed to encode JSON")
        }
        return json
    }
}

// MARK: - Transport callbacks

// Each callback holds the host weakly and resolves the manager when it fires,
// so a manager replaced by a later `enableTransport` is the one that is woken
// and the core's callback never keeps a stopped manager alive.

private final class HostBleTransportCallback: BleTransportCallback, @unchecked Sendable {
    private weak var host: ProtocolEngineHost?

    init(host: ProtocolEngineHost) {
        self.host = host
    }

    func onFragmentsAvailable() {
        host?.bleManager?.onFragmentsAvailable()
    }
}

private final class HostWifiDirectTransportCallback: WifiDirectTransportCallback, @unchecked Sendable {
    private weak var host: ProtocolEngineHost?

    init(host: ProtocolEngineHost) {
        self.host = host
    }

    func onMessagesAvailable() {
        host?.wifiDirectManager?.onMessagesAvailable()
    }
}

private final class HostReticulumTransportCallback: ReticulumTransportCallback, @unchecked Sendable {
    private weak var host: ProtocolEngineHost?

    init(host: ProtocolEngineHost) {
        self.host = host
    }

    func onMessagesAvailable() {
        host?.reticulumManager?.onMessagesAvailable()
    }
}

private final class HostNostrTransportCallback: NostrTransportCallback, @unchecked Sendable {
    private weak var host: ProtocolEngineHost?

    init(host: ProtocolEngineHost) {
        self.host = host
    }

    func onMessagesAvailable() {
        host?.nostrManager?.onMessagesAvailable()
    }
}
