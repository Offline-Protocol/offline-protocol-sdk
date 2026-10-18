import Flutter
import UIKit

/// Flutter host for the Offline Protocol engine (core lifecycle only in PR1).
///
/// UniFFI calls run on `engineQueue`; Flutter `result` callbacks run on the main queue (bridge K2).
public class OfflineProtocolPlugin: NSObject, FlutterPlugin {
    private var protocolInstance: OfflineProtocol?
    private var currentConfig: ProtocolConfig?

    private let engineQueue = DispatchQueue(
        label: "com.offlineprotocol.flutter.engine",
        qos: .userInitiated
    )

    public static func register(with registrar: FlutterPluginRegistrar) {
        let channel = FlutterMethodChannel(
            name: "com.offlineprotocol/offline_protocol",
            binaryMessenger: registrar.messenger()
        )
        let instance = OfflineProtocolPlugin()
        registrar.addMethodCallDelegate(instance, channel: channel)
    }

    deinit {
        engineQueue.sync {
            tearDownProtocol()
        }
    }

    public func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
        engineQueue.async { [self] in
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
                default:
                    DispatchQueue.main.async { result(FlutterMethodNotImplemented) }
                }
            } catch {
                DispatchQueue.main.async {
                    result(FlutterError(code: "ERROR", message: error.localizedDescription, details: nil))
                }
            }
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
        proto.setEventCallback(callback: FlutterEventCallback())
        InitialRuntimeConfigApplier.apply(proto: proto, rawConfig: parsed.raw)
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
    }

    private func stopOnEngine() {
        try? protocolInstance?.stop()
    }

    private func tearDownProtocol() {
        try? protocolInstance?.stop()
        protocolInstance?.destroy()
        protocolInstance = nil
        currentConfig = nil
    }
}

private final class FlutterEventCallback: EventCallback, @unchecked Sendable {
    func onEvent(eventJson: String) {
        // EventChannel wiring lands in PR2.
    }
}
