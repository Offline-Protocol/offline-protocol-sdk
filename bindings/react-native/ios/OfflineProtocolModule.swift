//
// OfflineProtocolModule.swift
// OfflineProtocol
//
// UniFFI-based implementation
//

import Foundation
import UIKit
import CoreBluetooth
import React

// Import generated UniFFI bindings
// The generated file is in ios/Generated/offline_protocol.swift
// We need to ensure it's included in the Xcode project

// Import the UniFFI generated code
// This should be available since offline_protocol.swift is included in the project
// If this import fails, it means the UniFFI bindings aren't properly included

@objc(OfflineProtocolModule)
class OfflineProtocolModule: RCTEventEmitter {
    private var protocolInstance: OfflineProtocol?
    private var meshServicesInstance: MeshServices?
    private var bleManager: BleManager?
    private var internetManager: InternetManager?
    private var wifiDirectManager: WifiDirectManager?
    private var reticulumManager: ReticulumManager?
    private var nostrManager: NostrManager?

    /// Whether React Native currently has a JS subscription on this module.
    ///
    /// Lock-guarded rather than a plain `Bool` because it is written from
    /// `startObserving`/`stopObserving` — delivered on RCTEventEmitter's queue —
    /// and read from wherever an event happens to originate, which for
    /// `emitInternetSupersededEvent` is InternetManager's socket callback. Those
    /// share no lock and no happens-before edge, so an unguarded `Bool` may be
    /// held in a register and read stale, dropping the emit on a `false` that
    /// was never true. That matters here because `internet_session_superseded`
    /// is *one-shot*: InternetManager latches the transport stopped and refuses
    /// auto- and force-reconnect until an explicit `start()`, so nothing ever
    /// restates it and the app is left showing a relay connection that is never
    /// coming back. Mirrors the Android module's `listenerCount` AtomicInteger,
    /// which exists for the identical hazard.
    ///
    /// A correct read is all iOS does here — there is no sticky hold, because
    /// `mesh_stopped_by_user` (the event that motivated one) has no iOS
    /// counterpart. See docs/react-native-integration.md §6.1.
    private let listenerLock = NSLock()
    private var _hasListeners = false
    private var hasListeners: Bool {
        get {
            listenerLock.lock()
            defer { listenerLock.unlock() }
            return _hasListeners
        }
        set {
            listenerLock.lock()
            defer { listenerLock.unlock() }
            _hasListeners = newValue
        }
    }

    private let processQueue = DispatchQueue(label: "offlineprotocol.processor")
    private var processTimer: DispatchSourceTimer?
    /// The deferred `bleStatusChanged(true)` backup call, held so `destroy` can
    /// cancel it. An uncancelled one re-enters the protocol up to a second after
    /// the module was torn down.
    private var bleStatusBackupWorkItem: DispatchWorkItem?
    private var currentConfig: ProtocolConfig?
    private var internetServerUrl: String?
    private var internetAutoReconnect: Bool = true
    
    override init() {
        print("[OfflineProtocolModule] init() called")
        super.init()
        addBackgroundObservers()
        print("[OfflineProtocolModule] init() completed successfully")
    }
    
    deinit {
        removeBackgroundObservers()
        stopProcessTimer()
        bleManager?.stop()
        bleManager = nil
        internetManager?.stop()
        internetManager = nil
        wifiDirectManager?.stop()
        wifiDirectManager = nil
        reticulumManager?.stop()
        reticulumManager = nil
        nostrManager?.stop()
        nostrManager = nil
        protocolInstance = nil
    }
    
    // MARK: - iOS background / Wi‑Fi suspension
    
