import Flutter
import UIKit

/// Flutter host for the Offline Protocol engine (lifecycle, events, transports).
///
/// UniFFI calls run on `engineQueue`; Flutter callbacks run on the main queue (bridge K2).
public class OfflineProtocolPlugin: NSObject, FlutterPlugin, FlutterStreamHandler {
    private var channel: FlutterMethodChannel?
    private var eventsChannel: FlutterEventChannel?
    private var protocolInstance: OfflineProtocol?
    private var currentConfig: ProtocolConfig?
    private var internetManager: InternetManager?
    private var bleManager: BleManager?
    private var wifiDirectManager: WifiDirectManager?

    private let engineQueue = DispatchQueue(
        label: "com.offlineprotocol.flutter.engine",
        qos: .userInitiated
    )
    private let attachmentLock = NSLock()
    private var isPluginAttached = false

    private let eventSeqLock = NSLock()
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

    @objc
    public func detachFromEngineForRegistrar(_ registrar: FlutterPluginRegistrar) {
        attachmentLock.lock()
        guard isPluginAttached else {
            attachmentLock.unlock()
            return
        }
        isPluginAttached = false
        attachmentLock.unlock()

        removeForegroundObserver()
        eventsChannel?.setStreamHandler(nil)
        eventSink = nil
        channel?.setMethodCallHandler(nil)
        engineQueue.sync {
            tearDownProtocol()
        }
    }

    deinit {
        removeForegroundObserver()
        engineQueue.sync {
            tearDownProtocol()
        }
    }

    private func installForegroundObserver() {
        let center = NotificationCenter.default
        backgroundObserver = center.addObserver(
            forName: UIApplication.didEnterBackgroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.engineQueue.async {
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
            self?.engineQueue.async {
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
            try? internetManager?.forceReconnect()
        }
    }

    public func onListen(withArguments arguments: Any?, eventSink events: @escaping FlutterEventSink) -> FlutterError? {
        eventSink = events
        return nil
    }

    public func onCancel(withArguments arguments: Any?) -> FlutterError? {
        eventSink = nil
        return nil
    }

    public func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
        guard isAttached() else {
            postDetachedError(to: result)
            return
        }
        engineQueue.async { [self] in
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
                    self.stopOnEngine()
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
        guard let sink = eventSink else { return }
        eventSeqLock.lock()
        let seq = eventSeq
        eventReplay.record(seq: seq, eventJson: eventJson)
        eventSeq += 1
        eventSeqLock.unlock()
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
        tearDownProtocol()
        let parsed = try ProtocolConfigParser.parse(configJson)
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

    private func wireWifiDirectCallbackIfNeeded(proto: OfflineProtocol) {
        guard currentConfig?.wifiDirectEnabled == true else { return }
        proto.setWifiDirectTransportCallback(
            callback: FlutterWifiDirectTransportCallback(wifiDirectManager: wifiDirectManager)
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

    private func stopOnEngine() {
        try? bleManager?.stop()
        try? internetManager?.stop()
        try? wifiDirectManager?.stop()
        try? protocolInstance?.stop()
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
        try? bleManager?.stop()
        bleManager = nil
        try? internetManager?.stop()
        internetManager = nil
        try? wifiDirectManager?.stop()
        wifiDirectManager = nil
        try? protocolInstance?.stop()
        protocolInstance?.destroy()
        protocolInstance = nil
        currentConfig = nil
        eventSeqLock.lock()
        eventSeq = 0
        eventReplay.clear()
        eventSeqLock.unlock()
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
