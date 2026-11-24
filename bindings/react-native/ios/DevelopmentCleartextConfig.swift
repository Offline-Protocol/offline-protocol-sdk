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
    /// `forScenario(\"developmentCleartext\")` reaches after `create()`:
    /// encryption off, `requireEncryption` off, and sibling MLS flags off.
    public static func make(appLabel: String, profile: String) -> ProtocolConfig {
        ProtocolConfig(
            appLabel: appLabel,
            profile: profile,
            bleEnabled: true,
            wifiDirectEnabled: false,
            internetEnabled: false,
            reticulumEnabled: false,
            nostrEnabled: false,
            preferOnline: false,
            initialTtl: 8,
            encryptionEnabled: false,
            autoKeyExchange: false,
            storePending: false,
            requireEncryption: false,
            maxPendingPerPeer: 64,
            maxPendingGlobal: 4_096,
            pendingTtlMs: 86_400_000,
            overflowPolicy: .dropOldest
        )
    }

    /// The old name of `make(appLabel:profile:)`.
    @available(*, deprecated, message: "appId on the mesh is now appLabel. Your App ID (app_…) from the dashboard is what turns on telemetry")
    public static func make(appId: String, profile: String) -> ProtocolConfig {
        make(appLabel: appId, profile: profile)
    }
}

/// One-shot remediation when fail-closed encryption rejects a send with
/// `EncryptFailed` because MLS is not initialized. Hosts that call UniFFI
/// directly use this in a `catch`, passing `mesh.isMlsInitialized()`;
/// `ProtocolEngineHost` wires the same text through `IntegratorLifecycleHints`.
public enum OfflineProtocolEncryptFailedDiagnostics {
    private static let lock = NSLock()
    private static var warned = false

    /// Resets the once-per-process guard (tests only).
    static func resetForTests() {
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

    /// Logs `remedyMessage(context:)` at most once per process, for an
    /// `EncryptFailed` with MLS not initialized. With `mlsInitialized` true the
    /// error has another cause (a session encrypt failure, a transfer that
    /// cannot leave unsealed) and nothing is logged. Call it only from an app on
    /// the default encrypted config: with the lab config, an `EncryptFailed`
    /// comes from a feature that is sealed or not sent at all, and this text
    /// would name the wrong fix.
    public static func logFirstRemedyIfNeeded(
        context: String,
        error: Error,
        mlsInitialized: Bool,
        sink: (String) -> Void
    ) {
        guard !mlsInitialized, isEncryptFailed(error) else { return }
        lock.lock()
        let first = !warned
        if first { warned = true }
        lock.unlock()
        if first { sink(remedyMessage(context: context)) }
    }
}