    /// When the app enters background, iOS kills MultipeerConnectivity. Notify Rust so DORS
    /// stops routing over Wi‑Fi Direct and uses BLE (allowed in background).
    private func addBackgroundObservers() {
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(applicationDidEnterBackground),
            name: UIApplication.didEnterBackgroundNotification,
            object: nil
        )
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(applicationWillEnterForeground),
            name: UIApplication.willEnterForegroundNotification,
            object: nil
        )
    }
    
    private func removeBackgroundObservers() {
        NotificationCenter.default.removeObserver(self, name: UIApplication.didEnterBackgroundNotification, object: nil)
        NotificationCenter.default.removeObserver(self, name: UIApplication.willEnterForegroundNotification, object: nil)
    }
    
    /// Gates the foreground relay reconnect on how long the app actually stayed
    /// in the background, fed the shared `MonotonicClock` (monotonic AND
    /// sleep-inclusive: it counts real time away — including device sleep —
    /// while staying immune to wall-clock steps from an NTP correction or manual
    /// change). Android has a paired `ForegroundReconnectPolicy` driving the
    /// same rule from its lifecycle callbacks, so both platforms heal on
    /// foreground automatically and identically. Main-thread confined: the
    /// lifecycle notifications that drive it are delivered on main.
    private let foregroundReconnectPolicy = ForegroundReconnectPolicy()

    /// Background assertion held while the deferred
    /// `wifiDirectStatusChanged(false)` hop completes. Main-confined: it is
    /// armed from the background notification and ended from either the
    /// expiration handler or a hop back to main, both of which run there — so
    /// "end exactly once" needs no synchronization.
    private var backgroundWifiTaskId: UIBackgroundTaskIdentifier = .invalid

    /// Main-only. Ends the assertion `id` unless a later background transition
    /// already superseded it. Leaving one unbalanced past its budget is itself
    /// a termination cause and ending the same identifier twice is a hard
    /// error, so ownership is checked rather than assumed: a completion whose
    /// assertion was superseded matches nothing and does nothing, which is
    /// what keeps a stale hop from ending the current transition's assertion.
    private func endBackgroundWifiTask(_ id: UIBackgroundTaskIdentifier) {
        guard id != .invalid, backgroundWifiTaskId == id else { return }
        UIApplication.shared.endBackgroundTask(id)
        backgroundWifiTaskId = .invalid
    }

    @objc private func applicationDidEnterBackground() {
        Self.testLastWifiStatusChangeForTesting = false
        foregroundReconnectPolicy.didEnterBackground(nowMs: MonotonicClock.nowMs())
        guard let proto = protocolInstance else { return }

        // Off-main deliberately. `wifiDirectStatusChanged` takes the global
        // protocol mutex on the Rust side — the same lock the process tick,
        // MLS work and group fan-out hold — so calling it synchronously here
        // parks the main thread for as long as that lock is contended, inside
        // the scene-update watchdog's ~10s wall-clock window. That is a
        // 0x8BADF00D kill, and it fires under exactly the load that makes the
        // lock slow. `processQueue` is serial, so this `false` and the `true`
        // from `applicationWillEnterForeground` cannot reorder.
        //
        // The background assertion keeps the hop as prompt as the synchronous
        // call it replaces: the SDK stays alive in the background on BLE, and
        // until this `false` lands DORS still scores a Wi-Fi Direct link iOS
        // has already torn down. `self` is captured strongly on purpose —
        // these closures are short-lived, they are not retained by `self`, and
        // a weak capture that lost its referent would leave the assertion
        // unbalanced.
        // Supersede an assertion still outstanding from an earlier transition;
        // its own completion will then match nothing and no-op.
        endBackgroundWifiTask(backgroundWifiTaskId)
        let armed = UIApplication.shared.beginBackgroundTask(
            withName: "OfflineProtocol.wifiDirectStatusChanged"
        ) {
            // Ending an assertion cancels its expiration handler, so this can
            // only fire while the one it belongs to is still the current one.
            self.endBackgroundWifiTask(self.backgroundWifiTaskId)
        }
        // Safe to assign after the call: the handler is delivered on main and
        // we have not yet returned to the runloop, so it cannot run first.
        backgroundWifiTaskId = armed
        processQueue.async {
            try? proto.wifiDirectStatusChanged(isConnected: false)
            DispatchQueue.main.async { self.endBackgroundWifiTask(armed) }
        }
    }

    @objc private func applicationWillEnterForeground() {
        Self.testLastWifiStatusChangeForTesting = true

        // Proactively heal a socket the OS likely killed during suspension.
        // `isInternetReady()` cannot tell a healthy socket from a zombie (both
        // report connected+authenticated), so gate on background duration
        // instead (see ForegroundReconnectPolicy): a stay long enough that the
        // socket is untrustworthy forces a reconnect. forceReconnect() no-ops
        // unless the transport is running/starting and resets the reconnect
        // backoff, so this is safe to call unconditionally within the gate.
        // Belt-and-braces with the in-transport write-stall watchdog, which
        // catches a zombie that survives (or first appears) without a
        // background→foreground edge. Apps no longer need to call
        // forceInternetReconnect() on foreground themselves.
        if foregroundReconnectPolicy.shouldReconnectOnForeground(nowMs: MonotonicClock.nowMs()) {
            internetManager?.forceReconnect()
        }

        guard let proto = protocolInstance else { return }
        // Same hop as the background side, for the same reason (the global
        // protocol mutex must not be taken on main during an OS-metered
        // transition) — and onto the same serial queue, which is what keeps
        // the false/true pair from reordering. No background assertion here:
        // the app is becoming active, not losing its runtime.
        processQueue.async {
            try? proto.wifiDirectStatusChanged(isConnected: true)
        }
    }

    /// Set by notification handlers for unit tests. Reset to nil before each test that uses it.
    static var testLastWifiStatusChangeForTesting: Bool?
    
    override class func requiresMainQueueSetup() -> Bool {
        return true
    }
    
    override func supportedEvents() -> [String]! {
        return [Events.onEvent, Events.onTelemetry]
    }

    override func startObserving() {
        hasListeners = true
    }

    override func stopObserving() {
        hasListeners = false
    }

    @objc override func addListener(_ eventName: String) {
        super.addListener(eventName)
    }

    @objc override func removeListeners(_ count: Double) {
        super.removeListeners(count)
    }
    
    // MARK: - Configuration Parsing
    
    private func parseConfig(_ configJson: String) throws -> (config: ProtocolConfig, raw: [String: Any]) {
        guard let jsonData = configJson.data(using: .utf8),
              let raw = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
            throw NSError(domain: "OfflineProtocol", code: -1, 
                         userInfo: [NSLocalizedDescriptionKey: "Invalid JSON"])
        }
        
        // Encryption section (four flags, compact-envelope kill switch,
        // pending queue): read by EncryptionConfigReader — Foundation-only so
        // the SwiftPM suite covers it; mirrors ProtocolConfigParser.kt on
        // Android, keep the read order and precedence in sync.
        let encryption = EncryptionConfigReader.read(raw)
        let overflowPolicy: OverflowPolicy
        switch encryption.overflowPolicyRaw.lowercased() {
        case "drop_newest":
            overflowPolicy = .dropNewest
        default:
            overflowPolicy = .dropOldest
        }
        // binaryWireEnabled (kill switch, default on): top level only — that
        // IS its home in both the JS config and the flat UniFFI dictionary.
        // camelCase or snake_case.
        let binaryWireEnabled = raw["binaryWireEnabled"] as? Bool
            ?? raw["binary_wire_enabled"] as? Bool ?? true

        // Nostr sealing (kill switch, default on). Nested home under
        // `transports.nostr.sealingEnabled` — where the rest of the Nostr
        // transport settings live — then the flat key, both cases. Same
        // nested-wins-over-flat rule as `encryption` and `group`; mirrors
        // ProtocolConfigParser.kt, keep in sync.
        let nostrRaw = (raw["transports"] as? [String: Any])?["nostr"] as? [String: Any]
        let nostrSealingEnabled = nostrRaw?["sealingEnabled"] as? Bool
            ?? nostrRaw?["sealing_enabled"] as? Bool
            ?? raw["nostrSealingEnabled"] as? Bool
            ?? raw["nostr_sealing_enabled"] as? Bool
            ?? true

        // Nostr cold contact (key-package publication + peer resolution, kill
        // switch, default on). Same nested-then-flat shape as sealing above.
        let nostrColdContactEnabled = nostrRaw?["coldContactEnabled"] as? Bool
            ?? nostrRaw?["cold_contact_enabled"] as? Bool
            ?? raw["nostrColdContactEnabled"] as? Bool
            ?? raw["nostr_cold_contact_enabled"] as? Bool
            ?? true

        // Group section (nested home under `group`, then top level, both
        // cases — same shape rules as `encryption`; mirrors
        // ProtocolConfigParser.kt, keep in sync). These were UniFFI-only
        // until the broadcast default flipped on: with no JS-reachable
        // opt-out, an RN app could not force per-member fan-out.
        let groupRaw = raw["group"] as? [String: Any]
        let maxGroupMembers = groupRaw?["maxGroupMembers"] as? Int
            ?? groupRaw?["max_group_members"] as? Int
            ?? raw["maxGroupMembers"] as? Int
            ?? raw["max_group_members"] as? Int
            ?? 256
        let groupRelayEnabled = groupRaw?["relayEnabled"] as? Bool
            ?? groupRaw?["relay_enabled"] as? Bool
            ?? raw["groupRelayEnabled"] as? Bool
            ?? raw["group_relay_enabled"] as? Bool
            ?? true
        let groupRelayBroadcastEnabled = groupRaw?["relayBroadcastEnabled"] as? Bool
            ?? groupRaw?["relay_broadcast_enabled"] as? Bool
            ?? raw["groupRelayBroadcastEnabled"] as? Bool
            ?? raw["group_relay_broadcast_enabled"] as? Bool
            ?? true
        // Default false — see GroupConfig::enforce_admin_commits. Enabling it
        // makes this device refuse membership commits it cannot authorize,
        // which forks it from every member that accepted them.
        let groupEnforceAdminCommits = groupRaw?["enforceAdminCommits"] as? Bool
            ?? groupRaw?["enforce_admin_commits"] as? Bool
            ?? raw["groupEnforceAdminCommits"] as? Bool
            ?? raw["group_enforce_admin_commits"] as? Bool
            ?? false

        let config = ProtocolConfig(
            appId: raw["appId"] as? String ?? raw["app_id"] as? String ?? "",
            userId: raw["userId"] as? String ?? raw["user_id"] as? String ?? "",
            bleEnabled: raw["bleEnabled"] as? Bool ?? raw["ble_enabled"] as? Bool ?? true,
            wifiDirectEnabled: raw["wifiDirectEnabled"] as? Bool ?? raw["wifi_direct_enabled"] as? Bool ?? true,
            internetEnabled: raw["internetEnabled"] as? Bool ?? raw["internet_enabled"] as? Bool ?? true,
            reticulumEnabled: raw["reticulumEnabled"] as? Bool ?? raw["reticulum_enabled"] as? Bool ?? false,
            nostrEnabled: raw["nostrEnabled"] as? Bool ?? raw["nostr_enabled"] as? Bool ?? false,
            preferOnline: raw["preferOnline"] as? Bool ?? raw["prefer_online"] as? Bool ?? false,
            initialTtl: UInt8(raw["initialTtl"] as? Int ?? raw["initial_ttl"] as? Int ?? 8),
            encryptionEnabled: encryption.enabled,
            autoKeyExchange: encryption.autoKeyExchange,
            storePending: encryption.storePending,
            requireEncryption: encryption.requireEncryption,
            maxPendingPerPeer: encryption.maxPendingPerPeer,
            maxPendingGlobal: encryption.maxPendingGlobal,
            pendingTtlMs: encryption.pendingTtlMs,
            overflowPolicy: overflowPolicy,
            // `clamping:` and not `UInt32(_:)` — the value is app-supplied JS,
            // and a negative or oversized number traps the initializer.
            maxGroupMembers: UInt32(clamping: maxGroupMembers),
            groupRelayEnabled: groupRelayEnabled,
            groupRelayBroadcastEnabled: groupRelayBroadcastEnabled,
            groupEnforceAdminCommits: groupEnforceAdminCommits,
            binaryWireEnabled: binaryWireEnabled,
            nostrSealingEnabled: nostrSealingEnabled,
            nostrColdContactEnabled: nostrColdContactEnabled,
            compactEnvelopeEnabled: encryption.compactEnvelopeEnabled,
            richPayloadEnabled: encryption.richPayloadEnabled,
            cryptoRecoveryEnabled: encryption.cryptoRecoveryEnabled
        )

        return (config, raw)
    }

    private func normalizeRelayPriority(_ priority: String?) -> RelayPriority? {
        guard let value = priority?.lowercased(), !value.isEmpty else {
            return nil
        }
        switch value {
        case "low":
            return .low
        case "medium":
            return .medium
        case "high":
            return .high
        case "never":
            return .low
        case "always":
            return .high
        case "auto":
            return .medium
        default:
            return nil
        }
    }

    /// Rejects with the stable typed code when the error maps (see
    /// ProtocolErrorBridge.swift), otherwise with the method's legacy
    /// fallback code and "<fallbackMessage>: <cause>".
    private func rejectWithProtocolError(
        _ error: Error,
        _ rejecter: RCTPromiseRejectBlock,
        fallbackCode: String,
        fallbackMessage: String
    ) {
        if let mapped = mapProtocolBridgeError(error) {
            rejecter(mapped.code, mapped.message, error)
        } else {
            rejecter(fallbackCode, "\(fallbackMessage): \(error.localizedDescription)", error)
        }
    }

    private func applyInitialRuntimeConfig(_ proto: OfflineProtocol, rawConfig: [String: Any]) {
        if let dorsDict = rawConfig["dors"] as? [String: Any] {
            let preferOnline = dorsDict["preferOnline"] as? Bool ?? dorsDict["prefer_online"] as? Bool ?? false
            let switchHysteresis = Float((dorsDict["switchHysteresis"] as? NSNumber)?.doubleValue
                                         ?? (dorsDict["switch_hysteresis"] as? NSNumber)?.doubleValue
                                         ?? 15.0)
            let switchCooldown = UInt64((dorsDict["switchCooldownSecs"] as? NSNumber)?.uint64Value
                                        ?? (dorsDict["switch_cooldown_secs"] as? NSNumber)?.uint64Value
                                        ?? 20)
            let bleRetry = UInt32((dorsDict["bleToWifiRetryThreshold"] as? NSNumber)?.uint32Value
                                  ?? (dorsDict["ble_to_wifi_retry_threshold"] as? NSNumber)?.uint32Value
                                  ?? 2)
            let minSuccessRate = Float((dorsDict["minSuccessRateBeforeEscalation"] as? NSNumber)?.floatValue
                                   ?? (dorsDict["min_success_rate_before_escalation"] as? NSNumber)?.floatValue
                                   ?? 0.3)
            let minSuccessRateClamped = min(max(minSuccessRate, 0.0), 1.0)
            let minBleSamples = UInt64((dorsDict["minBleSamplesBeforeSuccessRateEscalation"] as? NSNumber)?.uint64Value
                                      ?? (dorsDict["min_ble_samples_before_success_rate_escalation"] as? NSNumber)?.uint64Value
                                      ?? 5)
            let rssiThreshold = Int16((dorsDict["rssiSwitchThreshold"] as? NSNumber)?.int16Value
                                      ?? (dorsDict["rssi_switch_threshold"] as? NSNumber)?.int16Value
                                      ?? -85)
            let congestionThreshold = UInt64((dorsDict["congestionQueueThreshold"] as? NSNumber)?.uint64Value
                                             ?? (dorsDict["congestion_queue_threshold"] as? NSNumber)?.uint64Value
                                             ?? 50)
            let stabilityWindow = UInt64((dorsDict["stabilityWindowSecs"] as? NSNumber)?.uint64Value
                                         ?? (dorsDict["stability_window_secs"] as? NSNumber)?.uint64Value
                                         ?? 8)
            let poorSignalDuration = UInt64((dorsDict["poorSignalDurationSecs"] as? NSNumber)?.uint64Value
                                            ?? (dorsDict["poor_signal_duration_secs"] as? NSNumber)?.uint64Value
                                            ?? 10)
            let ttlThreshold = UInt8((dorsDict["ttlEscalationThreshold"] as? NSNumber)?.uint8Value
                                     ?? (dorsDict["ttl_escalation_threshold"] as? NSNumber)?.uint8Value
                                     ?? 2)
        let congestionDuration = UInt64((dorsDict["congestionDurationSecs"] as? NSNumber)?.uint64Value
                                        ?? (dorsDict["congestion_duration_secs"] as? NSNumber)?.uint64Value
                                        ?? 10)
        let ttlHold = UInt64((dorsDict["ttlEscalationHoldSecs"] as? NSNumber)?.uint64Value
                             ?? (dorsDict["ttl_escalation_hold_secs"] as? NSNumber)?.uint64Value
                             ?? 20)
        let historyWindowRaw = UInt64((dorsDict["historyWindowSize"] as? NSNumber)?.uint64Value
                                      ?? (dorsDict["history_window_size"] as? NSNumber)?.uint64Value
                                      ?? 10)
        let historyWindow = max(1, min(100, Int(historyWindowRaw)))
        let rawQueueRecovery = Float((dorsDict["queueRecoveryRatio"] as? NSNumber)?.floatValue
                                     ?? (dorsDict["queue_recovery_ratio"] as? NSNumber)?.floatValue
                                     ?? 0.5)
        let queueRecovery = min(max(rawQueueRecovery, 0.0), 1.0)

            let dorsConfig = DorsConfig(
                preferOnline: preferOnline,
                switchHysteresis: switchHysteresis,
                switchCooldownSecs: switchCooldown,
                bleToWifiRetryThreshold: bleRetry,
                minSuccessRateBeforeEscalation: minSuccessRateClamped,
                minBleSamplesBeforeSuccessRateEscalation: minBleSamples,
                rssiSwitchThreshold: rssiThreshold,
                congestionQueueThreshold: congestionThreshold,
                stabilityWindowSecs: stabilityWindow,
                poorSignalDurationSecs: poorSignalDuration,
            ttlEscalationThreshold: ttlThreshold,
            congestionDurationSecs: congestionDuration,
            ttlEscalationHoldSecs: ttlHold,
            historyWindowSize: UInt64(historyWindow),
            queueRecoveryRatio: queueRecovery
            )

            do {
                try proto.updateDorsConfig(config: dorsConfig)
                emitDiagnostic(level: "info", message: "Applied initial DORS config")
            } catch {
                emitDiagnostic(level: "warning", message: "Failed to apply initial DORS config", context: [
                    "error": error.localizedDescription
                ])
            }
        }

        if let relayDict = rawConfig["relay"] as? [String: Any] {
            let priorityRaw = (relayDict["relayPriority"] as? String) ?? (relayDict["relay_priority"] as? String)
            if let priority = normalizeRelayPriority(priorityRaw) {
                do {
                    try proto.setRelayPriority(priority: priority)
                    let priorityLabel: String
                    switch priority {
                    case .low: priorityLabel = "low"
                    case .medium: priorityLabel = "medium"
                    case .high: priorityLabel = "high"
                    @unknown default: priorityLabel = "medium"
                    }
                    emitDiagnostic(level: "info", message: "Applied initial relay priority", context: [
                        "priority": priorityRaw ?? priorityLabel
                    ])
                } catch {
                    emitDiagnostic(level: "warning", message: "Failed to apply initial relay priority", context: [
                        "error": error.localizedDescription
                    ])
                }
            }
        }
    }
    
    // MARK: - Exported Methods
    
    @objc func create(_ configJson: String,
                     resolver: @escaping RCTPromiseResolveBlock,
                     rejecter: @escaping RCTPromiseRejectBlock) {
        print("[OfflineProtocolModule] create() called with config: \(configJson)")
        do {
            print("[OfflineProtocolModule] Parsing config...")
            let parsed = try parseConfig(configJson)
            let config = parsed.config
            print("[OfflineProtocolModule] Config parsed successfully: \(config)")
            print("[OfflineProtocolModule] Creating OfflineProtocol instance...")
            let proto = try OfflineProtocol(config: config)
            print("[OfflineProtocolModule] OfflineProtocol instance created successfully")
            currentConfig = config
            emitDiagnostic(level: "info", message: "Protocol core created", context: [
                "appId": config.appId,
                "userId": config.userId,
                "bleEnabled": config.bleEnabled,
                "wifiDirectEnabled": config.wifiDirectEnabled,
                "internetEnabled": config.internetEnabled,
                "reticulumEnabled": config.reticulumEnabled
            ])
            
            // Set up event callback
            proto.setEventCallback(callback: EventCallbackImpl(emitter: self))

            applyInitialRuntimeConfig(proto, rawConfig: parsed.raw)

            protocolInstance = proto
            meshServicesInstance = try MeshServices(protocol: proto)

            // Initialize BLE manager if BLE is enabled
            if config.bleEnabled {
                let manager = BleManager(protocol: proto, deviceId: config.userId)
                manager.delegate = self
                bleManager = manager
                
                // Register event-driven transport callback — replaces timer-based polling.
                // When Rust enqueues a fragment, this callback fires and Swift sends immediately.
                proto.setBleTransportCallback(callback: BleTransportCallbackImpl(bleManager: manager))
                
                print("[OfflineProtocolModule] BLE Manager initialized for user: \(config.userId)")
                emitDiagnostic(level: "info", message: "BLE manager initialized", context: [
                    "userId": config.userId
                ])
            } else {
                emitDiagnostic(level: "warning", message: "BLE disabled in configuration", context: [
                    "userId": config.userId
                ])
            }
            
            // Initialize Internet manager if internet is enabled
            if config.internetEnabled {
                internetManager = InternetManager(protocol: proto, deviceId: config.userId)
                internetManager?.delegate = self
                internetManager?.serverMessageEmitter = { [weak self] rawJson in
                    self?.emitServerMessageEvent(rawJson)
                }
                internetManager?.connectionStatusEmitter = { [weak self] connected, authenticated in
                    self?.emitInternetStatusEvent(connected: connected, authenticated: authenticated)
                }
                internetManager?.supersededEmitter = { [weak self] reason in
                    self?.emitInternetSupersededEvent(reason: reason)
                }
                print("[OfflineProtocolModule] Internet Manager initialized for user: \(config.userId)")
                
                // Extract and store internet config for use during start()
                if let transportsDict = parsed.raw["transports"] as? [String: Any],
                   let internetDict = transportsDict["internet"] as? [String: Any] {
                    internetServerUrl = (internetDict["serverAddress"] as? String) ?? (internetDict["server_address"] as? String)
                    internetAutoReconnect = (internetDict["autoReconnect"] as? Bool) ?? (internetDict["auto_reconnect"] as? Bool) ?? true
                    print("[OfflineProtocolModule] Internet server URL from config: \(internetServerUrl ?? "nil")")
                }
                
                emitDiagnostic(level: "info", message: "Internet manager initialized", context: [
                    "userId": config.userId,
                    "serverUrl": internetServerUrl ?? "not configured"
                ])
            } else {
                emitDiagnostic(level: "info", message: "Internet disabled in configuration", context: [
                    "userId": config.userId
                ])
            }

            // Initialize Reticulum manager if reticulum is enabled
            if config.reticulumEnabled {
                let retManager = ReticulumManager(protocol: proto, deviceId: config.userId)
                retManager.delegate = self
                reticulumManager = retManager

                // Register event-driven transport callback — replaces timer-based polling.
                proto.setReticulumTransportCallback(callback: ReticulumTransportCallbackImpl(reticulumManager: retManager))

                print("[OfflineProtocolModule] Reticulum Manager initialized for user: \(config.userId)")

                // Extract and store reticulum config for use during start()
                if let transportsDict = parsed.raw["transports"] as? [String: Any],
                   let reticulumDict = transportsDict["reticulum"] as? [String: Any] {
                    let daemonAddress = (reticulumDict["daemonAddress"] as? String) ?? (reticulumDict["daemon_address"] as? String) ?? "localhost:4242"
                    let autoReconnect = (reticulumDict["autoReconnect"] as? Bool) ?? (reticulumDict["auto_reconnect"] as? Bool) ?? true
                    let maxReconnectAttempts = (reticulumDict["maxReconnectAttempts"] as? Int) ?? 0
                    reticulumManager?.configure(daemonAddress: daemonAddress, autoReconnect: autoReconnect, maxReconnectAttempts: maxReconnectAttempts)
                }

                emitDiagnostic(level: "info", message: "Reticulum manager initialized", context: [
                    "userId": config.userId
                ])
            } else {
                emitDiagnostic(level: "info", message: "Reticulum disabled in configuration", context: [
                    "userId": config.userId
                ])
            }

            // Initialize Nostr manager if nostr is enabled
            if config.nostrEnabled {
                let nostrMgr = NostrManager(protocol: proto, deviceId: config.userId)
                nostrMgr.delegate = self
                nostrManager = nostrMgr

                // Register event-driven transport callback
                proto.setNostrTransportCallback(callback: NostrTransportCallbackImpl(nostrManager: nostrMgr))

                print("[OfflineProtocolModule] Nostr Manager initialized for user: \(config.userId)")

                // Extract and store nostr config for use during start()
                if let transportsDict = parsed.raw["transports"] as? [String: Any],
                   let nostrDict = transportsDict["nostr"] as? [String: Any] {
                    let relayUrls = (nostrDict["relayUrls"] as? [String]) ?? (nostrDict["relay_urls"] as? [String]) ?? []
                    let autoReconnect = (nostrDict["autoReconnect"] as? Bool) ?? (nostrDict["auto_reconnect"] as? Bool) ?? true
                    let maxReconnectAttempts = (nostrDict["maxReconnectAttempts"] as? Int) ?? (nostrDict["max_reconnect_attempts"] as? Int) ?? 0
                    let connectionTimeout = (nostrDict["connectionTimeout"] as? Double) ?? (nostrDict["connection_timeout"] as? Double) ?? 30.0
                    nostrManager?.configure(relayUrls: relayUrls, autoReconnect: autoReconnect, maxReconnectAttempts: maxReconnectAttempts, connectionTimeout: connectionTimeout)
                }

                emitDiagnostic(level: "info", message: "Nostr manager initialized", context: [
                    "userId": config.userId
                ])
            } else {
                emitDiagnostic(level: "info", message: "Nostr disabled in configuration", context: [
                    "userId": config.userId
                ])
            }

            // Start process timer
            startProcessTimer()
            emitDiagnostic(level: "info", message: "Protocol process timer started")
            
            resolver(nil)
        } catch {
            emitDiagnostic(level: "error", message: "Failed to create protocol", context: [
                "error": error.localizedDescription
            ])
            rejecter("ERROR_CREATE", "Failed to create protocol: \(error.localizedDescription)", error)
        }
    }
    
    // MARK: - Event Handling
    
    fileprivate func sendEventToJS(_ eventName: String, body: Any?) {
        if hasListeners {
            sendEvent(withName: eventName, body: body)
        }
    }
    
    fileprivate func emitDiagnostic(level: String, message: String, context: [String: Any]? = nil) {
        guard hasListeners else { return }

        var payload: [String: Any] = [
            "type": "diagnostic",
            "level": level,
            "message": message
        ]

        if let context = context {
            payload["context"] = sanitizeJSONObject(context)
        }

        let sanitizedPayload = sanitizeJSONObject(payload)

        if let payloadDict = sanitizedPayload as? [String: Any],
           JSONSerialization.isValidJSONObject(payloadDict),
           let data = try? JSONSerialization.data(withJSONObject: payloadDict, options: []),
           let jsonString = String(data: data, encoding: .utf8) {
            sendEventToJS(Events.onEvent, body: ["eventJson": jsonString])
        } else {
            let fallback: [String: Any] = [
                "type": "diagnostic",
                "level": level,
                "message": message,
                "context": String(describing: context ?? [:])
            ]
            if let data = try? JSONSerialization.data(withJSONObject: fallback, options: []),
               let jsonString = String(data: data, encoding: .utf8) {
                sendEventToJS(Events.onEvent, body: ["eventJson": jsonString])
            }
        }
    }

    /// Forwards an internet-transport readiness transition as the
    /// `internet_status_changed` event. `authenticated: true` is the
    /// positive gate for raw server commands (replaces app-side
    /// `relayStatus === 'authenticated'` tracking).
    fileprivate func emitInternetStatusEvent(connected: Bool, authenticated: Bool) {
        guard hasListeners else { return }
        let payload: [String: Any] = [
            "type": "internet_status_changed",
            "connected": connected,
            "authenticated": authenticated
        ]
        if let data = try? JSONSerialization.data(withJSONObject: payload, options: []),
           let jsonString = String(data: data, encoding: .utf8) {
            sendEventToJS(Events.onEvent, body: ["eventJson": jsonString])
        }
    }

    /// Forwards a relay session displacement ("superseded") as the
    /// `internet_session_superseded` event. The relay closed this socket with
    /// code 4000 because a newer registration for the same identity took over;
    /// the SDK will NOT auto-reconnect. The app surfaces "connected elsewhere"
    /// and reconnects only on explicit user action (re-enabling the transport).
    fileprivate func emitInternetSupersededEvent(reason: String?) {
        guard hasListeners else { return }
        var payload: [String: Any] = [
            "type": "internet_session_superseded"
        ]
        if let reason = reason { payload["reason"] = reason }
        if let data = try? JSONSerialization.data(withJSONObject: payload, options: []),
           let jsonString = String(data: data, encoding: .utf8) {
            sendEventToJS(Events.onEvent, body: ["eventJson": jsonString])
        }
    }

    /// Forwards a raw relay frame apps need outside or in addition to SDK-owned
    /// processing (group snapshot extensions, invite links, role changes,
    /// rate limiting, unknown types) as the `internet_server_message` event.
    fileprivate func emitServerMessageEvent(_ rawJson: String) {
        guard hasListeners else { return }
        let payload: [String: Any] = [
            "type": "internet_server_message",
            "json": rawJson
        ]
        if let data = try? JSONSerialization.data(withJSONObject: payload, options: []),
           let jsonString = String(data: data, encoding: .utf8) {
            sendEventToJS(Events.onEvent, body: ["eventJson": jsonString])
        }
    }

    private static let iso8601Formatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    private func sanitizeJSONObject(_ value: Any) -> Any {
        switch value {
        case let dict as [String: Any]:
            var sanitized: [String: Any] = [:]
            for (key, nestedValue) in dict {
                sanitized[key] = sanitizeJSONObject(nestedValue)
            }
            return sanitized
        case let dict as [AnyHashable: Any]:
            var sanitized: [String: Any] = [:]
            for (key, nestedValue) in dict {
                sanitized[String(describing: key)] = sanitizeJSONObject(nestedValue)
            }
            return sanitized
        case let dict as NSDictionary:
            var sanitized: [String: Any] = [:]
            dict.forEach { key, value in
                sanitized[String(describing: key)] = sanitizeJSONObject(value)
            }
            return sanitized
        case let array as [Any]:
            return array.map { sanitizeJSONObject($0) }
        case let array as NSArray:
            return array.map { sanitizeJSONObject($0) }
        case let string as String:
            return string
        case let bool as Bool:
            return bool
        case let number as NSNumber:
            if CFNumberIsFloatType(number) {
                let doubleValue = number.doubleValue
                if !doubleValue.isFinite {
                    return String(describing: doubleValue)
                }
            }
            return number
        case let double as Double:
            return double.isFinite ? double : String(describing: double)
        case let float as Float:
            return float.isFinite ? float : String(describing: float)
        case let int as Int:
            return int
        case let int32 as Int32:
            return int32
        case let int64 as Int64:
            return int64
        case let uint as UInt:
            return uint
        case let uuid as UUID:
            return uuid.uuidString
        case let cbUuid as CBUUID:
            return cbUuid.uuidString
        case let date as Date:
            return OfflineProtocolModule.iso8601Formatter.string(from: date)
        case let data as Data:
            return data.base64EncodedString()
        case let dateComponents as DateComponents:
            return String(describing: dateComponents)
        case let url as URL:
            return url.absoluteString
        case let error as NSError:
            return [
                "domain": error.domain,
                "code": error.code,
                "userInfo": sanitizeJSONObject(error.userInfo)
            ]
        case is NSNull:
            return NSNull()
        default:
            return String(describing: value)
        }
    }
    
    @objc func start(_ resolver: @escaping RCTPromiseResolveBlock,
                   rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            emitDiagnostic(level: "info", message: "Starting protocol")
            try protocolInstance?.start()
            emitDiagnostic(level: "info", message: "Protocol core started")
            
            //  Start BLE manager if available - BLE should work independently
            // BLE peer discovery and messaging must work even when Internet/WiFi are disabled
            if let manager = bleManager {
                do {
                    print("[OfflineProtocolModule] Starting BLE manager (BLE should work independently of other transports)...")
                    emitDiagnostic(level: "info", message: "Starting BLE manager", context: [
                        "internetEnabled": currentConfig?.internetEnabled ?? false,
                        "wifiDirectEnabled": currentConfig?.wifiDirectEnabled ?? false
                    ])
                    try manager.start()
                    print("[OfflineProtocolModule] ✅ BLE Manager started successfully - scanning and advertising should be active")
                    emitDiagnostic(level: "info", message: "BLE manager started - peer discovery active", context: [
                        "scanning": true,
                        "advertising": true
                    ])
                    
                    //  Ensure bleStatusChanged(true) is called immediately and as backup
                    // This ensures BLE transport is marked as Available for message sending
                    do {
                        try protocolInstance?.bleStatusChanged(isAvailable: true)
                        print("[OfflineProtocolModule] ✅ Called protocol.bleStatusChanged(true) immediately")
                        emitDiagnostic(level: "info", message: "BLE status set to available")
                    } catch {
                        print("[OfflineProtocolModule] Immediate bleStatusChanged failed: \(error.localizedDescription)")
                    }
                    
                    // Backup call in case timing is off. Held in a work item so
                    // `destroy` can cancel it: uncancelled, it re-enters the
                    // protocol up to a second after teardown, which a wipe
                    // following `destroy` would race.
                    //
                    // Scheduled on processQueue, not main: bleStatusChanged
                    // takes the global protocol mutex, and this fires one
                    // second after start() — squarely inside the launch
                    // window where restore and the first flush make that
                    // lock slow (the 0x8BADF00D class the lifecycle hops
                    // fixed). processQueue also closes the cancel race for
                    // good: destroy's drainProcessQueue() barrier waits out
                    // a backup that already started running, which a
                    // main-queue backup could dodge.
                    bleStatusBackupWorkItem?.cancel()
                    let backup = DispatchWorkItem { [weak self] in
                        print("[OfflineProtocolModule] Backup bleStatusChanged(true) call")
                        self?.emitDiagnostic(level: "info", message: "Backup call to protocol.bleStatusChanged(true)")
                        try? self?.protocolInstance?.bleStatusChanged(isAvailable: true)
                        self?.emitDiagnostic(level: "info", message: "Backup bleStatusChanged(true) completed")
                    }
                    bleStatusBackupWorkItem = backup
                    processQueue.asyncAfter(deadline: .now() + 1.0, execute: backup)
                } catch {
                    print("[OfflineProtocolModule] ❌ FAILED to start BLE Manager: \(error.localizedDescription)")
                    emitDiagnostic(level: "error", message: "Failed to start BLE manager", context: [
                        "error": error.localizedDescription
                    ])
                    // Don't fail the entire start if BLE fails, but log the error clearly
                    print("[OfflineProtocolModule] ⚠️ Protocol will continue without BLE, but peer discovery and BLE messaging will not work")
                }
            } else {
                print("[OfflineProtocolModule] ⚠️ BLE manager is null - BLE was not initialized. Check if bleEnabled=true in config.")
                emitDiagnostic(level: "warning", message: "BLE manager is null - BLE not initialized", context: [
                    "bleEnabled": currentConfig?.bleEnabled ?? false
                ])
            }
            
            // Start Internet manager if configured with a server URL
            if let manager = internetManager, let serverUrl = internetServerUrl, !serverUrl.isEmpty {
                do {
                    try manager.configure(serverUrl: serverUrl, autoReconnect: internetAutoReconnect, maxReconnectAttempts: 0)
                    try manager.start()
                    print("[OfflineProtocolModule] Internet Manager started with URL: \(serverUrl)")
                    emitDiagnostic(level: "info", message: "Internet manager started", context: [
                        "serverUrl": serverUrl,
                        "autoReconnect": internetAutoReconnect
                    ])
                } catch {
                    print("[OfflineProtocolModule] Warning: Failed to start Internet Manager: \(error.localizedDescription)")
                    emitDiagnostic(level: "error", message: "Failed to start Internet manager", context: [
                        "error": error.localizedDescription,
                        "serverUrl": serverUrl
                    ])
                    // Don't fail the entire start if Internet fails
                }
            } else if internetManager != nil {
                emitDiagnostic(level: "warning", message: "Internet manager exists but no server URL configured")
            }

            // Reticulum manager is started on-demand via enableTransport("reticulum")
            // from the JS layer, not here — avoids double-start conflicts.

            // Nostr manager is started on-demand via enableTransport("nostr")
            // from the JS layer, not here — avoids double-start conflicts.

            resolver(nil)
        } catch {
            emitDiagnostic(level: "error", message: "Failed to start protocol", context: [
                "error": error.localizedDescription
            ])
            rejecter("ERROR_START", "Failed to start protocol: \(error.localizedDescription)", error)
        }
    }

    @objc func emitTestEvent(_ resolver: @escaping RCTPromiseResolveBlock,
                             rejecter: @escaping RCTPromiseRejectBlock) {
        protocolInstance?.emitTestEvent()
        resolver(nil)
    }

    @objc func installTelemetrySink(_ configDict: NSDictionary?,
                                    resolver: @escaping RCTPromiseResolveBlock,
                                    rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("NOT_STARTED", "Protocol not created", nil)
            return
        }
        do {
            let config = parseTelemetryConfig(configDict as? [String: Any])
            try proto.installTelemetrySink(sink: TelemetrySinkImpl(emitter: self), config: config)
            resolver(nil)
        } catch {
            rejecter("TELEMETRY_INSTALL", "Failed to install telemetry sink: \(error.localizedDescription)", error)
        }
    }

    @objc func pollTelemetryFrame(_ resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("NOT_STARTED", "Protocol not created", nil)
            return
        }
        resolver(proto.pollTelemetryFrame())
    }

    @objc func uninstallTelemetrySink(_ resolver: @escaping RCTPromiseResolveBlock,
                                      rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("NOT_STARTED", "Protocol not created", nil)
            return
        }
        do {
            try proto.uninstallTelemetrySink()
            resolver(nil)
        } catch {
            rejecter("TELEMETRY_UNINSTALL", "Failed to uninstall telemetry sink: \(error.localizedDescription)", error)
        }
    }

    @objc func telemetryInstallId(_ resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("NOT_STARTED", "Protocol not created", nil)
            return
        }
        resolver(proto.telemetryInstallId())
    }
    
    @objc func stop(_ resolver: @escaping RCTPromiseResolveBlock,
                   rejecter: @escaping RCTPromiseRejectBlock) {
        stopProcessTimer()
        
        // Stop BLE manager first
        bleManager?.stop()
        print("[OfflineProtocolModule] BLE Manager stopped")
        emitDiagnostic(level: "info", message: "BLE manager stopped")
        
        // Stop Internet manager
        internetManager?.stop()
        print("[OfflineProtocolModule] Internet Manager stopped")
        emitDiagnostic(level: "info", message: "Internet manager stopped")

        // Stop Reticulum manager
        reticulumManager?.stop()
        print("[OfflineProtocolModule] Reticulum Manager stopped")
        emitDiagnostic(level: "info", message: "Reticulum manager stopped")

        // Stop Nostr manager
        nostrManager?.stop()
        print("[OfflineProtocolModule] Nostr Manager stopped")
        emitDiagnostic(level: "info", message: "Nostr manager stopped")

        do {
            try protocolInstance?.stop()
            emitDiagnostic(level: "info", message: "Protocol stopped")
            resolver(nil)
        } catch {
            emitDiagnostic(level: "error", message: "Failed to stop protocol", context: [
                "error": error.localizedDescription
            ])
            rejecter("ERROR_STOP", "Failed to stop protocol: \(error.localizedDescription)", error)
        }
    }
    
    @objc func pause(_ resolver: @escaping RCTPromiseResolveBlock,
                    rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            // Pause transports for background mode
            bleManager?.pause()
            reticulumManager?.pause()
            nostrManager?.pause()
            internetManager?.pause()

            try protocolInstance?.pause()
            resolver(nil)
        } catch {
            rejecter("ERROR_PAUSE", "Failed to pause protocol: \(error.localizedDescription)", error)
        }
    }
    
    @objc func resume(_ resolver: @escaping RCTPromiseResolveBlock,
                     rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            try protocolInstance?.resume()

            // Resume transports
            bleManager?.resume()
            reticulumManager?.resume()
            nostrManager?.resume()
            internetManager?.resume()

            resolver(nil)
        } catch {
            rejecter("ERROR_RESUME", "Failed to resume protocol: \(error.localizedDescription)", error)
        }
    }
    
    @objc func sendMessage(_ recipient: String,
                          content: String,
                          priority: Int,
                          replyToMsg: String?,
                          resolver: @escaping RCTPromiseResolveBlock,
                          rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            let msgPriority: MessagePriority = {
                switch priority {
                case 0: return .low
                case 1: return .medium
                case 2: return .high
                case 3: return .critical
                default: return .medium
                }
            }()
            
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }
            
            let messageId = try proto.sendMessage(recipient: recipient, content: content, priority: msgPriority, replyToMsg: replyToMsg)
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_SEND", fallbackMessage: "Failed to send message")
        }
    }

    /// Rich send surface: reply context, rich media metadata, and forward
    /// attribution — sealed inside the MLS ciphertext for capable
    /// recipients, silently dropped for everyone else (never cleartext).
    @objc func sendMessageRich(_ recipient: String,
                               content: String,
                               priority: Int,
                               replyToMsg: String?,
                               options: NSDictionary?,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            let msgPriority: MessagePriority = {
                switch priority {
                case 0: return .low
                case 1: return .medium
                case 2: return .high
                case 3: return .critical
                default: return .medium
                }
            }()

            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }

            let dict = options as? [String: Any] ?? [:]
            let sendOptions = SendMessageOptions(
                priority: msgPriority,
                replyToMsg: replyToMsg,
                contentType: (dict["content_type"] as? String).map(parseContentType),
                replyContext: parseReplyContext(dict["reply_context"] as? [String: Any]),
                mediaMetadata: parseRichMediaMetadata(dict["media_metadata"] as? [String: Any]),
                forwardInfo: parseForwardInfo(dict["forward_info"] as? [String: Any])
            )

            let messageId = try proto.sendMessageRich(recipient: recipient, content: content, options: sendOptions)
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_SEND", fallbackMessage: "Failed to send rich message")
        }
    }

    private func parseReplyContext(_ dict: [String: Any]?) -> ReplyContext? {
        guard let dict = dict else { return nil }
        return ReplyContext(
            sender: dict["sender"] as? String ?? "",
            text: dict["text"] as? String ?? "",
            timestamp: (dict["timestamp"] as? NSNumber)?.int64Value,
            replyMediaLabel: dict["reply_media_label"] as? String,
            replyContentType: dict["reply_content_type"] as? String
        )
    }

    private func parseForwardInfo(_ dict: [String: Any]?) -> ForwardInfo? {
        guard let dict = dict else { return nil }
        return ForwardInfo(
            originalSender: dict["original_sender"] as? String ?? "",
            originalMessageId: dict["original_message_id"] as? String ?? "",
            originalTimestamp: (dict["original_timestamp"] as? NSNumber)?.int64Value ?? 0,
            forwardCount: (dict["forward_count"] as? NSNumber)?.uint32Value ?? 1
        )
    }

    /// Full MediaMetadata parser for the rich send surface — unlike the
    /// legacy sendMedia mapping, this includes the cloud/sticker fields
    /// (they only ever travel MLS-sealed on this path).
    private func parseRichMediaMetadata(_ dict: [String: Any]?) -> MediaMetadata? {
        guard let dict = dict else { return nil }
        return MediaMetadata(
            mimeType: dict["mime_type"] as? String ?? "",
            fileName: dict["file_name"] as? String ?? "",
            fileSize: (dict["file_size"] as? NSNumber)?.uint64Value ?? 0,
            durationMs: (dict["duration_ms"] as? NSNumber)?.uint64Value,
            width: (dict["width"] as? NSNumber)?.uint32Value,
            height: (dict["height"] as? NSNumber)?.uint32Value,
            thumbnailBase64: dict["thumbnail_base64"] as? String,
            mediaId: dict["media_id"] as? String,
            downloadUrl: dict["download_url"] as? String,
            thumbnailUrl: dict["thumbnail_url"] as? String,
            encryptionKey: dict["encryption_key"] as? String,
            iv: dict["iv"] as? String,
            ciphertextHash: dict["ciphertext_hash"] as? String,
            stickerProvider: dict["sticker_provider"] as? String,
            stickerRemoteId: dict["sticker_remote_id"] as? String,
            stickerKind: dict["sticker_kind"] as? String
        )
    }

    /// Forwards a message to a new recipient with original sender attribution.
    @objc func forwardMessage(_ originalMessageJson: String,
                              newRecipient: String,
                              priority: NSNumber?,
                              resolver: @escaping RCTPromiseResolveBlock,
                              rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }

            var msgPriority: MessagePriority? = nil
            if let p = priority {
                switch p.intValue {
                case 0: msgPriority = .low
                case 1: msgPriority = .medium
                case 2: msgPriority = .high
                case 3: msgPriority = .critical
                default: break
                }
            }

            let messageId = try proto.forwardMessage(originalMessageJson: originalMessageJson, newRecipient: newRecipient, priority: msgPriority)
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_FORWARD", fallbackMessage: "Failed to forward message")
        }
    }

    @objc func sendConnectionRequest(_ recipient: String,
                                     senderName: String,
                                     keyPackage: [NSNumber]?,
                                     initialMessage: String?,
                                     resolver: @escaping RCTPromiseResolveBlock,
                                     rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }

            let keyPackageData = keyPackage?.map { UInt8($0.intValue) }
            let messageId = try proto.sendConnectionRequest(
                recipient: recipient,
                senderName: senderName,
                keyPackage: keyPackageData,
                initialMessage: initialMessage
            )
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_CONNECTION_REQUEST", fallbackMessage: "Failed to send connection request")
        }
    }

    @objc func acceptConnectionRequest(_ recipient: String,
                                       accepterName: String,
                                       keyPackage: [NSNumber]?,
                                       resolver: @escaping RCTPromiseResolveBlock,
                                       rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }

            let keyPackageData = keyPackage?.map { UInt8($0.intValue) }
            let messageId = try proto.acceptConnectionRequest(
                recipient: recipient,
                accepterName: accepterName,
                keyPackage: keyPackageData
            )
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_CONNECTION_REQUEST", fallbackMessage: "Failed to accept connection request")
        }
    }

    @objc func rejectConnectionRequest(_ recipient: String,
                                       resolver: @escaping RCTPromiseResolveBlock,
                                       rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }

            let messageId = try proto.rejectConnectionRequest(recipient: recipient)
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_CONNECTION_REQUEST", fallbackMessage: "Failed to reject connection request")
        }
    }

    @objc func cancelConnectionRequest(_ recipient: String,
                                       resolver: @escaping RCTPromiseResolveBlock,
                                       rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }

            let messageId = try proto.cancelConnectionRequest(recipient: recipient)
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_CONNECTION_REQUEST", fallbackMessage: "Failed to cancel connection request")
        }
    }
    
    // MARK: - Service Discovery & Request/Response (via MeshServices)

    @objc func registerService(_ serviceId: String,
                               version: String,
                               capabilitiesJson: String,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let svc = meshServicesInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "MeshServices not initialized"])
            }
            var capabilities: [String: String] = [:]
            if let data = capabilitiesJson.data(using: .utf8),
               let parsed = try? JSONSerialization.jsonObject(with: data) as? [String: String] {
                capabilities = parsed
            }
            try svc.registerService(serviceId: serviceId, version: version, capabilities: capabilities)
            resolver(NSNull())
        } catch {
            rejecter("ERROR_REGISTER_SERVICE", "Failed to register service: \(error.localizedDescription)", error)
        }
    }

    @objc func unregisterService(_ serviceId: String,
                                 resolver: @escaping RCTPromiseResolveBlock,
                                 rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let svc = meshServicesInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "MeshServices not initialized"])
            }
            let removed = try svc.unregisterService(serviceId: serviceId)
            resolver(removed)
        } catch {
            rejecter("ERROR_UNREGISTER_SERVICE", "Failed to unregister service: \(error.localizedDescription)", error)
        }
    }

    @objc func discoverServices(_ serviceId: String?,
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let svc = meshServicesInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "MeshServices not initialized"])
            }
            let queryId = try svc.discoverServices(serviceId: serviceId)
            resolver(queryId)
        } catch {
            rejecter("ERROR_DISCOVER_SERVICES", "Failed to discover services: \(error.localizedDescription)", error)
        }
    }

    @objc func sendServiceRequest(_ provider: String,
                                  serviceId: String,
                                  method: String,
                                  body: String,
                                  resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let svc = meshServicesInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "MeshServices not initialized"])
            }
            let requestId = try svc.sendServiceRequest(provider: provider, serviceId: serviceId, method: method, body: body)
            resolver(requestId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_SERVICE_REQUEST", fallbackMessage: "Failed to send service request")
        }
    }

    @objc func respondToServiceRequest(_ requestId: String,
                                       requester: String,
                                       serviceId: String,
                                       status: String,
                                       body: String,
                                       resolver: @escaping RCTPromiseResolveBlock,
                                       rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let svc = meshServicesInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "MeshServices not initialized"])
            }
            let messageId = try svc.respondToServiceRequest(requestId: requestId, requester: requester, serviceId: serviceId, status: status, body: body)
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_SERVICE_RESPONSE", fallbackMessage: "Failed to respond to service request")
        }
    }

    // MARK: - User Blocking

    @objc func blockUser(_ userId: String,
                         resolver: @escaping RCTPromiseResolveBlock,
                         rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }
            try proto.blockUser(userId: userId)
            resolver(nil)
        } catch {
            rejecter("ERROR_BLOCK_USER", "Failed to block user: \(error.localizedDescription)", error)
        }
    }

    @objc func unblockUser(_ userId: String,
                           resolver: @escaping RCTPromiseResolveBlock,
                           rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }
            try proto.unblockUser(userId: userId)
            resolver(nil)
        } catch {
            rejecter("ERROR_UNBLOCK_USER", "Failed to unblock user: \(error.localizedDescription)", error)
        }
    }

    @objc func getBlockedUsers(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }
            let blocked = try proto.getBlockedUsers()
            resolver(blocked)
        } catch {
            rejecter("ERROR_GET_BLOCKED", "Failed to get blocked users: \(error.localizedDescription)", error)
        }
    }

    @objc func isUserBlocked(_ userId: String,
                             resolver: @escaping RCTPromiseResolveBlock,
                             rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }
            let blocked = try proto.isUserBlocked(userId: userId)
            resolver(blocked)
        } catch {
            rejecter("ERROR_IS_BLOCKED", "Failed to check blocked status: \(error.localizedDescription)", error)
        }
    }

    /// Reset the TOFU-pinned public key for a peer.
    @objc func resetTofuForPeer(_ peerId: String,
                                 resolver: @escaping RCTPromiseResolveBlock,
                                 rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }
            let removed = try proto.resetTofuForPeer(peerId: peerId)
            resolver(removed)
        } catch {
            rejecter("ERROR_TOFU", "Failed to reset TOFU for peer: \(error.localizedDescription)", error)
        }
    }

    // ─── Presence, Typing, Read Receipts ───────────────────────

    @objc func sendPresenceUpdate(_ recipient: String,
                                  status: Int,
                                  resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }
            let presenceStatus: PresenceStatus
            switch status {
            case 0: presenceStatus = .online
            case 1: presenceStatus = .away
            case 2: presenceStatus = .offline
            default: presenceStatus = .online
            }
            let messageId = try proto.sendPresenceUpdate(recipient: recipient, status: presenceStatus)
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_PRESENCE_UPDATE", fallbackMessage: "Failed to send presence update")
        }
    }

    @objc func sendTypingIndicator(_ recipient: String,
                                   conversationId: String,
                                   isTyping: Bool,
                                   resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }
            let messageId = try proto.sendTypingIndicator(recipient: recipient, conversationId: conversationId, isTyping: isTyping)
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_TYPING_INDICATOR", fallbackMessage: "Failed to send typing indicator")
        }
    }

    @objc func sendReadReceipt(_ recipient: String,
                               messageIds: [String],
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let proto = protocolInstance else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
            }
            let messageId = try proto.sendReadReceipt(recipient: recipient, messageIds: messageIds)
            resolver(messageId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_READ_RECEIPT", fallbackMessage: "Failed to send read receipt")
        }
    }

    @objc func receiveMessage(_ resolver: @escaping RCTPromiseResolveBlock,
                             rejecter: @escaping RCTPromiseRejectBlock) {
        if let messageJson = protocolInstance?.receiveMessage() {
            resolver(messageJson)
        } else {
            resolver(NSNull())
        }
    }
    
    @objc func destroy(_ resolver: @escaping RCTPromiseResolveBlock,
                       rejecter: @escaping RCTPromiseRejectBlock) {
        stopProcessTimer()
        bleStatusBackupWorkItem?.cancel()
        bleStatusBackupWorkItem = nil
        drainProcessQueue()
        bleManager?.stop()
        bleManager = nil
        internetManager?.stop()
        internetManager = nil
        reticulumManager?.stop()
        reticulumManager = nil
        nostrManager?.stop()
        nostrManager = nil
        do {
            try protocolInstance?.stop()
        } catch {
            // Ignore stop failures during destroy
        }
        protocolInstance = nil
        meshServicesInstance = nil
        currentConfig = nil
        resolver(nil)
    }

    /// Erases every byte of persisted SDK state for one account.
    ///
    /// Takes the identity explicitly rather than reading `currentConfig`,
    /// because the call this exists for happens *after* `destroy`, which clears
    /// it. That also lets an application wipe the account it just signed out of
    /// while a different one is already running.
    ///
    /// Refuses to wipe the account this instance is currently running: the
    /// protocol persists as it works — outbox entries on the send path, pending
    /// snapshots, sealed state records — so a wipe underneath a live instance
    /// races those writes and leaves a partially repopulated container. Call
    /// `destroy()` first.
    @objc func wipePersistedState(_ appId: String,
                                  userId: String,
                                  resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        let namespace = StorageNamespace.account(appId: appId, userId: userId)
        if let config = currentConfig,
           StorageNamespace.account(appId: config.appId, userId: config.userId) == namespace {
            rejecter(
                "ERROR_WIPE_STATE",
                "Refusing to wipe storage for the account this instance is running. "
                    + "Call destroy() first.",
                nil
            )
            return
        }

        // Secure storage first: it holds the key every sealed protocol-state
        // record is written under, so an interrupted wipe leaves the remainder
        // as ciphertext nobody can open rather than readable state.
        var firstError: Error?
        do {
            try MlsSecureStorage.wipeAccount(accountNamespace: namespace)
        } catch {
            firstError = error
        }
        do {
            try AppContainerProtocolStateStorage.wipeAccount(accountNamespace: namespace)
        } catch {
            firstError = firstError ?? error
        }

        if let firstError {
            rejecter(
                "ERROR_WIPE_STATE",
                "Failed to wipe persisted state: \(firstError.localizedDescription)",
                firstError
            )
            return
        }
        resolver(nil)
    }

    // MARK: - Transport Management
    
    @objc func enableTransport(_ type: String,
                               config: NSDictionary?,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_TRANSPORT_ENABLE", "Protocol not initialized", nil)
            return
        }
        do {
            switch type.lowercased() {
            case "internet":
                // Configure and start Internet transport via InternetManager
                if internetManager == nil {
                    // The manager's identity must be the real user id: the
                    // control-op translator filters self out of member deltas
                    // and gates LeaveGroup on it, so a placeholder would
                    // silently corrupt relay group state. Mirrors the
                    // Android module's guard.
                    guard let userId = currentConfig?.userId else {
                        throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [
                            NSLocalizedDescriptionKey: "Cannot enable Internet transport before initialize(config)"
                        ])
                    }
                    let newManager = InternetManager(protocol: proto, deviceId: userId)
                    newManager.delegate = self
                    newManager.serverMessageEmitter = { [weak self] rawJson in
                        self?.emitServerMessageEvent(rawJson)
                    }
                    newManager.connectionStatusEmitter = { [weak self] connected, authenticated in
                        self?.emitInternetStatusEvent(connected: connected, authenticated: authenticated)
                    }
                    newManager.supersededEmitter = { [weak self] reason in
                        self?.emitInternetSupersededEvent(reason: reason)
                    }
                    internetManager = newManager
                    emitDiagnostic(level: "info", message: "Internet manager created on demand")
                }
                
                guard let manager = internetManager else {
                    throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Failed to create Internet manager"])
                }
                
                // Stop the manager first if it's running or still connecting
                // (to ensure a clean restart — start() rejects .starting, and
                // stop() releases the previous session either way)
                if manager.state == .running || manager.state == .starting {
                    manager.stop()
                }

                try configureAndStartInternet(manager: manager, config: config)
                emitDiagnostic(level: "info", message: "Internet transport enabled")
                
            case "wifidirect", "wifi_direct":
                // Configure and start WiFi Direct transport via WifiDirectManager
                if wifiDirectManager == nil {
                    // Create manager if not already created
                    let newManager = WifiDirectManager(protocol: proto, deviceId: currentConfig?.userId ?? "unknown")
                    newManager.delegate = self
                    wifiDirectManager = newManager
                    proto.setWifiDirectTransportCallback(callback: WifiDirectTransportCallbackImpl(wifiDirectManager: newManager))
                    emitDiagnostic(level: "info", message: "WiFi Direct manager created on demand")
                }
                
                guard let manager = wifiDirectManager else {
                    throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Failed to create WiFi Direct manager"])
                }
                
                // Stop the manager first if it's running (to ensure clean restart)
                if manager.state == .running {
                    manager.stop()
                }
                
                try manager.start()
                emitDiagnostic(level: "info", message: "WiFi Direct transport enabled")
                
            case "ble":
                // Start BLE manager if stopped
                if bleManager == nil {
                    let newManager = BleManager(protocol: proto, deviceId: currentConfig?.userId ?? "unknown")
                    newManager.delegate = self
                    bleManager = newManager
                    proto.setBleTransportCallback(callback: BleTransportCallbackImpl(bleManager: newManager))
                    emitDiagnostic(level: "info", message: "BLE manager created on demand")
                }

                guard let manager = bleManager else {
                    throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Failed to create BLE manager"])
                }

                if manager.state != .running {
                    try manager.start()
                    emitDiagnostic(level: "info", message: "BLE transport enabled")
                }

            case "reticulum":
                if reticulumManager == nil {
                    let newManager = ReticulumManager(protocol: proto, deviceId: currentConfig?.userId ?? "unknown")
                    newManager.delegate = self
                    reticulumManager = newManager
                    proto.setReticulumTransportCallback(callback: ReticulumTransportCallbackImpl(reticulumManager: newManager))
                    emitDiagnostic(level: "info", message: "Reticulum manager created on demand")
                }

                guard let manager = reticulumManager else {
                    throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Failed to create Reticulum manager"])
                }

                if manager.state == .running {
                    manager.stop()
                }

                try configureAndStartReticulum(manager: manager, config: config)
                emitDiagnostic(level: "info", message: "Reticulum transport enabled")

            case "nostr":
                if nostrManager == nil {
                    let newManager = NostrManager(protocol: proto, deviceId: currentConfig?.userId ?? "unknown")
                    newManager.delegate = self
                    nostrManager = newManager
                    proto.setNostrTransportCallback(callback: NostrTransportCallbackImpl(nostrManager: newManager))
                    emitDiagnostic(level: "info", message: "Nostr manager created on demand")
                }

                guard let manager = nostrManager else {
                    throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Failed to create Nostr manager"])
                }

                if manager.state == .running {
                    manager.stop()
                }

                try configureAndStartNostr(manager: manager, config: config)
                emitDiagnostic(level: "info", message: "Nostr transport enabled")

            default:
                throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Unsupported transport type: \(type)"])
            }
            resolver(nil)
        } catch {
            rejecter("ERROR_TRANSPORT_ENABLE", "Failed to enable transport: \(error.localizedDescription)", error)
        }
    }
    
    private func configureAndStartInternet(manager: InternetManager, config: NSDictionary?) throws {
        let serverAddress = ((config?["serverAddress"] as? String) ?? (config?["server_url"] as? String))?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let address = serverAddress, !address.isEmpty else {
            throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Internet transport requires a serverAddress"])
        }
        
        // Build WebSocket URL
        var wsUrl = address
        if !wsUrl.hasPrefix("ws://") && !wsUrl.hasPrefix("wss://") {
            // Default to wss:// for secure connection
            wsUrl = "wss://\(wsUrl)"
        }
        
        // Append port if specified
        if let portNumber = config?["port"] as? NSNumber ?? config?["serverPort"] as? NSNumber {
            if let url = URL(string: wsUrl), url.port == nil {
                wsUrl = "\(wsUrl):\(portNumber.intValue)"
            }
        }
        
        let autoReconnect = (config?["autoReconnect"] as? Bool) ?? true
        let maxRetries = (config?["maxReconnectAttempts"] as? Int) ?? 0
        
        // Set auth token if provided in config
        if let authToken = config?["authToken"] as? String {
            manager.setAuthToken(authToken)
        }
        
        // Internet transport is already registered during protocol initialization
        // Just configure and start the WebSocket manager
        try manager.configure(serverUrl: wsUrl, autoReconnect: autoReconnect, maxReconnectAttempts: maxRetries)
        try manager.start()
        
        emitDiagnostic(level: "info", message: "Internet transport enabled", context: [
            "serverUrl": wsUrl,
            "autoReconnect": autoReconnect,
            "hasAuthToken": config?["authToken"] != nil
        ])
    }

    private func configureAndStartReticulum(manager: ReticulumManager, config: NSDictionary?) throws {
        let daemonAddress = (config?["daemonAddress"] as? String) ?? (config?["daemon_address"] as? String) ?? "localhost:4242"
        let autoReconnect = (config?["autoReconnect"] as? Bool) ?? true
        let maxRetries = (config?["maxReconnectAttempts"] as? Int) ?? 0

        manager.configure(daemonAddress: daemonAddress, autoReconnect: autoReconnect, maxReconnectAttempts: maxRetries)
        try manager.start()

        emitDiagnostic(level: "info", message: "Reticulum transport enabled", context: [
            "daemonAddress": daemonAddress,
            "autoReconnect": autoReconnect
        ])
    }

    private func configureAndStartNostr(manager: NostrManager, config: NSDictionary?) throws {
        let relayUrls = (config?["relayUrls"] as? [String]) ?? (config?["relay_urls"] as? [String]) ?? []
        guard !relayUrls.isEmpty else {
            throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Nostr transport requires at least one relay URL"])
        }
        let autoReconnect = (config?["autoReconnect"] as? Bool) ?? true
        let maxRetries = (config?["maxReconnectAttempts"] as? Int) ?? 0
        let connectionTimeout = (config?["connectionTimeout"] as? Double) ?? 30.0

        manager.configure(relayUrls: relayUrls, autoReconnect: autoReconnect, maxReconnectAttempts: maxRetries, connectionTimeout: connectionTimeout)
        try manager.start()

        emitDiagnostic(level: "info", message: "Nostr transport enabled", context: [
            "relayCount": relayUrls.count,
            "autoReconnect": autoReconnect
        ])
    }

    @objc func disableTransport(_ type: String,
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_TRANSPORT_DISABLE", "Protocol not initialized", nil)
            return
        }
        do {
            // Stop corresponding transport manager and mark transport as unavailable
            // Note: We don't remove the transport from the Rust protocol anymore
            // because that prevents re-enabling it. Instead, we just stop the manager
            // and the transport status will be updated to unavailable/disconnected.
            switch type.lowercased() {
            case "internet":
                internetManager?.stop()
                // Notify the protocol that internet is disconnected
                try? proto.internetStatusChanged(isConnected: false)
                emitDiagnostic(level: "info", message: "Internet transport disabled (manager stopped)")
            case "wifidirect", "wifi_direct":
                wifiDirectManager?.stop()
                emitDiagnostic(level: "info", message: "WiFi Direct transport disabled (manager stopped)")
            case "ble":
                bleManager?.stop()
                try? proto.bleStatusChanged(isAvailable: false)
                emitDiagnostic(level: "info", message: "BLE transport disabled (manager stopped)")
            case "reticulum":
                reticulumManager?.stop()
                try? proto.reticulumStatusChanged(isConnected: false)
                emitDiagnostic(level: "info", message: "Reticulum transport disabled (manager stopped)")
            case "nostr":
                nostrManager?.stop()
                try? proto.nostrStatusChanged(isConnected: false)
                emitDiagnostic(level: "info", message: "Nostr transport disabled (manager stopped)")
            default:
                throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Unsupported transport type: \(type)"])
            }
            
            resolver(nil)
        } catch {
            rejecter("ERROR_TRANSPORT_DISABLE", "Failed to disable transport: \(error.localizedDescription)", error)
        }
    }
    
    
    @objc func isBluetoothEnabled(_ resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        // On iOS, we can check the CBCentralManager state
        // However, checking state requires instantiation and authorization
        // For simplicity, return true as iOS will prompt when BLE is actually used
        let state = bleManager?.bluetoothState ?? .unknown
        switch state {
        case .poweredOn:
            resolver(true)
        case .poweredOff:
            resolver(false)
        case .unauthorized, .unsupported:
            resolver(false)
        default:
            // Unknown or resetting - assume enabled
            resolver(true)
        }
    }
    
    @objc func requestEnableBluetooth(_ resolver: @escaping RCTPromiseResolveBlock,
                                      rejecter: @escaping RCTPromiseRejectBlock) {
        // iOS doesn't allow programmatic Bluetooth enabling
        // The system will prompt when BLE is used
        // Return false to indicate the app should show a manual prompt
        resolver(false)
    }
    
    @objc func getTopology(_ resolver: @escaping RCTPromiseResolveBlock,
                           rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_TOPOLOGY", "Protocol not initialized", nil)
            return
        }
        do {
            let topology = try proto.getTopology()
            let json = try buildTopologyJson(topology)
            resolver(json)
        } catch {
            rejecter("ERROR_TOPOLOGY", "Failed to get topology: \(error.localizedDescription)", error)
        }
    }
    
    @objc func getMessageStats(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESSAGE_STATS", "Protocol not initialized", nil)
            return
        }
        do {
            let stats = proto.getMessageStats()
            let json = try buildMessageStatsJson(stats)
            resolver(json)
        } catch {
            rejecter("ERROR_MESSAGE_STATS", "Failed to get message stats: \(error.localizedDescription)", error)
        }
    }
    
    @objc func getDeliverySuccessRate(_ resolver: @escaping RCTPromiseResolveBlock,
                                      rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_DELIVERY_RATE", "Protocol not initialized", nil)
            return
        }
        let rate = Double(proto.getDeliverySuccessRate())
        resolver(rate)
    }
    
    @objc func getMedianLatency(_ resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MEDIAN_LATENCY", "Protocol not initialized", nil)
            return
        }
        let latency = proto.getMedianLatency()
        if latency == 0 {
            resolver(NSNull())
        } else {
            resolver(NSNumber(value: latency))
        }
    }
    
    @objc func getMedianHops(_ resolver: @escaping RCTPromiseResolveBlock,
                             rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MEDIAN_HOPS", "Protocol not initialized", nil)
            return
        }
        let hops = proto.getMedianHops()
        if hops == 0 {
            resolver(NSNull())
        } else {
            resolver(NSNumber(value: Int(hops)))
        }
    }
    
    @objc func sendFile(_ recipient: String,
                        fileData: String,
                        fileName: String,
                        resolver: @escaping RCTPromiseResolveBlock,
                        rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_SEND_FILE", "Protocol not initialized", nil)
            return
        }
        do {
            guard let data = Data(base64Encoded: fileData) else {
                rejecter("ERROR_SEND_FILE", "Invalid base64 file data", nil)
                return
            }
            let fileId = try proto.sendFile(recipient: recipient, fileData: Array(data), fileName: fileName)
            resolver(fileId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_SEND_FILE", fallbackMessage: "Failed to send file")
        }
    }
    
    @objc func sendMedia(_ recipient: String,
                         fileData: String,
                         fileName: String,
                         contentType: String,
                         mediaMetadata: NSDictionary?,
                         resolver: @escaping RCTPromiseResolveBlock,
                         rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_SEND_MEDIA", "Protocol not initialized", nil)
            return
        }
        do {
            guard let data = Data(base64Encoded: fileData) else {
                rejecter("ERROR_SEND_MEDIA", "Invalid base64 file data", nil)
                return
            }
            let ct = parseContentType(contentType)
            var meta: MediaMetadata? = nil
            if let dict = mediaMetadata as? [String: Any] {
                meta = MediaMetadata(
                    mimeType: dict["mime_type"] as? String ?? "",
                    fileName: dict["file_name"] as? String ?? fileName,
                    fileSize: (dict["file_size"] as? NSNumber)?.uint64Value ?? 0,
                    durationMs: (dict["duration_ms"] as? NSNumber)?.uint64Value,
                    width: (dict["width"] as? NSNumber)?.uint32Value,
                    height: (dict["height"] as? NSNumber)?.uint32Value,
                    thumbnailBase64: dict["thumbnail_base64"] as? String
                )
            }
            let fileId = try proto.sendMedia(recipient: recipient, fileData: Array(data), fileName: fileName, contentType: ct, mediaMetadata: meta)
            resolver(fileId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_SEND_MEDIA", fallbackMessage: "Failed to send media")
        }
    }

    @objc func sendMediaRich(_ recipient: String,
                             fileData: String,
                             fileName: String,
                             contentType: String,
                             options: NSDictionary?,
                             resolver: @escaping RCTPromiseResolveBlock,
                             rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_SEND_MEDIA", "Protocol not initialized", nil)
            return
        }
        do {
            guard let data = Data(base64Encoded: fileData) else {
                rejecter("ERROR_SEND_MEDIA", "Invalid base64 file data", nil)
                return
            }
            let ct = parseContentType(contentType)
            let dict = options as? [String: Any]
            let sendOptions = MediaSendOptions(
                mediaMetadata: parseRichMediaMetadata(dict?["media_metadata"] as? [String: Any]),
                caption: dict?["caption"] as? String,
                replyToMsg: dict?["reply_to_msg"] as? String,
                replyContext: parseReplyContext(dict?["reply_context"] as? [String: Any]),
                forwardInfo: parseForwardInfo(dict?["forward_info"] as? [String: Any]),
                fileId: dict?["file_id"] as? String
            )
            let fileId = try proto.sendMediaRich(recipient: recipient, fileData: Array(data), fileName: fileName, contentType: ct, options: sendOptions)
            resolver(fileId)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_SEND_MEDIA", fallbackMessage: "Failed to send media")
        }
    }

    private func parseContentType(_ value: String) -> ContentType {
        switch value.lowercased() {
        case "text": return .text
        case "image": return .image
        case "video": return .video
        case "audio": return .audio
        case "voice_note": return .voiceNote
        case "video_note": return .videoNote
        case "file": return .file
        case "file_chunk": return .fileChunk
        case "poll": return .poll
        default: return .file
        }
    }
    
    @objc func getFileProgress(_ fileId: String,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_FILE_PROGRESS", "Protocol not initialized", nil)
            return
        }
        if let progress = proto.getFileProgress(fileId: fileId) {
            let result: [String: Any] = [
                "file_id": progress.fileId,
                "chunks_sent": Int(progress.chunksSent),
                "total_chunks": Int(progress.totalChunks),
                "percentage": Int(progress.percentage)
            ]
            resolver(result)
        } else {
            resolver(NSNull())
        }
    }
    
    @objc func cancelFileTransfer(_ fileId: String,
                                  resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_FILE_CANCEL", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.cancelFileTransfer(fileId: fileId)
            resolver(true)
        } catch {
            if error.localizedDescription.localizedCaseInsensitiveContains("not found") {
                resolver(false)
            } else {
                rejecter("ERROR_FILE_CANCEL", "Failed to cancel file transfer: \(error.localizedDescription)", error)
            }
        }
    }
    
    // MARK: - BLE Transport Methods
    
    @objc func blePeerDiscovered(_ peerId: String,
                                 rssi: Int,
                                 resolver: @escaping RCTPromiseResolveBlock,
                                 rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_BLE", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.blePeerDiscovered(peerId: peerId, rssi: Int16(rssi))
            resolver(nil)
        } catch {
            rejecter("ERROR_BLE", "BLE peer discovered failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func blePeerLost(_ peerId: String,
                          resolver: @escaping RCTPromiseResolveBlock,
                          rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_BLE", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.blePeerLost(peerId: peerId)
            resolver(nil)
        } catch {
            rejecter("ERROR_BLE", "BLE peer lost failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func bleStatusChanged(_ isAvailable: Bool,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_BLE", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.bleStatusChanged(isAvailable: isAvailable)
            resolver(nil)
        } catch {
            rejecter("ERROR_BLE", "BLE status changed failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func bleFragmentReceived(_ senderId: String,
                                   fragmentData: [NSNumber],
                                   resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_BLE", "Protocol not initialized", nil)
            return
        }
        do {
            let fragment = fragmentData.map { UInt8($0.intValue) }
            try proto.bleFragmentReceived(senderId: senderId, fragment: fragment)
            resolver(nil)
        } catch {
            rejecter("ERROR_BLE", "BLE fragment received failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func bleGetNextFragment(_ resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_BLE", "Protocol not initialized", nil)
            return
        }
        if let fragment = proto.bleGetNextFragment() {
            let dict: [String: Any] = [
                "recipientId": fragment.recipientId,
                "data": fragment.data.map { NSNumber(value: $0) }
            ]
            resolver(dict)
        } else {
            resolver(NSNull())
        }
    }
    
    @objc func bleReturnFragment(_ resolver: @escaping RCTPromiseResolveBlock,
                                 rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_BLE", "Protocol not initialized", nil)
            return
        }
        proto.bleReturnFragment()
        resolver(nil)
    }
    
    @objc func bleGetPeerCount(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_BLE", "Protocol not initialized", nil)
            return
        }
        let count = proto.bleGetPeerCount()
        resolver(NSNumber(value: count))
    }
    
    @objc func getActiveTransports(_ resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_TRANSPORT", "Protocol not initialized", nil)
            return
        }
        let transports = proto.getActiveTransports()
        resolver(transports)
    }
    
    @objc func getState(_ resolver: @escaping RCTPromiseResolveBlock,
                       rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            resolver("Stopped")
            return
        }
        let state = proto.getState()
        let stateString: String
        switch state {
        case .stopped:
            stateString = "Stopped"
        case .running:
            stateString = "Running"
        case .paused:
            stateString = "Paused"
        @unknown default:
            stateString = "Unknown"
        }
        resolver(stateString)
    }
    
    // MARK: - Battery Management
    
    @objc func setBatteryLevel(_ level: Int,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_BATTERY", "Protocol not initialized", nil)
            return
        }
        proto.setBatteryLevel(level: UInt8(min(100, max(0, level))))
        resolver(nil)
    }
    
    @objc func getBatteryLevel(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_BATTERY", "Protocol not initialized", nil)
            return
        }
        if let level = proto.getBatteryLevel() {
            resolver(NSNumber(value: level))
        } else {
            resolver(NSNull())
        }
    }
    
    // MARK: - Relay Management
    
    @objc func setRelayPriority(_ priorityString: String,
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_RELAY", "Protocol not initialized", nil)
            return
        }
        do {
            let priority: RelayPriority
            switch priorityString.lowercased() {
            case "low":
                priority = .low
            case "high":
                priority = .high
            default:
                priority = .medium
            }
            
            try proto.setRelayPriority(priority: priority)
            resolver(nil)
        } catch {
            rejecter("ERROR_RELAY", "Failed to set relay priority: \(error.localizedDescription)", error)
        }
    }
    
    @objc func getRelayPriority(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            resolver("medium")
            return
        }
        let priority = proto.getRelayPriority()
        let priorityString: String
        switch priority {
        case .low:
            priorityString = "low"
        case .medium:
            priorityString = "medium"
        case .high:
            priorityString = "high"
        @unknown default:
            priorityString = "medium"
        }
        resolver(priorityString)
    }
    
    @objc func isRelay(_ resolver: @escaping RCTPromiseResolveBlock,
                      rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            resolver(false)
            return
        }
        let isRelay = proto.isRelay()
        resolver(NSNumber(value: isRelay))
    }
    
    // MARK: - Transport Metrics
    
    @objc func getTransportMetrics(_ transportType: String,
                                   resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_METRICS", "Protocol not initialized", nil)
            return
        }
        let type: TransportType
        switch transportType.lowercased() {
        case "ble":
            type = .ble
        case "wifidirect":
            type = .wiFiDirect
        case "internet":
            type = .internet
        default:
            type = .ble
        }
        
        if let metrics = proto.getTransportMetrics(transportType: type) {
            let metricsDict: [String: Any] = [
                "packetsSent": NSNumber(value: metrics.packetsSent),
                "packetsReceived": NSNumber(value: metrics.packetsReceived),
                "bytesSent": NSNumber(value: metrics.bytesSent),
                "bytesReceived": NSNumber(value: metrics.bytesReceived),
                "errorRate": NSNumber(value: metrics.errorRate),
                "avgLatencyMs": NSNumber(value: metrics.avgLatencyMs)
            ]
            resolver(metricsDict)
        } else {
            resolver(NSNull())
        }
    }
    
    // MARK: - Manual Transport Control
    
    @objc func forceTransport(_ transportType: String,
                             resolver: @escaping RCTPromiseResolveBlock,
                             rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_TRANSPORT", "Protocol not initialized", nil)
            return
        }
        do {
            let type: TransportType
            switch transportType.lowercased() {
            case "ble":
                type = .ble
            case "wifidirect":
                type = .wiFiDirect
            case "internet":
                type = .internet
            default:
                type = .ble
            }
            
            try proto.forceTransport(transportType: type)
            resolver(nil)
        } catch {
            rejecter("ERROR_TRANSPORT", "Failed to force transport: \(error.localizedDescription)", error)
        }
    }
    
    @objc func releaseTransportLock(_ resolver: @escaping RCTPromiseResolveBlock,
                                    rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_TRANSPORT", "Protocol not initialized", nil)
            return
        }
        proto.releaseTransportLock()
        resolver(nil)
    }
    
    // MARK: - DORS Configuration
    
    @objc func updateDorsConfig(_ configJson: String,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_CONFIG", "Protocol not initialized", nil)
            return
        }
        do {
            guard let jsonData = configJson.data(using: .utf8),
                  let config = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Invalid JSON"])
            }

            let poorSignalDuration = (config["poorSignalDurationSecs"] as? NSNumber)?.uint64Value ?? 10
            let ttlThreshold = (config["ttlEscalationThreshold"] as? NSNumber)?.uint8Value ?? 2
            let congestionDuration = max((config["congestionDurationSecs"] as? NSNumber)?.uint64Value ?? 10, 0)
            let ttlHold = max((config["ttlEscalationHoldSecs"] as? NSNumber)?.uint64Value ?? 20, 1)
            let historyWindowRaw = (config["historyWindowSize"] as? NSNumber)?.uint64Value ?? 10
            let historyWindow = max(1, min(100, Int(historyWindowRaw)))
            let rawQueueRecovery = (config["queueRecoveryRatio"] as? NSNumber)?.floatValue ?? 0.5
            let queueRecovery = min(max(rawQueueRecovery, 0.0), 1.0)
            
            let minSuccessRate = (config["minSuccessRateBeforeEscalation"] as? NSNumber)?.floatValue ?? 0.3
            let minSuccessRateClamped = min(max(minSuccessRate, 0.0), 1.0)
            let minBleSamples = (config["minBleSamplesBeforeSuccessRateEscalation"] as? NSNumber)?.uint64Value ?? 5
            let dorsConfig = DorsConfig(
                preferOnline: config["preferOnline"] as? Bool ?? false,
                switchHysteresis: max((config["switchHysteresis"] as? NSNumber)?.floatValue ?? 15.0, 0),
                switchCooldownSecs: max((config["switchCooldownSecs"] as? NSNumber)?.uint64Value ?? 20, 0),
                bleToWifiRetryThreshold: (config["bleToWifiRetryThreshold"] as? NSNumber)?.uint32Value ?? 2,
                minSuccessRateBeforeEscalation: minSuccessRateClamped,
                minBleSamplesBeforeSuccessRateEscalation: minBleSamples,
                rssiSwitchThreshold: (config["rssiSwitchThreshold"] as? NSNumber)?.int16Value ?? -85,
                congestionQueueThreshold: (config["congestionQueueThreshold"] as? NSNumber)?.uint64Value ?? 50,
                stabilityWindowSecs: (config["stabilityWindowSecs"] as? NSNumber)?.uint64Value ?? 8,
                poorSignalDurationSecs: poorSignalDuration,
                ttlEscalationThreshold: ttlThreshold,
                congestionDurationSecs: UInt64(congestionDuration),
                ttlEscalationHoldSecs: UInt64(ttlHold),
                historyWindowSize: UInt64(historyWindow),
                queueRecoveryRatio: queueRecovery
            )
            
            try proto.updateDorsConfig(config: dorsConfig)
            resolver(nil)
        } catch {
            rejecter("ERROR_CONFIG", "Failed to update DORS config: \(error.localizedDescription)", error)
        }
    }
    
    @objc func getDorsConfig(_ resolver: @escaping RCTPromiseResolveBlock,
                            rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_CONFIG", "Protocol not initialized", nil)
            return
        }
        let config = proto.getDorsConfig()
        let configDict: [String: Any] = [
            "preferOnline": config.preferOnline,
            "switchHysteresis": config.switchHysteresis,
            "switchCooldownSecs": config.switchCooldownSecs,
            "bleToWifiRetryThreshold": config.bleToWifiRetryThreshold,
            "minSuccessRateBeforeEscalation": config.minSuccessRateBeforeEscalation,
            "minBleSamplesBeforeSuccessRateEscalation": config.minBleSamplesBeforeSuccessRateEscalation,
            "rssiSwitchThreshold": config.rssiSwitchThreshold,
            "congestionQueueThreshold": config.congestionQueueThreshold,
            "stabilityWindowSecs": config.stabilityWindowSecs,
            "poorSignalDurationSecs": config.poorSignalDurationSecs,
            "ttlEscalationThreshold": config.ttlEscalationThreshold,
            "congestionDurationSecs": config.congestionDurationSecs,
            "ttlEscalationHoldSecs": config.ttlEscalationHoldSecs,
            "historyWindowSize": config.historyWindowSize,
            "queueRecoveryRatio": config.queueRecoveryRatio
        ]
        resolver(configDict)
    }
    
    // MARK: - Reliability Configuration
    
    @objc func updateAckConfig(_ configJson: String,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_CONFIG", "Protocol not initialized", nil)
            return
        }
        do {
            guard let jsonData = configJson.data(using: .utf8),
                  let config = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                             userInfo: [NSLocalizedDescriptionKey: "Invalid ACK config JSON"])
            }
            
            let ackConfig = AckConfig(
                defaultTimeoutMs: (config["defaultTimeoutMs"] as? NSNumber)?.uint64Value ?? 10000,
                maxPendingAcks: (config["maxPendingAcks"] as? NSNumber)?.uint64Value ?? 1000
            )
            
            try proto.updateAckConfig(config: ackConfig)
            resolver(nil)
        } catch {
            rejecter("ERROR_CONFIG", "Failed to update ACK config: \(error.localizedDescription)", error)
        }
    }
    
    @objc func updateRetryConfig(_ configJson: String,
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_CONFIG", "Protocol not initialized", nil)
            return
        }
        do {
            guard let jsonData = configJson.data(using: .utf8),
                  let config = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                             userInfo: [NSLocalizedDescriptionKey: "Invalid retry config JSON"])
            }
            
            // Fallbacks must mirror the Rust defaults in
            // offline-protocol-reliability/src/constants.rs: a partial config
            // rebuilds the whole RetryConfig, so a stale fallback silently
            // overrides the SDK default for every field the app didn't set.
            let retryConfig = RetryConfig(
                maxRetries: (config["maxRetries"] as? NSNumber)?.uint32Value ?? 10,
                initialDelayMs: (config["initialDelayMs"] as? NSNumber)?.uint64Value ?? 1000,
                maxDelayMs: (config["maxDelayMs"] as? NSNumber)?.uint64Value ?? 300000,
                backoffMultiplier: (config["backoffMultiplier"] as? NSNumber)?.floatValue ?? 2.0,
                outboxMaxLifetimeMs: (config["outboxMaxLifetimeMs"] as? NSNumber)?.uint64Value ?? 604800000,
                pendingMessageMaxLifetimeMs: (config["pendingMessageMaxLifetimeMs"] as? NSNumber)?.uint64Value ?? 604800000
            )
            
            try proto.updateRetryConfig(config: retryConfig)
            resolver(nil)
        } catch {
            rejecter("ERROR_CONFIG", "Failed to update retry config: \(error.localizedDescription)", error)
        }
    }
    
    @objc func updateDedupConfig(_ configJson: String,
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_CONFIG", "Protocol not initialized", nil)
            return
        }
        do {
            guard let jsonData = configJson.data(using: .utf8),
                  let config = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                             userInfo: [NSLocalizedDescriptionKey: "Invalid dedup config JSON"])
            }
            
            let dedupConfig = DedupConfig(
                maxTrackedMessages: (config["maxTrackedMessages"] as? NSNumber)?.uint64Value ?? 10000,
                retentionTimeSecs: (config["retentionTimeSecs"] as? NSNumber)?.uint64Value ?? 3600
            )
            
            try proto.updateDedupConfig(config: dedupConfig)
            resolver(nil)
        } catch {
            rejecter("ERROR_CONFIG", "Failed to update dedup config: \(error.localizedDescription)", error)
        }
    }
    
    @objc func getDedupStats(_ resolver: @escaping RCTPromiseResolveBlock,
                            rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_STATS", "Protocol not initialized", nil)
            return
        }
        let stats = proto.getDedupStats()
        let statsDict: [String: Any] = [
            "totalTracked": stats.totalTracked,
            "recentTracked": stats.recentTracked,
            "capacityUsedPercent": stats.capacityUsedPercent,
            "mode": stats.mode
        ]
        resolver(statsDict)
    }
    
    @objc func getPendingAckCount(_ resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_STATS", "Protocol not initialized", nil)
            return
        }
        resolver(NSNumber(value: proto.getPendingAckCount()))
    }
    
    @objc func getRetryQueueSize(_ resolver: @escaping RCTPromiseResolveBlock,
                                 rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_STATS", "Protocol not initialized", nil)
            return
        }
        resolver(NSNumber(value: proto.getRetryQueueSize()))
    }
    
    // MARK: - Gradient Routing
    
    @objc func learnRoute(_ destination: String,
                          nextHop: String,
                          hopCount: Int,
                          quality: Double,
                          sequenceNumber: NSNumber,
                          resolver: @escaping RCTPromiseResolveBlock,
                          rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_ROUTING", "Protocol not initialized", nil)
            return
        }
        // Clamp to match Android (coerceAtLeast(0)); avoids negative wrapping to uint32 (e.g. -1 → 2^32-1).
        let seq = max(0, sequenceNumber.intValue)
        proto.learnRoute(
            destination: destination,
            nextHop: nextHop,
            hopCount: UInt8(min(255, max(0, hopCount))),
            quality: Float(quality),
            sequenceNumber: UInt32(seq)
        )
        resolver(nil)
    }
    
    @objc func getBestRoute(_ destination: String,
                            resolver: @escaping RCTPromiseResolveBlock,
                            rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_ROUTING", "Protocol not initialized", nil)
            return
        }
        if let route = proto.getBestRoute(destination: destination) {
            let routeDict: [String: Any] = [
                "nextHop": route.nextHop,
                "hopCount": Int(route.hopCount),
                "quality": Double(route.quality),
                "lastSeenMs": Int(route.lastSeenMs)
            ]
            resolver(routeDict)
        } else {
            resolver(NSNull())
        }
    }
    
    @objc func getAllRoutes(_ destination: String,
                            resolver: @escaping RCTPromiseResolveBlock,
                            rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_ROUTING", "Protocol not initialized", nil)
            return
        }
        let routes = proto.getAllRoutes(destination: destination)
        let routesArray = routes.map { route -> [String: Any] in
            [
                "nextHop": route.nextHop,
                "hopCount": Int(route.hopCount),
                "quality": Double(route.quality),
                "lastSeenMs": Int(route.lastSeenMs)
            ]
        }
        resolver(routesArray)
    }
    
    @objc func hasRoute(_ destination: String,
                        resolver: @escaping RCTPromiseResolveBlock,
                        rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_ROUTING", "Protocol not initialized", nil)
            return
        }
        let exists = proto.hasRoute(destination: destination)
        resolver(NSNumber(value: exists))
    }
    
    @objc func removeNeighborRoutes(_ neighborId: String,
                                    resolver: @escaping RCTPromiseResolveBlock,
                                    rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_ROUTING", "Protocol not initialized", nil)
            return
        }
        proto.removeNeighborRoutes(neighborId: neighborId)
        resolver(nil)
    }
    
    @objc func cleanupExpiredRoutes(_ resolver: @escaping RCTPromiseResolveBlock,
                                    rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_ROUTING", "Protocol not initialized", nil)
            return
        }
        proto.cleanupExpiredRoutes()
        resolver(nil)
    }
    
    @objc func getRoutingStats(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_ROUTING", "Protocol not initialized", nil)
            return
        }
        let stats = proto.getRoutingStats()
        let statsDict: [String: Any] = [
            "destinationCount": Int(stats.destinationCount),
            "routeCount": Int(stats.routeCount)
        ]
        resolver(statsDict)
    }
    
    @objc func updateRoutingConfig(_ configJson: String,
                                   resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_ROUTING", "Protocol not initialized", nil)
            return
        }
        do {
            guard let jsonData = configJson.data(using: .utf8),
                  let config = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                             userInfo: [NSLocalizedDescriptionKey: "Invalid routing config JSON"])
            }
            
            let routingConfig = GradientRoutingConfig(
                maxRoutesPerDestination: (config["maxRoutesPerDestination"] as? NSNumber)?.uint32Value ?? 3,
                routeTtlSecs: (config["routeTtlSecs"] as? NSNumber)?.uint64Value ?? 300,
                maxRoutingTableSize: (config["maxRoutingTableSize"] as? NSNumber)?.uint32Value ?? 1000
            )
            
            proto.updateRoutingConfig(config: routingConfig)
            resolver(nil)
        } catch {
            rejecter("ERROR_ROUTING", "Failed to update routing config: \(error.localizedDescription)", error)
        }
    }
    
    // MARK: - DORS Decision Support
    
    @objc func shouldEscalateToWifi(_ resolver: @escaping RCTPromiseResolveBlock,
                                    rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_DORS", "Protocol not initialized", nil)
            return
        }
        let shouldEscalate = proto.shouldEscalateToWifi()
        resolver(NSNumber(value: shouldEscalate))
    }
    
    // MARK: - File Transfer Operations
    
    @objc func processFileChunk(_ fileId: String,
                                chunkIndex: Int,
                                totalChunks: Int,
                                fileSize: Double,
                                fileName: String,
                                fileChecksum: String,
                                data: [NSNumber],
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_FILE", "Protocol not initialized", nil)
            return
        }
        do {
            let bytes = data.map { UInt8($0.intValue) }
            try proto.processFileChunk(
                fileId: fileId,
                chunkIndex: UInt32(chunkIndex),
                totalChunks: UInt32(totalChunks),
                fileSize: UInt64(fileSize),
                fileName: fileName,
                fileChecksum: fileChecksum,
                data: bytes
            )
            resolver(nil)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_FILE", fallbackMessage: "Failed to process file chunk")
        }
    }
    
    @objc func finalizeFile(_ fileId: String,
                            resolver: @escaping RCTPromiseResolveBlock,
                            rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_FILE", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.finalizeFile(fileId: fileId)
            resolver(nil)
        } catch {
            rejecter("ERROR_FILE", "Failed to finalize file: \(error.localizedDescription)", error)
        }
    }
    
    // MARK: - WiFi Direct Transport Methods
    
    @objc func wifiDirectStatusChanged(_ isConnected: Bool,
                                       resolver: @escaping RCTPromiseResolveBlock,
                                       rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_WIFI_DIRECT", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.wifiDirectStatusChanged(isConnected: isConnected)
            resolver(nil)
        } catch {
            rejecter("ERROR_WIFI_DIRECT", "WiFi Direct status changed failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func wifiDirectMessageReceived(_ senderId: String,
                                         data: [NSNumber],
                                         resolver: @escaping RCTPromiseResolveBlock,
                                         rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_WIFI_DIRECT", "Protocol not initialized", nil)
            return
        }
        do {
            let bytes = data.map { UInt8($0.intValue) }
            try proto.wifiDirectMessageReceived(senderId: senderId, data: bytes)
            resolver(nil)
        } catch {
            rejecter("ERROR_WIFI_DIRECT", "WiFi Direct message received failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func wifiDirectGetNextMessage(_ resolver: @escaping RCTPromiseResolveBlock,
                                        rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_WIFI_DIRECT", "Protocol not initialized", nil)
            return
        }
        if let message = proto.wifiDirectGetNextMessage() {
            let dict: [String: Any] = [
                "recipientId": message.recipientId,
                "data": message.data.map { NSNumber(value: $0) }
            ]
            resolver(dict)
        } else {
            resolver(NSNull())
        }
    }
    
    @objc func wifiDirectPeerConnected(_ peerId: String,
                                       resolver: @escaping RCTPromiseResolveBlock,
                                       rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_WIFI_DIRECT", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.wifiDirectPeerConnected(peerId: peerId)
            resolver(nil)
        } catch {
            rejecter("ERROR_WIFI_DIRECT", "WiFi Direct peer connected failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func wifiDirectPeerDisconnected(_ peerId: String,
                                          resolver: @escaping RCTPromiseResolveBlock,
                                          rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_WIFI_DIRECT", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.wifiDirectPeerDisconnected(peerId: peerId)
            resolver(nil)
        } catch {
            rejecter("ERROR_WIFI_DIRECT", "WiFi Direct peer disconnected failed: \(error.localizedDescription)", error)
        }
    }
    
    // MARK: - Internet Transport Methods
    
    @objc func internetStatusChanged(_ isConnected: Bool,
                                     resolver: @escaping RCTPromiseResolveBlock,
                                     rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_INTERNET", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.internetStatusChanged(isConnected: isConnected)
            resolver(nil)
        } catch {
            rejecter("ERROR_INTERNET", "Internet status changed failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func internetMessageReceived(_ senderId: String,
                                       data: [NSNumber],
                                       resolver: @escaping RCTPromiseResolveBlock,
                                       rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_INTERNET", "Protocol not initialized", nil)
            return
        }
        do {
            let bytes = data.map { UInt8($0.intValue) }
            try proto.internetMessageReceived(senderId: senderId, data: bytes)
            resolver(nil)
        } catch {
            rejecter("ERROR_INTERNET", "Internet message received failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func internetGetNextMessage(_ resolver: @escaping RCTPromiseResolveBlock,
                                      rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_INTERNET", "Protocol not initialized", nil)
            return
        }
        if let message = proto.internetGetNextMessage() {
            let dict: [String: Any] = [
                "messageId": message.messageId,
                "recipientId": message.recipientId,
                "data": message.data.map { NSNumber(value: $0) }
            ]
            resolver(dict)
        } else {
            resolver(NSNull())
        }
    }
    
    @objc func internetConfirmSent(_ messageId: String,
                                   resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_INTERNET", "Protocol not initialized", nil)
            return
        }
        proto.internetConfirmSent(messageId: messageId)
        resolver(nil)
    }
    
    @objc func internetSendFailed(_ messageId: String,
                                  resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_INTERNET", "Protocol not initialized", nil)
            return
        }
        proto.internetSendFailed(messageId: messageId)
        resolver(nil)
    }
    
    // MARK: - MLS (End-to-End Encryption)
    
    /// Initialize MLS with built-in secure storage (Keychain)
    @objc func initializeMlsWithSecureStorage(_ resolver: @escaping RCTPromiseResolveBlock,
                                              rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        guard let config = currentConfig else {
            rejecter("ERROR_MLS", "Protocol config not initialized", nil)
            return
        }
        do {
            let accountNamespace = StorageNamespace.account(
                appId: config.appId,
                userId: config.userId
            )
            let secureStorage = try MlsSecureStorage(
                accountNamespace: accountNamespace
            )
            let protocolStateStorage = try AppContainerProtocolStateStorage(
                accountNamespace: accountNamespace
            )
            try proto.initializeMls(
                secureStorage: secureStorage,
                protocolStateStorage: protocolStateStorage
            )
            // Either of these means this account is starting from a fresh MLS
            // identity — never let that pass silently.
            switch secureStorage.legacyAdoption {
            case .conflict?:
                emitDiagnostic(
                    level: "error",
                    message: "Legacy secure store belongs to another account; "
                        + "this account starts from a fresh MLS identity and "
                        + "cannot decrypt its previous sessions. Its pre-split "
                        + "delivery state is unreachable too, so it also comes "
                        + "up with an empty outbox and an empty block list — "
                        + "every previously blocked peer is unblocked"
                )
            case .claimUnverified?:
                emitDiagnostic(
                    level: "error",
                    message: "Could not record this account's claim on the "
                        + "legacy secure store, so it was not adopted: another "
                        + "account could otherwise inherit the same MLS "
                        + "identity. This account starts from a fresh identity "
                        + "and comes up with an empty outbox and an empty block "
                        + "list — every previously blocked peer is unblocked. "
                        + "The credential store is failing writes; retrying on "
                        + "a healthy store adopts normally"
                )
            case .adopt?, .resume?, .none:
                break
            }
            emitDiagnostic(
                level: "info",
                message: "MLS initialized with split Keychain and app-container storage"
            )
            resolver(nil)
        } catch {
            emitDiagnostic(level: "error", message: "Failed to initialize MLS", context: [
                "error": error.localizedDescription
            ])
            rejecter("ERROR_MLS", "Failed to initialize MLS: \(error.localizedDescription)", error)
        }
    }
    
    /// Check if MLS is initialized
    @objc func isMlsInitialized(_ resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            resolver(false)
            return
        }
        resolver(proto.isMlsInitialized())
    }
    
    // ========================================================================
    // IDENTITY AND SIGNING OPERATIONS
    // ========================================================================
    
    /// Get the identity public key (Ed25519, 32 bytes)
    @objc func getIdentityPublicKey(_ resolver: @escaping RCTPromiseResolveBlock,
                                    rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        do {
            let publicKey = try proto.getIdentityPublicKey()
            resolver(publicKey.map { NSNumber(value: $0) })
        } catch {
            rejecter("ERROR_CRYPTO", "Failed to get identity public key: \(error.localizedDescription)", error)
        }
    }
    
    /// Derive a user ID from a public key
    @objc func deriveUserIdFromPublicKey(_ publicKey: [NSNumber],
                                         resolver: @escaping RCTPromiseResolveBlock,
                                         rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        let publicKeyBytes = publicKey.map { $0.uint8Value }
        let userId = proto.deriveUserIdFromPublicKey(publicKey: publicKeyBytes)
        resolver(userId)
    }
    
    /// Sign data with the identity private key
    @objc func signData(_ data: [NSNumber],
                        resolver: @escaping RCTPromiseResolveBlock,
                        rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        do {
            let dataBytes = data.map { $0.uint8Value }
            let signature = try proto.signData(data: dataBytes)
            resolver(signature.map { NSNumber(value: $0) })
        } catch {
            rejecter("ERROR_CRYPTO", "Failed to sign data: \(error.localizedDescription)", error)
        }
    }
    
    /// Verify a signature against a public key
    @objc func verifySignature(_ publicKey: [NSNumber],
                               data: [NSNumber],
                               signature: [NSNumber],
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        do {
            let publicKeyBytes = publicKey.map { $0.uint8Value }
            let dataBytes = data.map { $0.uint8Value }
            let signatureBytes = signature.map { $0.uint8Value }
            let isValid = try proto.verifySignature(publicKey: publicKeyBytes, data: dataBytes, signature: signatureBytes)
            resolver(isValid)
        } catch {
            rejecter("ERROR_CRYPTO", "Failed to verify signature: \(error.localizedDescription)", error)
        }
    }
    
    /// Generate a new MLS key package
    @objc func mlsGenerateKeyPackage(_ resolver: @escaping RCTPromiseResolveBlock,
                                     rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            let bundle = try proto.mlsGenerateKeyPackage()
            let result: [String: Any] = [
                "packageId": bundle.packageId,
                "userId": bundle.userId,
                "keyPackageData": bundle.keyPackageData.map { NSNumber(value: $0) },
                "createdAtMs": NSNumber(value: bundle.createdAtMs),
                "expiresAtMs": NSNumber(value: bundle.expiresAtMs),
                "synced": bundle.synced
            ]
            resolver(result)
        } catch {
            rejecter("ERROR_MLS", "Failed to generate key package: \(error.localizedDescription)", error)
        }
    }
    
    /// Get existing or generate new key package
    @objc func mlsGetOrCreateKeyPackage(_ resolver: @escaping RCTPromiseResolveBlock,
                                        rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            let bundle = try proto.mlsGetOrCreateKeyPackage()
            let result: [String: Any] = [
                "packageId": bundle.packageId,
                "userId": bundle.userId,
                "keyPackageData": bundle.keyPackageData.map { NSNumber(value: $0) },
                "createdAtMs": NSNumber(value: bundle.createdAtMs),
                "expiresAtMs": NSNumber(value: bundle.expiresAtMs),
                "synced": bundle.synced
            ]
            resolver(result)
        } catch {
            rejecter("ERROR_MLS", "Failed to get key package: \(error.localizedDescription)", error)
        }
    }

    /// Get pending key packages to upload
    @objc func mlsGetPendingKeyPackages(_ resolver: @escaping RCTPromiseResolveBlock,
                                        rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        let bundles = proto.mlsGetPendingKeyPackages()
        let results = bundles.map { bundle -> [String: Any] in
            return [
                "packageId": bundle.packageId,
                "userId": bundle.userId,
                "keyPackageData": bundle.keyPackageData.map { NSNumber(value: $0) },
                "createdAtMs": NSNumber(value: bundle.createdAtMs),
                "expiresAtMs": NSNumber(value: bundle.expiresAtMs),
                "synced": bundle.synced
            ]
        }
        resolver(results)
    }

    /// Mark key package as synced (uploaded)
    @objc func mlsMarkKeyPackageSynced(_ packageId: String,
                                       resolver: @escaping RCTPromiseResolveBlock,
                                       rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.mlsMarkKeyPackageSynced(packageId: packageId)
            resolver(nil)
        } catch {
            rejecter("ERROR_MLS", "Failed to mark key package synced: \(error.localizedDescription)", error)
        }
    }

    /// Create a 1:1 session (returns Welcome message)
    @objc func mlsCreateSession(_ otherUserId: String,
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            let welcome = try proto.mlsCreateSession(otherUserId: otherUserId)
            let result: [String: Any] = [
                "groupId": welcome.groupId,
                "welcomeData": welcome.welcomeData.map { NSNumber(value: $0) },
                "inviterId": welcome.inviterId,
                "groupName": welcome.groupName ?? NSNull(),
                "timestampMs": NSNumber(value: welcome.timestampMs)
            ]
            resolver(result)
        } catch {
            rejecter("ERROR_MLS", "Failed to create session: \(error.localizedDescription)", error)
        }
    }

    /// Join a session from Welcome message
    @objc func mlsJoinSession(_ welcomeJson: String,
                              resolver: @escaping RCTPromiseResolveBlock,
                              rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            guard let jsonData = welcomeJson.data(using: .utf8),
                  let json = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Invalid JSON"])
            }
            
            let welcomeDataNumbers = json["welcomeData"] as? [NSNumber] ?? []
            let welcomeData = welcomeDataNumbers.map { UInt8($0.intValue) }
            
            let welcome = MlsWelcomeMessage(
                groupId: json["groupId"] as? String ?? "",
                welcomeData: welcomeData,
                inviterId: json["inviterId"] as? String ?? "",
                groupName: json["groupName"] as? String,
                timestampMs: (json["timestampMs"] as? NSNumber)?.uint64Value ?? 0
            )
            
            let info = try proto.mlsJoinSession(welcome: welcome)
            let result: [String: Any] = [
                "groupId": info.groupId,
                "name": info.name ?? NSNull(),
                "members": info.members,
                "epoch": NSNumber(value: info.epoch),
                "isSession": info.isSession,
                "createdAtMs": NSNumber(value: info.createdAtMs),
                "lastActivityMs": NSNumber(value: info.lastActivityMs)
            ]
            resolver(result)
        } catch {
            rejecter("ERROR_MLS", "Failed to join session: \(error.localizedDescription)", error)
        }
    }

    /// Decrypt a message from a user
    @objc func mlsDecryptFromUser(_ encryptedJson: String,
                                  resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            guard let jsonData = encryptedJson.data(using: .utf8),
                  let json = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Invalid JSON"])
            }
            
            let ciphertextNumbers = json["ciphertext"] as? [NSNumber] ?? []
            let ciphertext = ciphertextNumbers.map { UInt8($0.intValue) }
            
            let encrypted = MlsEncryptedMessage(
                groupId: json["groupId"] as? String ?? "",
                messageType: json["messageType"] as? String ?? "Application",
                epoch: (json["epoch"] as? NSNumber)?.uint64Value ?? 0,
                ciphertext: ciphertext,
                senderId: json["senderId"] as? String ?? "",
                timestampMs: (json["timestampMs"] as? NSNumber)?.uint64Value ?? 0
            )
            
            if let plaintext = try proto.mlsDecryptFromUser(encrypted: encrypted) {
                resolver(plaintext.map { NSNumber(value: $0) })
            } else {
                resolver(NSNull())
            }
        } catch {
            rejecter("ERROR_MLS", "Failed to decrypt message from user: \(error.localizedDescription)", error)
        }
    }

    /// Delete a session
    @objc func mlsDeleteSession(_ otherUserId: String,
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.mlsDeleteSession(otherUserId: otherUserId)
            resolver(nil)
        } catch {
            rejecter("ERROR_MLS", "Failed to delete session: \(error.localizedDescription)", error)
        }
    }
    
    /// Import a contact's key package
    @objc func mlsImportKeyPackage(_ userId: String,
                                   keyPackageData: [NSNumber],
                                   resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            let data = keyPackageData.map { UInt8($0.intValue) }
            try proto.mlsImportKeyPackage(userId: userId, keyPackageData: data)
            resolver(nil)
        } catch {
            rejecter("ERROR_MLS", "Failed to import key package: \(error.localizedDescription)", error)
        }
    }
    
    /// Check if a session exists with a user
    @objc func mlsHasSession(_ otherUserId: String,
                             resolver: @escaping RCTPromiseResolveBlock,
                             rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            resolver(false)
            return
        }
        resolver(proto.mlsHasSession(otherUserId: otherUserId))
    }
    
    /// Check if a pending key package is available for a peer
    @objc func hasPendingKeyPackage(_ peerId: String,
                                    resolver: @escaping RCTPromiseResolveBlock,
                                    rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            resolver(false)
            return
        }
        resolver(proto.hasPendingKeyPackage(peerId: peerId))
    }

    /// Returns the current session establishment state for a peer.
    @objc func getEstablishmentState(_ peerId: String,
                                     resolver: @escaping RCTPromiseResolveBlock,
                                     rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            resolver("NoKeyPackage")
            return
        }
        do {
            let state = try proto.getEstablishmentState(peerId: peerId)
            let stateString: String
            switch state {
            case .noKeyPackage:
                stateString = "NoKeyPackage"
            case .haveKeyPackage:
                stateString = "HaveKeyPackage"
            case .sessionPending:
                stateString = "SessionPending"
            case .sessionConfirmed:
                stateString = "SessionConfirmed"
            }
            resolver(stateString)
        } catch {
            rejecter("ERROR_MLS", "Failed to get establishment state: \(error.localizedDescription)", error)
        }
    }
    
    /// Establish a secure session with a peer (high-level API)
    @objc func establishSecureSession(_ peerId: String,
                                      resolver: @escaping RCTPromiseResolveBlock,
                                      rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            if let welcome = try proto.establishSecureSession(peerId: peerId) {
                let result: [String: Any] = [
                    "groupId": welcome.groupId,
                    "welcomeData": welcome.welcomeData.map { NSNumber(value: $0) },
                    "inviterId": welcome.inviterId,
                    "groupName": welcome.groupName ?? NSNull(),
                    "timestampMs": NSNumber(value: welcome.timestampMs)
                ]
                resolver(result)
            } else {
                // Session already exists
                resolver(NSNull())
            }
        } catch {
            rejecter("ERROR_MLS", "Failed to establish secure session: \(error.localizedDescription)", error)
        }
    }
    
    /// Encrypt a message for a user
    @objc func mlsEncryptForUser(_ otherUserId: String,
                                 plaintext: [NSNumber],
                                 resolver: @escaping RCTPromiseResolveBlock,
                                 rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            let data = plaintext.map { UInt8($0.intValue) }
            let encrypted = try proto.mlsEncryptForUser(otherUserId: otherUserId, plaintext: data)
            let result: [String: Any] = [
                "groupId": encrypted.groupId,
                "messageType": encrypted.messageType,
                "epoch": NSNumber(value: encrypted.epoch),
                "ciphertext": encrypted.ciphertext.map { NSNumber(value: $0) },
                "senderId": encrypted.senderId,
                "timestampMs": NSNumber(value: encrypted.timestampMs)
            ]
            resolver(result)
        } catch {
            rejecter("ERROR_MLS", "Failed to encrypt message: \(error.localizedDescription)", error)
        }
    }
    
    /// Decrypt a message
    @objc func mlsDecrypt(_ encryptedJson: String,
                          resolver: @escaping RCTPromiseResolveBlock,
                          rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            guard let jsonData = encryptedJson.data(using: .utf8),
                  let json = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Invalid JSON"])
            }
            
            let ciphertextNumbers = json["ciphertext"] as? [NSNumber] ?? []
            let ciphertext = ciphertextNumbers.map { UInt8($0.intValue) }
            
            let encrypted = MlsEncryptedMessage(
                groupId: json["groupId"] as? String ?? "",
                messageType: json["messageType"] as? String ?? "Application",
                epoch: (json["epoch"] as? NSNumber)?.uint64Value ?? 0,
                ciphertext: ciphertext,
                senderId: json["senderId"] as? String ?? "",
                timestampMs: (json["timestampMs"] as? NSNumber)?.uint64Value ?? 0
            )
            
            if let plaintext = try proto.mlsDecrypt(encrypted: encrypted) {
                resolver(plaintext.map { NSNumber(value: $0) })
            } else {
                resolver(NSNull())
            }
        } catch {
            rejecter("ERROR_MLS", "Failed to decrypt message: \(error.localizedDescription)", error)
        }
    }
    
    /// List all active sessions
    @objc func mlsListSessions(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            resolver([])
            return
        }
        resolver(proto.mlsListSessions())
    }

    /// Process a Welcome message
    @objc func mlsProcessWelcome(_ welcomeJson: String,
                                 resolver: @escaping RCTPromiseResolveBlock,
                                 rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MLS", "Protocol not initialized", nil)
            return
        }
        do {
            guard let jsonData = welcomeJson.data(using: .utf8),
                  let json = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Invalid JSON"])
            }
            
            let welcomeDataNumbers = json["welcomeData"] as? [NSNumber] ?? []
            let welcomeData = welcomeDataNumbers.map { UInt8($0.intValue) }
            
            let welcome = MlsWelcomeMessage(
                groupId: json["groupId"] as? String ?? "",
                welcomeData: welcomeData,
                inviterId: json["inviterId"] as? String ?? "",
                groupName: json["groupName"] as? String,
                timestampMs: (json["timestampMs"] as? NSNumber)?.uint64Value ?? 0
            )
            
            let info = try proto.mlsProcessWelcome(welcome: welcome)
            let result: [String: Any] = [
                "groupId": info.groupId,
                "name": info.name ?? NSNull(),
                "members": info.members,
                "epoch": NSNumber(value: info.epoch),
                "isSession": info.isSession,
                "createdAtMs": NSNumber(value: info.createdAtMs),
                "lastActivityMs": NSNumber(value: info.lastActivityMs)
            ]
            resolver(result)
        } catch {
            rejecter("ERROR_MLS", "Failed to process welcome message: \(error.localizedDescription)", error)
        }
    }
    
    // MARK: - Mesh Group Management (Protocol-Level MLS Groups)

    /// Create a new MLS group via the mesh transport layer.
    @objc func meshCreateGroup(_ groupName: String,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            let info = try proto.createGroup(groupName: groupName)
            let result: [String: Any] = [
                "groupId": info.groupId,
                "groupName": info.name ?? groupName,
                "memberIds": info.members,
                "epoch": info.epoch,
                "createdAt": info.createdAtMs
            ]
            resolver(result)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to create mesh group")
        }
    }

    /// Invite a member to an MLS group, sending Welcome+Commit via mesh transport.
    @objc func meshInviteToGroup(_ groupId: String,
                                 inviteeUserId: String,
                                 resolver: @escaping RCTPromiseResolveBlock,
                                 rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.inviteToGroup(groupId: groupId, inviteeUserId: inviteeUserId)
            resolver(nil)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to invite to mesh group")
        }
    }

    /// Send an MLS-encrypted message to all group members via mesh transport.
    @objc func meshSendGroupMessage(_ groupId: String,
                                    content: String,
                                    priority: String?,
                                    replyToMsg: String?,
                                    resolver: @escaping RCTPromiseResolveBlock,
                                    rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            var msgPriority: MessagePriority? = nil
            if let priorityStr = priority {
                switch priorityStr.lowercased() {
                case "low":
                    msgPriority = .low
                case "medium":
                    msgPriority = .medium
                case "high":
                    msgPriority = .high
                case "critical":
                    msgPriority = .critical
                default:
                    rejecter("ERROR_MESH_GROUP", "Invalid priority: \(priorityStr). Must be low, medium, high, or critical.", nil)
                    return
                }
            }
            let messageIds = try proto.sendGroupMessage(groupId: groupId, content: content, priority: msgPriority, replyToMsg: replyToMsg)
            resolver(messageIds)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to send mesh group message")
        }
    }

    /// Forward a message to all members of a group with forwarding attribution.
    @objc func meshForwardMessageToGroup(_ originalMessageJson: String,
                                         groupId: String,
                                         priority: String?,
                                         resolver: @escaping RCTPromiseResolveBlock,
                                         rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            var msgPriority: MessagePriority? = nil
            if let priorityStr = priority {
                switch priorityStr.lowercased() {
                case "low":
                    msgPriority = .low
                case "medium":
                    msgPriority = .medium
                case "high":
                    msgPriority = .high
                case "critical":
                    msgPriority = .critical
                default:
                    rejecter("ERROR_MESH_GROUP", "Invalid priority: \(priorityStr). Must be low, medium, high, or critical.", nil)
                    return
                }
            }
            let messageIds = try proto.forwardMessageToGroup(originalMessageJson: originalMessageJson, groupId: groupId, priority: msgPriority)
            resolver(messageIds)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to forward message to group")
        }
    }

    /// Remove a member from an MLS group with notification via mesh transport.
    @objc func meshRemoveFromGroup(_ groupId: String,
                                   memberId: String,
                                   resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.removeFromGroup(groupId: groupId, memberId: memberId)
            resolver(nil)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to remove member from mesh group")
        }
    }

    /// Leave an MLS group with notification via mesh transport.
    @objc func meshLeaveGroup(_ groupId: String,
                              resolver: @escaping RCTPromiseResolveBlock,
                              rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.leaveGroup(groupId: groupId)
            resolver(nil)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to leave mesh group")
        }
    }

    /// List all MLS groups the local user belongs to.
    @objc func meshListGroups(_ resolver: @escaping RCTPromiseResolveBlock,
                              rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            let groups = try proto.listGroups()
            resolver(groups)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to list mesh groups")
        }
    }

    /// Get information about an MLS group.
    @objc func meshGetGroupInfo(_ groupId: String,
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            if let info = try proto.getGroupInfo(groupId: groupId) {
                let result: [String: Any] = [
                    "groupId": info.groupId,
                    "name": info.name ?? NSNull(),
                    "members": info.members,
                    "epoch": NSNumber(value: info.epoch),
                    "isSession": info.isSession,
                    "createdAtMs": NSNumber(value: info.createdAtMs),
                    "lastActivityMs": NSNumber(value: info.lastActivityMs)
                ]
                resolver(result)
            } else {
                resolver(NSNull())
            }
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to get group info")
        }
    }

    /// Whether a rich group send right now would seal its extras, and which
    /// members hold the gate closed (point-in-time, advisory).
    @objc func meshGroupRichReadiness(_ groupId: String,
                                      resolver: @escaping RCTPromiseResolveBlock,
                                      rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            let readiness = try proto.groupRichReadiness(groupId: groupId)
            let result: [String: Any] = [
                "ready": readiness.ready,
                "unknownMembers": readiness.unknownMembers
            ]
            resolver(result)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to get group rich readiness")
        }
    }

    /// Relay-side registration state of a group ('synced' | 'pending' |
    /// 'unsynced'). Point-in-time; transitions surface as
    /// `group_relay_sync_changed` events.
    @objc func meshGroupRelaySyncState(_ groupId: String,
                                       resolver: @escaping RCTPromiseResolveBlock,
                                       rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            let state = try proto.groupRelaySyncState(groupId: groupId)
            switch state {
            case .synced: resolver("synced")
            case .pending: resolver("pending")
            case .unsynced: resolver("unsynced")
            }
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to get relay sync state")
        }
    }

    /// Registers (or re-registers) a group with the relay server on demand —
    /// the supported path before relay-dependent raw server commands
    /// (invite links). Outcome arrives as `group_relay_sync_changed`;
    /// resolves true when the frame was queued or the group is already
    /// synced, false when relay grouping is disabled or Internet is down.
    @objc func meshRequestGroupRelayRegistration(_ groupId: String,
                                                 resolver: @escaping RCTPromiseResolveBlock,
                                                 rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            resolver(try proto.requestGroupRelayRegistration(groupId: groupId))
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to request relay registration")
        }
    }

    /// Set a member's role in a group (admin only).
    @objc func meshSetMemberRole(_ groupId: String,
                                  userId: String,
                                  role: String,
                                  resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.setMemberRole(groupId: groupId, userId: userId, role: role)
            resolver(nil)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to set member role")
        }
    }

    /// Get a member's role in a group.
    @objc func meshGetMemberRole(_ groupId: String,
                                  userId: String,
                                  resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            let role = try proto.getMemberRole(groupId: groupId, userId: userId)
            resolver(role)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to get member role")
        }
    }

    /// Get all member roles in a group.
    @objc func meshGetGroupRoles(_ groupId: String,
                                  resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            let roles = try proto.getGroupRoles(groupId: groupId)
            resolver(roles)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to get group roles")
        }
    }

    /// Rename a group (admin only, broadcasts to all members).
    @objc func meshRenameGroup(_ groupId: String,
                                newName: String,
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_MESH_GROUP", "Protocol not initialized", nil)
            return
        }
        do {
            try proto.renameGroup(groupId: groupId, newName: newName)
            resolver(nil)
        } catch {
            rejectWithProtocolError(error, rejecter, fallbackCode: "ERROR_MESH_GROUP", fallbackMessage: "Failed to rename group")
        }
    }

    /// One-shot relay presence query for a peer (last-seen display, pre-send
    /// checks). Fire-and-event: resolves true if the query was written to
    /// the socket; the answer arrives as the SDK's `presence_updated` event.
    /// `options.force` parks the query through the chat-open reconnect
    /// window instead of failing fast (see InternetManager.checkPresence).
    @objc func checkInternetPresence(_ userId: String,
                                     options: NSDictionary,
                                     resolver: @escaping RCTPromiseResolveBlock,
                                     rejecter: @escaping RCTPromiseRejectBlock) {
        guard let manager = internetManager else {
            // Same message shape as the Android module's rejectWithProtocolError.
            rejecter("ERROR_INTERNET", "Failed to query presence: Internet transport not initialized", nil)
            return
        }
        let force = (options["force"] as? NSNumber)?.boolValue ?? false
        manager.checkPresence(userId: userId, force: force) { written in
            resolver(written)
        }
    }

    /// Sends a raw, caller-built relay command verbatim over the SDK's
    /// socket (invite-link lifecycle and other server-plane ops). Resolves
    /// true when written; false when invalid JSON or not connected and
    /// authenticated. Responses arrive as `internet_server_message` events.
    @objc func internetSendRawCommand(_ json: String,
                                      resolver: @escaping RCTPromiseResolveBlock,
                                      rejecter: @escaping RCTPromiseRejectBlock) {
        guard let manager = internetManager else {
            // Same message shape as the Android module's rejectWithProtocolError.
            rejecter("ERROR_INTERNET", "Failed to send raw server command: Internet transport not initialized", nil)
            return
        }
        manager.sendRawCommand(json: json) { written in
            resolver(written)
        }
    }

    /// Whether the internet socket is connected AND relay-authenticated —
    /// the same gate `internetSendRawCommand` checks. Point-in-time;
    /// transitions arrive as `internet_status_changed` events. Resolves
    /// false (never rejects) when the internet transport isn't initialized.
    @objc func internetIsReady(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        resolver(internetManager?.isReady() ?? false)
    }

    /// Forces an immediate teardown + reconnect + re-authenticate of the
    /// internet socket, bypassing the exponential backoff — the deterministic
    /// recovery for a foreground-after-background where the cached ready flags
    /// may be stale (see InternetManager.forceReconnect). Resolves true when
    /// the request reached a live internet transport ("accepted", not
    /// "reconnected" — also true when the transport is initialized but not
    /// running, where forceReconnect is a deliberate no-op); false (never
    /// rejects) only when the internet transport isn't initialized.
    @objc func internetForceReconnect(_ resolver: @escaping RCTPromiseResolveBlock,
                                      rejecter: @escaping RCTPromiseRejectBlock) {
        guard let manager = internetManager else {
            resolver(false)
            return
        }
        manager.forceReconnect()
        resolver(true)
    }

    // MARK: - Helpers
    
    private func parseInternetConfig(_ config: NSDictionary?) throws -> (String, UInt16) {
        let serverAddress = ((config?["serverAddress"] as? String) ?? (config?["server_url"] as? String))?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let address = serverAddress, !address.isEmpty else {
            throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Internet transport requires a serverAddress"])
        }

        var portNumber: Int? = nil
        if let value = config?["port"] as? NSNumber {
            portNumber = value.intValue
        } else if let value = config?["serverPort"] as? NSNumber {
            portNumber = value.intValue
        }

        var host = address
        if let url = URL(string: address), let scheme = url.scheme, let urlHost = url.host {
            host = urlHost
            if let urlPort = url.port {
                portNumber = urlPort
            } else if portNumber == nil {
                switch scheme.lowercased() {
                case "wss", "https": portNumber = 443
                case "ws", "http": portNumber = 80
                default: break
                }
            }
        }

        if portNumber == nil {
            portNumber = 443
        }

        guard let finalPort = portNumber, finalPort >= 0 && finalPort <= 65535 else {
            throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Invalid port value provided"])
        }

        return (host, UInt16(finalPort))
    }

    private func transportType(from type: String) throws -> TransportType {
        switch type.lowercased() {
        case "internet":
            return .internet
        case "ble":
            return .ble
        case "wifidirect", "wifi_direct":
            return .wiFiDirect
        case "reticulum":
            return .reticulum
        default:
            throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Unsupported transport type: \(type)"])
        }
    }

    private func buildTopologyJson(_ topology: NetworkTopology) throws -> String {
        var connectionCounts: [String: Int] = [:]
        var transportsByNode: [String: Set<String>] = [:]

        let linksArray: [[String: Any]] = topology.links.map { link in
            let transportName = normalizeTransportName(link.transport)

            connectionCounts[link.sourceId, default: 0] += 1
            connectionCounts[link.targetId, default: 0] += 1

            transportsByNode[link.sourceId, default: Set<String>()].insert(transportName)
            transportsByNode[link.targetId, default: Set<String>()].insert(transportName)

            return [
                "from": link.sourceId,
                "to": link.targetId,
                "quality": Double(link.quality),
                "transport": transportName,
                "rssi": NSNull()
            ]
        }

        let nodesArray: [[String: Any]] = topology.nodes.map { node in
            let transports = Array(transportsByNode[node.nodeId] ?? [])
            return [
                "user_id": node.nodeId,
                "role": node.role.lowercased(),
                "connection_count": connectionCounts[node.nodeId] ?? 0,
                "battery_level": NSNull(),
                "last_seen": Int(node.lastSeenMs / 1000),
                "transports": transports
            ]
        }

        let averageQuality: Double = {
            guard !linksArray.isEmpty else { return 0.0 }
            let total = linksArray.reduce(0.0) { partialResult, entry in
                partialResult + (entry["quality"] as? Double ?? 0.0)
            }
            return total / Double(linksArray.count)
        }()

        let stats: [String: Any] = [
            "total_nodes": topology.nodes.count,
            "relay_nodes": topology.nodes.filter { $0.role.lowercased() == "relay" }.count,
            "total_connections": topology.links.count,
            "avg_link_quality": averageQuality,
            "network_diameter": NSNull()
        ]

        let payload: [String: Any] = [
            "timestamp": Int(Date().timeIntervalSince1970),
            "local_user_id": currentConfig?.userId ?? "",
            "nodes": nodesArray,
            "links": linksArray,
            "stats": stats
        ]

        let data = try JSONSerialization.data(withJSONObject: payload, options: [])
        guard let json = String(data: data, encoding: .utf8) else {
            throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Failed to encode topology JSON"])
        }
        return json
    }

    private func buildMessageStatsJson(_ stats: [MessageStats]) throws -> String {
        let array: [[String: Any]] = stats.map { stat in
            var entry: [String: Any] = [
                "message_id": stat.messageId,
                "sender": NSNull(),
                "recipient": NSNull(),
                "sent_at": Int(stat.sentAtMs),
                "hop_count": Int(stat.hopCount),
                "transport": NSNull(),
                "retry_count": 0,
                "status": stat.status
            ]

            if let delivered = stat.deliveredAtMs {
                entry["delivered_at"] = Int(delivered)
                entry["latency_ms"] = max(Int(delivered) - Int(stat.sentAtMs), 0)
            } else {
                entry["delivered_at"] = NSNull()
                entry["latency_ms"] = NSNull()
            }

            return entry
        }

        let data = try JSONSerialization.data(withJSONObject: array, options: [])
        guard let json = String(data: data, encoding: .utf8) else {
            throw NSError(domain: "OfflineProtocol", code: -1, userInfo: [NSLocalizedDescriptionKey: "Failed to encode message stats JSON"])
        }
        return json
    }
    
    private func normalizeTransportName(_ name: String) -> String {
        let lower = name.lowercased()
        switch lower {
        case "ble":
            return "ble"
        case "internet":
            return "internet"
        case "wifi_direct", "wifidirect", "wi_fi_direct":
            return "wifiDirect"
        default:
            return lower
        }
    }
    
    // MARK: - Process Timer
    private static let maxMessagesPerProcessTick = 100
    
    private func startProcessTimer() {
        stopProcessTimer()
        
        let timer = DispatchSource.makeTimerSource(queue: processQueue)
        timer.schedule(deadline: .now(), repeating: .milliseconds(100))
        timer.setEventHandler { [weak self] in
            self?.processProtocol()
        }
        timer.resume()
        
        processTimer = timer
    }
    
    private func stopProcessTimer() {
        processTimer?.cancel()
        processTimer = nil
    }

    /// Waits for a process tick that is already running to finish.
    ///
    /// `DispatchSourceTimer.cancel()` only stops *future* firings; a handler
    /// mid-flight keeps going, and it holds a strong reference to the protocol
    /// instance, so it can still persist (outbox entries, retry lifecycles)
    /// after `destroy` has returned. A wipe that follows `destroy` would then
    /// race a straggler and leave a repopulated container.
    ///
    /// `processQueue` is serial, so a block enqueued here cannot run until the
    /// in-flight handler has returned. Bounded, so a wedged tick cannot hang the
    /// bridge — the tick itself is short, and the timeout is far longer than one
    /// takes. Safe from any other queue: `destroy` runs on the bridge queue,
    /// never on `processQueue`.
    private func drainProcessQueue() {
        let drained = DispatchSemaphore(value: 0)
        processQueue.async { drained.signal() }
        _ = drained.wait(timeout: .now() + 2.0)
    }

    private func processProtocol() {
        guard let instance = protocolInstance else { return }
        do {
            try instance.process()
            drainIncomingMessages(instance)
        } catch {
            print("Process error: \(error)")
        }
    }

    private func drainIncomingMessages(_ instance: OfflineProtocol) {
        var drained = 0
        while drained < Self.maxMessagesPerProcessTick {
            guard instance.receiveMessage() != nil else { break }
            drained += 1
        }
        if drained == Self.maxMessagesPerProcessTick {
            emitDiagnostic(
                level: "warning",
                message: "Capped receiveMessage drain for this process tick",
                context: ["maxBatch": Self.maxMessagesPerProcessTick]
            )
        }
    }
}

