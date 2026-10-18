import Flutter
import UIKit

/// Flutter host for the Offline Protocol engine (core lifecycle only in PR1).
public class OfflineProtocolPlugin: NSObject, FlutterPlugin {
    private var protocolInstance: OfflineProtocol?
    private var currentConfig: ProtocolConfig?

    public static func register(with registrar: FlutterPluginRegistrar) {
        let channel = FlutterMethodChannel(
            name: "com.offlineprotocol/offline_protocol",
            binaryMessenger: registrar.messenger()
        )
        let instance = OfflineProtocolPlugin()
        registrar.addMethodCallDelegate(instance, channel: channel)
    }

    public func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
        do {
            switch call.method {
            case "create":
                try create(call.arguments, result: result)
            case "initializeMlsWithSecureStorage":
                try initializeMls(result: result)
            case "start":
                try start(result: result)
            case "stop":
                stop(result: result)
            case "destroy":
                destroy(result: result)
            case "localAddress":
                localAddress(result: result)
            default:
                result(FlutterMethodNotImplemented)
            }
        } catch {
            result(FlutterError(code: "ERROR", message: error.localizedDescription, details: nil))
        }
    }

    private func create(_ arguments: Any?, result: @escaping FlutterResult) throws {
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
        protocolInstance = proto
        currentConfig = parsed.config
        result(nil)
    }

    private func initializeMls(result: @escaping FlutterResult) throws {
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
        result(nil)
    }

    private func start(result: @escaping FlutterResult) throws {
        guard let proto = protocolInstance else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"]
            )
        }
        try proto.start()
        result(nil)
    }

    private func stop(result: @escaping FlutterResult) {
        try? protocolInstance?.stop()
        result(nil)
    }

    private func destroy(result: @escaping FlutterResult) {
        tearDownProtocol()
        result(nil)
    }

    private func localAddress(result: @escaping FlutterResult) {
        result(protocolInstance?.localAddress())
    }

    private func tearDownProtocol() {
        try? protocolInstance?.stop()
        protocolInstance?.destroy()
        protocolInstance = nil
        currentConfig = nil
    }
}

private final class FlutterEventCallback: EventCallback {
    func onEvent(eventJson: String) {
        // EventChannel wiring lands in PR2.
    }
}
