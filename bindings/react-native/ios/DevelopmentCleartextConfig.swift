//
// DevelopmentCleartextConfig.swift
// OfflineProtocol
//
// Lab-only configuration matching React Native's `developmentCleartext`
// preset (`ConfigScenarios.forScenario`). Production builds must keep the
// default fail-closed encryption path.
//

import Foundation

/// Native counterpart to the React Native `developmentCleartext` scenario:
/// BLE on, every other transport off, encryption fully disabled so sends work
/// without MLS. **Never ship this to end users.**
public enum OfflineProtocolDevelopmentCleartextConfig {

    /// Documented in `docs/native-development-cleartext.md` and
    /// `docs/react-native-config-presets.md`.
    public static let labOnlyNotice =
        "OfflineProtocolDevelopmentCleartextConfig is for lab / simulator smoke "
        + "tests only. Production builds must use default encryption and "
        + "OfflineProtocolReferenceStorage.initializeMls before start()."

    /// Builds a `ProtocolConfig` with the same encryption posture React Native's
    /// `forScenario(\"developmentCleartext\")` reaches after `create()`.
    public static func make(appId: String, profile: String) -> ProtocolConfig {
        ProtocolConfig(
            appId: appId,
            profile: profile,
            bleEnabled: true,
            wifiDirectEnabled: false,
            internetEnabled: false,
            reticulumEnabled: false,
            nostrEnabled: false,
            preferOnline: false,
            initialTtl: 8,
            encryptionEnabled: false,
            autoKeyExchange: true,
            storePending: true,
            requireEncryption: false,
            maxPendingPerPeer: 100,
            maxPendingGlobal: 1_000,
            pendingTtlMs: 86_400_000,
            overflowPolicy: .dropOldest
        )
    }
}

/// One-shot remediation when fail-closed encryption rejects a send with
/// `EncryptFailed`. Hosts that call UniFFI directly use this in a `catch`;
/// `ProtocolEngineHost` wires the same text through `IntegratorLifecycleHints`.
public enum OfflineProtocolEncryptFailedDiagnostics {
    private static let lock = NSLock()
    private static var warned = false

    /// Resets the once-per-process guard (tests only).
    public static func resetForTests() {
        lock.lock()
        warned = false
        lock.unlock()
    }

    public static func isEncryptFailed(_ error: Error) -> Bool {
        guard let protocolError = error as? ProtocolError else { return false }
        if case .EncryptFailed = protocolError { return true }
        return false
    }

    public static func remedyMessage(context: String) -> String {
        IntegratorEncryptionRemedy.message(context: context)
    }

    /// Logs `remedyMessage(context)` at most once for any `EncryptFailed`.
    public static func logFirstRemedyIfNeeded(context: String, error: Error, sink: (String) -> Void) {
        guard isEncryptFailed(error) else { return }
        lock.lock()
        let first = !warned
        if first { warned = true }
        lock.unlock()
        if first { sink(remedyMessage(context: context)) }
    }
}