// MARK: - TransportManagerDelegate

extension OfflineProtocolModule: TransportManagerDelegate {
    func transportManager(_ manager: TransportManager, didChangeState state: TransportState) {
        let transportName = manager.transportName
        emitDiagnostic(level: "info", message: "\(transportName) state changed", context: [
            "transport": manager.transportId,
            "state": String(describing: state)
        ])
    }
    
    func transportManager(_ manager: TransportManager, didEncounterError error: Error) {
        let transportName = manager.transportName
        emitDiagnostic(level: "error", message: "\(transportName) error", context: [
            "transport": manager.transportId,
            "error": error.localizedDescription
        ])
    }
    
    func transportManager(_ manager: TransportManager, didUpdateMetrics metrics: [String : Any]) {
        var context = metrics
        context["transport"] = manager.transportId
        emitDiagnostic(level: "info", message: "\(manager.transportName) metrics", context: context)
    }
    
    func transportManager(_ manager: TransportManager, didEmitDiagnostic level: String, message: String, context: [String : Any]) {
        var enrichedContext = context
        enrichedContext["transport"] = manager.transportId
        emitDiagnostic(level: level, message: message, context: enrichedContext)
    }
}

// MARK: - EventCallback Implementation

class EventCallbackImpl: EventCallback, @unchecked Sendable {
    weak var emitter: OfflineProtocolModule?
    
