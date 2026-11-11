//
// Development-only ordering hints for native ProtocolEngineHost integrators
// (Flutter plugin, direct Swift hosts). React Native disables these at the
// host constructor because the TypeScript layer already warns (G3 PR1).
//

import Foundation

enum IntegratorLifecycleHints {
    private static var warnedKeys = Set<String>()
    private static let docPointer =
        "docs/integrator-map.md (React Native lifecycle ordering) and the iOS/Android integration guides"

    static func resetSession() {
        warnedKeys.removeAll()
    }

    private static func warnOnce(key: String, message: String, enabled: Bool) {
        guard enabled else { return }
        guard !warnedKeys.contains(key) else { return }
        warnedKeys.insert(key)
        print("[OfflineProtocol] \(message)")
    }

    static func warnIfCallBeforeEngineStarted(
        context: String,
        proto: OfflineProtocol?,
        enabled: Bool
    ) {
        guard enabled, let proto else { return }
        if proto.getState() == .running { return }
        warnOnce(
            key: "engine:\(context)",
            message:
                "\(context): call start() on the protocol core before this API. "
                + "Install setEventCallback before start(). See \(docPointer).",
            enabled: true
        )
    }

    static func warnIfDataMutationBeforeEngine(
        context: String,
        proto: OfflineProtocol?,
        enabled: Bool
    ) {
        guard enabled, let proto else { return }
        if proto.getState() == .running { return }
        warnOnce(
            key: "data:\(context)",
            message:
                "\(context): the data store is not open yet. "
                + "Call initializeMlsWithSecureStorage and start() first, or declare "
                + "setInterest before start() for replication policy.",
            enabled: true
        )
    }

    static func warnIfInterestAfterStart(enabled: Bool) {
        guard enabled else { return }
        warnOnce(
            key: "interest:after-start",
            message:
                "setInterest after start() can only widen replication; "
                + "narrowing does not delete documents already pulled during start-up. "
                + "Declare interest before start() when you need a tight filter from the first exchange.",
            enabled: true
        )
    }

    static func warnIfStartWithoutMls(
        encryptionEnabled: Bool,
        proto: OfflineProtocol?,
        enabled: Bool
    ) {
        guard enabled, encryptionEnabled, let proto else { return }
        if proto.isMlsInitialized() { return }
        warnOnce(
            key: "start:mls",
            message:
                "start() with encryption enabled before initializeMlsWithSecureStorage(). "
                + "Secure send and handshake will not work until MLS storage is opened.",
            enabled: true
        )
    }

    static func warnIfSendWithoutMls(
        context: String,
        encryptionEnabled: Bool,
        proto: OfflineProtocol?,
        enabled: Bool
    ) {
        guard enabled, encryptionEnabled, let proto else { return }
        if proto.getState() != .running { return }
        if proto.isMlsInitialized() { return }
        warnOnce(
            key: "mls:\(context)",
            message:
                "\(context): encryption is enabled but MLS is not initialized. "
                + "Run initializeMlsWithSecureStorage() successfully before send.",
            enabled: true
        )
    }
}
