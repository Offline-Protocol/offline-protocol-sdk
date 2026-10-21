import Flutter
import UIKit

/// Flutter host for the Offline Protocol engine (lifecycle, events, transports).
///
/// UniFFI calls run on `engineQueue`; Flutter callbacks run on the main queue (bridge K2).
/// Nothing on the main thread waits for the engine queue: engine stop can block on
/// transport I/O, so detach queues the teardown and returns.
public class OfflineProtocolPlugin: NSObject, FlutterPlugin, FlutterStreamHandler {
    private var channel: FlutterMethodChannel?
    private var eventsChannel: FlutterEventChannel?
    private var protocolInstance: OfflineProtocol?
    private var currentConfig: ProtocolConfig?
    private var internetManager: InternetManager?
    private var bleManager: BleManager?
    private var wifiDirectManager: WifiDirectManager?

    /// One serial queue for every plugin instance in the process. Flutter
    /// registers a new instance for every engine, so a queue per instance would
    /// let a new engine's `create` run while the previous engine's queued
    /// teardown is still blocked on transport I/O: two cores under one identity,
    /// both on BLE and the relay. Shared, the teardown always runs first.
    private static let engineQueue = DispatchQueue(
        label: "com.offlineprotocol.flutter.engine",
        qos: .userInitiated
    )
    private let attachmentLock = NSLock()
    private var isPluginAttached = false

    /// Guards `eventSink`, `eventSeq` and the replay ring together. The sink is
    /// written on the main thread and read on the UniFFI callback thread, and an
    /// unsynchronised read of a reference-typed property is a memory-safety race.
    private let eventLock = NSLock()
    private var eventSeq = 0
    private let eventReplay = EventReplayRing()
    private var eventSink: FlutterEventSink?

    private let foregroundReconnectPolicy = ForegroundReconnectPolicy()
    private var foregroundObserver: NSObjectProtocol?
    private var backgroundObserver: NSObjectProtocol?

    public static func register(with registrar: FlutterPluginRegistrar) {
        let instance = OfflineProtocolPlugin()
        let channel = FlutterMethodChannel(
            name: "com.offlineprotocol/offline_protocol",
            binaryMessenger: registrar.messenger()
        )
        let events = FlutterEventChannel(
            name: "com.offlineprotocol/offline_protocol_events",
            binaryMessenger: registrar.messenger()
        )
        instance.channel = channel
        instance.eventsChannel = events
        registrar.addMethodCallDelegate(instance, channel: channel)
        events.setStreamHandler(instance)
        registrar.publish(instance)
        instance.setAttached(true)
        instance.installForegroundObserver()
    }

    public func detachFromEngine(for registrar: FlutterPluginRegistrar) {
        attachmentLock.lock()
        guard isPluginAttached else {
            attachmentLock.unlock()
            return
        }
        isPluginAttached = false
        attachmentLock.unlock()

        removeForegroundObserver()
        eventsChannel?.setStreamHandler(nil)
        setEventSink(nil)
        channel?.setMethodCallHandler(nil)
        // Queued behind every call already submitted, never waited on. The block
        // holds `self` until the teardown has run.
        Self.engineQueue.async { [self] in
            self.tearDownProtocol()
        }
    }

    deinit {
        removeForegroundObserver()
        // Directly, never `engineQueue.sync`: every block on that queue holds
        // `self` strongly or weakly, so deinit can run on the engine queue
        // itself, where a sync onto it deadlocks. No block can be running
        // against this instance by the time it is deallocating.
        tearDownProtocol()
    }

    private func installForegroundObserver() {
        let center = NotificationCenter.default
        backgroundObserver = center.addObserver(
            forName: UIApplication.didEnterBackgroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            OfflineProtocolPlugin.engineQueue.async {
                self?.foregroundReconnectPolicy.didEnterBackground(
                    nowMs: MonotonicClock.nowMs()
                )
            }
        }
        foregroundObserver = center.addObserver(
            forName: UIApplication.didBecomeActiveNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            OfflineProtocolPlugin.engineQueue.async {
                self?.handleAppBecameActive()
            }
        }
    }