    init(emitter: OfflineProtocolModule) {
        self.emitter = emitter
    }
    
    func onEvent(eventJson: String) {
        guard let emitter = emitter else { return }
        let body: [String: Any] = ["eventJson": eventJson]
        if Thread.isMainThread {
            emitter.sendEventToJS(OfflineProtocolModule.Events.onEvent, body: body)
        } else {
            DispatchQueue.main.async {
                emitter.sendEventToJS(OfflineProtocolModule.Events.onEvent, body: body)
            }
        }
    }
}

// MARK: - BLE Transport Callback (Event-Driven Sending)

class BleTransportCallbackImpl: BleTransportCallback, @unchecked Sendable {
    weak var bleManager: BleManager?
    
    init(bleManager: BleManager) {
        self.bleManager = bleManager
    }
    
    func onFragmentsAvailable() {
        bleManager?.onFragmentsAvailable()
    }
}

// MARK: - WiFi Direct Transport Callback (Event-Driven Sending)

class WifiDirectTransportCallbackImpl: WifiDirectTransportCallback, @unchecked Sendable {
    weak var wifiDirectManager: WifiDirectManager?

    init(wifiDirectManager: WifiDirectManager) {
        self.wifiDirectManager = wifiDirectManager
    }

    func onMessagesAvailable() {
        wifiDirectManager?.onMessagesAvailable()
    }
}

