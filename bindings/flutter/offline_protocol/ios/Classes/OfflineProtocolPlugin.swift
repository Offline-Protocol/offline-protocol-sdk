import Flutter
import UIKit

/// Flutter host for the Offline Protocol engine (lifecycle, events, Internet, send).
///
/// UniFFI calls run on `engineQueue`; Flutter callbacks run on the main queue (bridge K2).
public class OfflineProtocolPlugin: NSObject, FlutterPlugin, FlutterStreamHandler {
    private var channel: FlutterMethodChannel?
    private var eventsChannel: FlutterEventChannel?
    private var protocolInstance: OfflineProtocol?
    private var currentConfig: ProtocolConfig?
    private var rawConfig: [String: Any] = [:]
    private var storedInternetConfig = InternetTransportBootstrap.StoredInternetConfig()
    private var internetManager: InternetManager?

    private let engineQueue = DispatchQueue(
        label: "com.offlineprotocol.flutter.engine",
        qos: .userInitiated
    )
    private let attachmentLock = NSLock()
    private var isPluginAttached = false

    private let eventSeqLock = NSLock()
    private var eventSeq = 0
    private var eventSink: FlutterEventSink?

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

        eventsChannel?.setStreamHandler(nil)
        eventSink = nil
        channel?.setMethodCallHandler(nil)
        engineQueue.sync {
            tearDownProtocol()
        }
    }

    deinit {
        engineQueue.sync {
            tearDownProtocol()
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
                    try self.enableInternetTransportOnEngine(call.arguments)
                    DispatchQueue.main.async { result(nil) }
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
        rawConfig = parsed.raw
        storedInternetConfig = InternetTransportBootstrap.readStoredConfig(raw: parsed.raw)
        if parsed.config.internetEnabled {
            let manager = InternetManager(
                protocol: proto,
                deviceId: parsed.config.profile,
                appId: parsed.config.appId
            )
            InternetTransportBootstrap.attachEmitters(manager: manager, emitEventJson: { [weak self] json in
                self?.emitEventJson(json)
            })
            internetManager = manager
        }
        protocolInstance = proto
        currentConfig = parsed.config
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
        if let manager = internetManager {
            do {
                try InternetTransportBootstrap.startIfConfigured(
                    manager: manager,
                    stored: storedInternetConfig
                )
            } catch {
                NSLog("OfflineProtocolPlugin: Internet transport failed to start (core is running): \(error)")
            }
        }
    }

    private func stopOnEngine() {
        try? internetManager?.stop()
        try? protocolInstance?.stop()
    }

    private func enableInternetTransportOnEngine(_ arguments: Any?) throws {
        guard let configJson = arguments as? String,
              let data = configJson.data(using: .utf8),
              let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "enableInternetTransport expects a JSON string"]
            )
        }
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
        if internetManager == nil {
            let manager = InternetManager(protocol: proto, deviceId: config.profile, appId: config.appId)
            InternetTransportBootstrap.attachEmitters(manager: manager, emitEventJson: { [weak self] json in
                self?.emitEventJson(json)
            })
            internetManager = manager
        }
        guard let manager = internetManager else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Failed to create Internet manager"]
            )
        }
        try InternetTransportBootstrap.configureAndStart(manager: manager, config: object)
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
        try? internetManager?.stop()
        internetManager = nil
        storedInternetConfig = InternetTransportBootstrap.StoredInternetConfig()
        rawConfig = [:]
        eventSeqLock.lock()
        eventSeq = 0
        eventSeqLock.unlock()
        try? protocolInstance?.stop()
        protocolInstance?.destroy()
        protocolInstance = nil
        currentConfig = nil
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