    private func removeForegroundObserver() {
        let center = NotificationCenter.default
        if let token = foregroundObserver {
            center.removeObserver(token)
            foregroundObserver = nil
        }
        if let token = backgroundObserver {
            center.removeObserver(token)
            backgroundObserver = nil
        }
    }

    private func handleAppBecameActive() {
        if foregroundReconnectPolicy.shouldReconnectOnForeground(nowMs: MonotonicClock.nowMs()) {
            internetManager?.forceReconnect()
        }
    }

    public func onListen(withArguments arguments: Any?, eventSink events: @escaping FlutterEventSink) -> FlutterError? {
        setEventSink(events)
        return nil
    }

    public func onCancel(withArguments arguments: Any?) -> FlutterError? {
        setEventSink(nil)
        return nil
    }

    private func setEventSink(_ sink: FlutterEventSink?) {
        eventLock.lock()
        eventSink = sink
        eventLock.unlock()
    }

    public func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
        guard isAttached() else {
            postDetachedError(to: result)
            return
        }
        Self.engineQueue.async { [self] in
            guard self.isAttached() else {
                self.postDetachedError(to: result)
                return
            }
            do {
                switch call.method {
                case "create":
                    try self.createOnEngine(call.arguments)
                    DispatchQueue.main.async { result(nil) }
                case "initializeMlsWithSecureStorage":
                    try self.initializeMlsOnEngine()
                    DispatchQueue.main.async { result(nil) }
                case "start":
                    try self.startOnEngine()
                    DispatchQueue.main.async { result(nil) }
                case "stop":
                    try self.stopOnEngine()
                    DispatchQueue.main.async { result(nil) }
                case "destroy":
                    self.tearDownProtocol()
                    DispatchQueue.main.async { result(nil) }
                case "localAddress":
                    let address = self.protocolInstance?.localAddress()
                    DispatchQueue.main.async { result(address) }
                case "sendMessage":
                    let messageId = try self.sendMessageOnEngine(call.arguments)
                    DispatchQueue.main.async { result(messageId) }
                case "enableInternetTransport":
                    try self.enableTransportOnEngine(type: "internet", configJson: call.arguments as? String)
                    DispatchQueue.main.async { result(nil) }
                case "enableTransport":
                    guard let map = call.arguments as? [String: Any],
                          let type = map["type"] as? String else {
                        throw NSError(
                            domain: "OfflineProtocol",
                            code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "enableTransport expects type and config"]
                        )
                    }
                    let configJson = map["config"] as? String
                    try self.enableTransportOnEngine(type: type, configJson: configJson)
                    DispatchQueue.main.async { result(nil) }
                case "replayEvents":
                    guard let map = call.arguments as? [String: Any],
                          let fromSeq = (map["fromSeq"] as? NSNumber)?.intValue,
                          let toSeq = (map["toSeq"] as? NSNumber)?.intValue else {
                        throw NSError(
                            domain: "OfflineProtocol",
                            code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "replayEvents expects fromSeq and toSeq"]
                        )
                    }
                    let replayed = self.eventReplay.replay(fromSeq: fromSeq, toSeq: toSeq)
                    let payload = replayed.map {
                        ["seq": $0.seq, "eventJson": $0.eventJson] as [String: Any]
                    }
                    DispatchQueue.main.async { result(payload) }
                default:
                    DispatchQueue.main.async { result(FlutterMethodNotImplemented) }
                }
            } catch {
                DispatchQueue.main.async {
                    if let mapped = mapProtocolBridgeError(error) {
                        result(FlutterError(code: mapped.code, message: mapped.message, details: nil))
                    } else {
                        result(FlutterError(code: "ERROR", message: error.localizedDescription, details: nil))
                    }
                }
            }
        }
    }

    private func isAttached() -> Bool {
        attachmentLock.lock()
        defer { attachmentLock.unlock() }
        return isPluginAttached
    }

    private func setAttached(_ attached: Bool) {
        attachmentLock.lock()
        isPluginAttached = attached
        attachmentLock.unlock()
    }

    private func postDetachedError(to result: @escaping FlutterResult) {
        DispatchQueue.main.async {
            result(FlutterError(code: "ERROR", message: "Plugin detached", details: nil))
        }
    }

    private func emitEventJson(_ eventJson: String) {
        eventLock.lock()
        guard let sink = eventSink else {
            eventLock.unlock()
            return
        }
        let seq = eventSeq
        eventReplay.record(seq: seq, eventJson: eventJson)
        eventSeq += 1
        eventLock.unlock()
        DispatchQueue.main.async {
            sink([
                "eventJson": eventJson,
                "seq": seq,
            ])
        }
    }

    private func createOnEngine(_ arguments: Any?) throws {
        guard let configJson = arguments as? String else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "create expects a JSON string"]
            )
        }
        // Validated before the running engine is torn down: a config this
        // plugin refuses must leave the current session as it was.
        let parsed = try ProtocolConfigParser.parse(configJson)
        try refuseUnhostedTransports(parsed.config)
        tearDownProtocol()
        let proto = try OfflineProtocol(config: parsed.config)
        proto.setEventCallback(callback: FlutterEventCallback(emit: { [weak self] json in
            self?.emitEventJson(json)
        }))
        InitialRuntimeConfigApplier.apply(proto: proto, rawConfig: parsed.raw)
        protocolInstance = proto
        currentConfig = parsed.config

        if parsed.config.bleEnabled {
            let manager = BleManager(
                protocol: proto,
                deviceId: parsed.config.profile,
                appId: parsed.config.appId
            )
            bleManager = manager
            proto.setBleTransportCallback(callback: FlutterBleTransportCallback(bleManager: manager))
        }
        wireWifiDirectCallbackIfNeeded(proto: proto)
    }

    /// Refuses a config that enables a transport this plugin has no manager for.
    /// Accepted, the core would route to a transport nothing serves and messages
    /// would wait on it with no error anywhere.
    private func refuseUnhostedTransports(_ config: ProtocolConfig) throws {
        var unhosted: [String] = []
        if config.reticulumEnabled { unhosted.append("reticulumEnabled") }
        if config.nostrEnabled { unhosted.append("nostrEnabled") }
        guard unhosted.isEmpty else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [
                    NSLocalizedDescriptionKey:
                        "\(unhosted.joined(separator: ", ")) is not supported by the Flutter plugin yet; set it to false",
                ]
            )
        }
    }

    private func wireWifiDirectCallbackIfNeeded(proto: OfflineProtocol) {
        guard currentConfig?.wifiDirectEnabled == true,
              let manager = wifiDirectManager else { return }
        proto.setWifiDirectTransportCallback(
            callback: FlutterWifiDirectTransportCallback(wifiDirectManager: manager)
        )
    }

    private func initializeMlsOnEngine() throws {
        guard let proto = protocolInstance else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"]
            )
        }
        guard let config = currentConfig else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Protocol config not initialized"]
            )
        }
        let accountNamespace = StorageNamespace.account(
            appId: config.appId,
            profile: config.profile
        )
        let secureStorage = try MlsSecureStorage(accountNamespace: accountNamespace)
        let protocolStateStorage = try AppContainerProtocolStateStorage(
            accountNamespace: accountNamespace
        )
        try proto.initializeMls(
            secureStorage: secureStorage,
            protocolStateStorage: protocolStateStorage
        )
    }

    private func startOnEngine() throws {
        guard let proto = protocolInstance else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"]
            )
        }
        try proto.start()
        if let manager = bleManager, manager.state != .running {
            try manager.start()
        }
    }

    /// The transports stop first and cannot fail; the core's failure reaches
    /// Dart, as Android's `TeardownSequence` reports the first failure.
    private func stopOnEngine() throws {
        bleManager?.stop()
        internetManager?.stop()
        wifiDirectManager?.stop()
        try protocolInstance?.stop()
    }

    private func enableTransportOnEngine(type: String, configJson: String?) throws {
        guard let proto = protocolInstance else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"]
            )
        }
        guard let config = currentConfig else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Protocol config not initialized"]
            )
        }
        let normalized = type.lowercased()
        switch normalized {
        case "internet":
            var object: [String: Any] = [:]
            if let configJson,
               let data = configJson.data(using: .utf8),
               let parsed = try JSONSerialization.jsonObject(with: data) as? [String: Any] {
                object = parsed
            }
            let manager = try ensureInternetManager(proto: proto, config: config)
            if manager.state == .running || manager.state == .starting {
                manager.stop()
            }
            try InternetTransportBootstrap.configureAndStart(manager: manager, config: object)
        case "wifidirect", "wifi_direct":
            if wifiDirectManager == nil {
                let manager = WifiDirectManager(protocol: proto, deviceId: config.profile)
                wifiDirectManager = manager
                proto.setWifiDirectTransportCallback(
                    callback: FlutterWifiDirectTransportCallback(wifiDirectManager: manager)
                )
            }
            guard let manager = wifiDirectManager else {
                throw NSError(
                    domain: "OfflineProtocol",
                    code: -1,
                    userInfo: [NSLocalizedDescriptionKey: "Failed to create Wi-Fi Direct manager"]
                )
            }
            if manager.state == .running || manager.state == .starting {
                manager.stop()
            }
            try manager.start()
        case "ble":
            if bleManager == nil {
                let manager = BleManager(
                    protocol: proto,
                    deviceId: config.profile,
                    appId: config.appId
                )
                bleManager = manager
                proto.setBleTransportCallback(callback: FlutterBleTransportCallback(bleManager: manager))
            }
            guard let manager = bleManager else {
                throw NSError(
                    domain: "OfflineProtocol",
                    code: -1,
                    userInfo: [NSLocalizedDescriptionKey: "Failed to create BLE manager"]
                )
            }
            if manager.state != .running {
                try manager.start()
            }
        case "reticulum", "nostr":
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [
                    NSLocalizedDescriptionKey:
                        "Transport \(type) is not compiled into the Flutter plugin yet",
                ]
            )
        default:
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Unsupported transport type: \(type)"]
            )
        }
    }

    private func ensureInternetManager(
        proto: OfflineProtocol,
        config: ProtocolConfig
    ) throws -> InternetManager {
        if let manager = internetManager {
            return manager
        }
        let manager = InternetManager(
            protocol: proto,
            deviceId: config.profile,
            appId: config.appId
        )
        InternetTransportBootstrap.attachEmitters(manager: manager, emitEventJson: { [weak self] json in
            self?.emitEventJson(json)
        })
        internetManager = manager
        return manager
    }

    private func sendMessageOnEngine(_ arguments: Any?) throws -> String {
        guard let map = arguments as? [String: Any] else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "sendMessage expects a map"]
            )
        }
        guard let recipient = map["recipient"] as? String else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "sendMessage requires recipient"]
            )
        }
        guard let content = map["content"] as? String else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "sendMessage requires content"]
            )
        }
        let priorityValue = (map["priority"] as? NSNumber)?.intValue ?? 1
        let replyTo = map["replyToMsg"] as? String
        let msgPriority: MessagePriority
        switch priorityValue {
        case 0: msgPriority = .low
        case 1: msgPriority = .medium
        case 2: msgPriority = .high
        case 3: msgPriority = .critical
        default: msgPriority = .medium
        }
        guard let proto = protocolInstance else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"]
            )
        }
        return try proto.sendMessage(
            recipient: recipient,
            content: content,
            priority: msgPriority,
            replyToMsg: replyTo
        )
    }

    private func tearDownProtocol() {
        bleManager?.stop()
        bleManager = nil
        internetManager?.stop()
        internetManager = nil
        wifiDirectManager?.stop()
        wifiDirectManager = nil
        try? protocolInstance?.stop()
        protocolInstance = nil
        currentConfig = nil
        eventLock.lock()
        eventSeq = 0
        eventReplay.clear()
        eventLock.unlock()
    }
}

private final class FlutterEventCallback: EventCallback, @unchecked Sendable {
    private let emit: (String) -> Void

    init(emit: @escaping (String) -> Void) {
        self.emit = emit
    }

    func onEvent(eventJson: String) {
        emit(eventJson)
    }
}