// MARK: - Reticulum Transport Callback (Event-Driven Sending)

class ReticulumTransportCallbackImpl: ReticulumTransportCallback, @unchecked Sendable {
    weak var reticulumManager: ReticulumManager?

    init(reticulumManager: ReticulumManager) {
        self.reticulumManager = reticulumManager
    }

    func onMessagesAvailable() {
        reticulumManager?.onMessagesAvailable()
    }
}

// MARK: - Nostr Transport Callback (Event-Driven Sending)

class NostrTransportCallbackImpl: NostrTransportCallback, @unchecked Sendable {
    weak var nostrManager: NostrManager?

    init(nostrManager: NostrManager) {
        self.nostrManager = nostrManager
    }

    func onMessagesAvailable() {
        nostrManager?.onMessagesAvailable()
    }
}

// Make Events accessible
extension OfflineProtocolModule {
    fileprivate struct Events {
        static let onEvent = "OfflineProtocol_Event"
        static let onTelemetry = "OfflineProtocol_Telemetry"
    }
}

// MARK: - TelemetrySink Implementation

class TelemetrySinkImpl: TelemetrySink, @unchecked Sendable {
    weak var emitter: OfflineProtocolModule?

    init(emitter: OfflineProtocolModule) {
        self.emitter = emitter
    }

    // Every callback is invoked synchronously from the Rust emit path; the
    // encoders are pure Swift dict construction and cannot throw, so the
    // only failure mode here is `emitter` having been deallocated.
    private func dispatch(_ body: [String: Any]) {
        guard let emitter = emitter else { return }
        let send: () -> Void = {
            emitter.sendEventToJS(OfflineProtocolModule.Events.onTelemetry, body: body)
        }
        if Thread.isMainThread {
            send()
        } else {
            DispatchQueue.main.async(execute: send)
        }
    }

    func onProtocolEvent(eventJson: String) {
        dispatch(["category": "protocol", "eventJson": eventJson])
    }

    func onMlsEvent(eventJson: String) {
        dispatch(["category": "mls", "eventJson": eventJson])
    }

    func onMetricsFrame(frame: MetricsFrame) {
        dispatch(["category": "metricsFrame", "frame": TelemetrySinkImpl.encode(frame: frame)])
    }

    func onTransportState(event: TransportStateEvent) {
        dispatch(["category": "transportState", "event": TelemetrySinkImpl.encode(event: event)])
    }

    func onRoutingDecision(decision: RoutingDecision) {
        dispatch(["category": "routingDecision", "decision": TelemetrySinkImpl.encode(decision: decision)])
    }

    func onDeviceCapability(snapshot: DeviceCapabilitySnapshot) {
        dispatch(["category": "deviceCapability", "snapshot": TelemetrySinkImpl.encode(snapshot: snapshot)])
    }

    func onExtension(name: String, payloadJson: String) {
        dispatch(["category": "extension", "name": name, "payloadJson": payloadJson])
    }

    // MARK: Encoders (UniFFI structs → JSON-safe dictionaries)
    //
    // IMPORTANT: every dict produced below MUST be structurally identical
    // to the JSON envelope the Rust adapter enqueues on the pull channel.
    // The canonical contract is pinned by the `shape_parity_*_envelope`
    // tests in `crates/offline-protocol-uniffi/src/lib.rs`. If those tests
    // change, update the matching encoder here in lockstep — the TS
    // `TelemetryRecord` discriminated union expects ONE shape regardless
    // of whether a record arrived via `onTelemetry` (push) or
    // `pollTelemetry` (pull).

    fileprivate static func encode(metrics m: TransportMetrics) -> [String: Any] {
        var d: [String: Any] = [
            "packetsSent": m.packetsSent,
            "packetsReceived": m.packetsReceived,
            "bytesSent": m.bytesSent,
            "bytesReceived": m.bytesReceived,
            "errorRate": m.errorRate,
            "avgLatencyMs": m.avgLatencyMs,
        ]
        if let v = m.rssi { d["rssi"] = v }
        if let v = m.bandwidthBps { d["bandwidthBps"] = v }
        if let v = m.congestion { d["congestion"] = v }
        if let v = m.queueDepth { d["queueDepth"] = v }
        if let v = m.batteryLevel { d["batteryLevel"] = v }
        if let v = m.isCharging { d["isCharging"] = v }
        if let v = m.relayConnectionCount { d["relayConnectionCount"] = v }
        if let v = m.isActiveRelay { d["isActiveRelay"] = v }
        if let v = m.deliveryRatio { d["deliveryRatio"] = v }
        if let v = m.dropRate { d["dropRate"] = v }
        if let v = m.averageHopCount { d["averageHopCount"] = v }
        if let v = m.energyCost { d["energyCost"] = v }
        return d
    }

    fileprivate static func encode(transportType t: TransportType) -> String {
        switch t {
        case .internet: return "internet"
        case .ble: return "ble"
        case .wiFiDirect: return "wifiDirect"
        case .reticulum: return "reticulum"
        case .nostr: return "nostr"
        }
    }

    fileprivate static func encode(status s: TransportStatus) -> String {
        switch s {
        case .available: return "available"
        case .unavailable: return "unavailable"
        case .connecting: return "connecting"
        case .disconnected: return "disconnected"
        case .error: return "error"
        }
    }

    fileprivate static func encode(frame f: MetricsFrame) -> [String: Any] {
        var d: [String: Any] = [
            "timestampMs": f.timestampMs,
            "transports": f.transports.map { entry -> [String: Any] in
                [
                    "transport": encode(transportType: entry.transport),
                    "metrics": encode(metrics: entry.metrics),
                ]
            },
            "retryQueue": [
                "totalCount": f.retryQueue.totalCount,
                "readyCount": f.retryQueue.readyCount,
                "criticalPriorityCount": f.retryQueue.criticalPriorityCount,
                "highPriorityCount": f.retryQueue.highPriorityCount,
                "mediumPriorityCount": f.retryQueue.mediumPriorityCount,
                "lowPriorityCount": f.retryQueue.lowPriorityCount,
            ],
            "dedup": [
                "totalTracked": f.dedup.totalTracked,
                "recentTracked": f.dedup.recentTracked,
                "capacityUsedPercent": f.dedup.capacityUsedPercent,
                "mode": f.dedup.mode,
            ],
            "ackPending": f.ackPending,
            "neighborCount": f.neighborCount,
            "isLocalRelay": f.isLocalRelay,
        ]
        if let fpr = f.dedup.falsePositiveRate,
           var dedup = d["dedup"] as? [String: Any] {
            dedup["falsePositiveRate"] = fpr
            d["dedup"] = dedup
        }
        if let t = f.currentTransport { d["currentTransport"] = encode(transportType: t) }
        return d
    }

    fileprivate static func encode(event e: TransportStateEvent) -> [String: Any] {
        return [
            "timestampMs": e.timestampMs,
            "transport": encode(transportType: e.transport),
            "previous": encode(status: e.previous),
            "current": encode(status: e.current),
        ]
    }

    fileprivate static func encode(decision d: RoutingDecision) -> [String: Any] {
        var out: [String: Any] = [
            "timestampMs": d.timestampMs,
            "phase": encode(phase: d.phase),
            "scores": d.scores.map { s -> [String: Any] in
                [
                    "transport": encode(transportType: s.transport),
                    "signal": s.signal, "proximity": s.proximity,
                    "bandwidth": s.bandwidth, "congestion": s.congestion,
                    "energy": s.energy, "reliability": s.reliability,
                    "load": s.load, "total": s.total,
                ]
            },
        ]
        if let v = d.from { out["from"] = encode(transportType: v) }
        if let v = d.to { out["to"] = encode(transportType: v) }
        if let v = d.winningScore { out["winningScore"] = v }
        if let v = d.reasonCode { out["reasonCode"] = encode(reason: v) }
        return out
    }

    fileprivate static func encode(phase p: RoutingPhase) -> String {
        switch p {
        case .scoreUpdated: return "scoreUpdated"
        case .selected: return "selected"
        case .switched: return "switched"
        case .escalated: return "escalated"
        case .unknown: return "unknown"
        }
    }

    fileprivate static func encode(reason r: RoutingReasonCode) -> String {
        switch r {
        case .initialSelection: return "initialSelection"
        case .primarySelected: return "primarySelected"
        case .primarySuccess: return "primarySuccess"
        case .fallbackSuccess: return "fallbackSuccess"
        case .escalationApplied: return "escalationApplied"
        case .currentUnavailable: return "currentUnavailable"
        case .retryThreshold: return "retryThreshold"
        case .poorSignal: return "poorSignal"
        case .congestion: return "congestion"
        case .lowTtl: return "lowTtl"
        case .lowSuccessRate: return "lowSuccessRate"
        case .unknown: return "unknown"
        }
    }

    fileprivate static func encode(snapshot s: DeviceCapabilitySnapshot) -> [String: Any] {
        var d: [String: Any] = [
            "timestampMs": s.timestampMs,
            "isCharging": s.isCharging,
            "relayRole": s.relayRole == .relay ? "relay" : "regular",
            "changedFields": s.changedFields,
        ]
        if let v = s.batteryLevel { d["batteryLevel"] = v }
        return d
    }
}

// MARK: - TelemetryConfig parsing

extension OfflineProtocolModule {
    // The TS `TelemetryConfig` type (bindings/react-native/src/types.ts)
    // only emits camelCase keys — this parser matches that contract.
    //
    // On unrecognised `mlsVerbosity` strings we log a warning and fall back
    // to `nil` (Rust default). Silent fallback would have hid integrator
    // typos behind "it just applies Lifecycle", which is indistinguishable
    // from "my config took effect".
    fileprivate func parseTelemetryConfig(_ dict: [String: Any]?) -> TelemetryConfig {
        guard let dict = dict else {
            return TelemetryConfig(
                scrubIds: nil, mlsVerbosity: nil,
                metricsCadenceMs: nil, routingDiagnostic: nil,
                enablePollQueue: nil, mlsSamplingBypass: nil
            )
        }
        let verbosity: MlsVerbosity?
        if let raw = dict["mlsVerbosity"] as? String {
            switch raw.lowercased() {
            case "off": verbosity = .off
            case "diagnostic": verbosity = .diagnostic
            case "lifecycle": verbosity = .lifecycle
            default:
                print("[OfflineProtocolModule] telemetry: unknown mlsVerbosity '\(raw)' — expected 'off', 'lifecycle', or 'diagnostic'. Falling back to the Rust default (lifecycle).")
                verbosity = nil
            }
        } else {
            verbosity = nil
        }
        let scrubIds = dict["scrubIds"] as? Bool
        // `metricsCadenceMs` is config-sized (cadence in ms fits comfortably
        // in an f64's 53-bit mantissa). Don't reuse this cast for counter
        // fields that can exceed 2^53.
        let cadence = (dict["metricsCadenceMs"] as? NSNumber)?.uint64Value
        let routingDiag = dict["routingDiagnostic"] as? Bool
        let enablePollQueue = dict["enablePollQueue"] as? Bool
        let mlsSamplingBypass = dict["mlsSamplingBypass"] as? Bool
        return TelemetryConfig(
            scrubIds: scrubIds,
            mlsVerbosity: verbosity,
            metricsCadenceMs: cadence,
            routingDiagnostic: routingDiag,
            enablePollQueue: enablePollQueue,
            mlsSamplingBypass: mlsSamplingBypass
        )
    }
}
