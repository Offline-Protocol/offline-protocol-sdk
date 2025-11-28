//
//  OfflineProtocolModule.swift
//  OfflineProtocol
//
//  UniFFI-based implementation
//

import Foundation
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
    private var bleManager: BleManager?
    private var internetManager: InternetManager?
    private var hasListeners = false
    private let processQueue = DispatchQueue(label: "offlineprotocol.processor")
    private var processTimer: DispatchSourceTimer?
    private var currentConfig: ProtocolConfig?
    
    override init() {
        print("[OfflineProtocolModule] init() called")
        super.init()
        print("[OfflineProtocolModule] init() completed successfully")
    }
    
    deinit {
        stopProcessTimer()
        bleManager?.stop()
        bleManager = nil
        internetManager?.stop()
        internetManager = nil
        protocolInstance = nil
    }
    
    override class func requiresMainQueueSetup() -> Bool {
        return true
    }
    
    override func supportedEvents() -> [String]! {
        return [Events.onEvent]
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
        
        let config = ProtocolConfig(
            appId: raw["appId"] as? String ?? raw["app_id"] as? String ?? "",
            userId: raw["userId"] as? String ?? raw["user_id"] as? String ?? "",
            bleEnabled: raw["bleEnabled"] as? Bool ?? raw["ble_enabled"] as? Bool ?? true,
            wifiDirectEnabled: raw["wifiDirectEnabled"] as? Bool ?? raw["wifi_direct_enabled"] as? Bool ?? true,
            internetEnabled: raw["internetEnabled"] as? Bool ?? raw["internet_enabled"] as? Bool ?? true,
            preferOnline: raw["preferOnline"] as? Bool ?? raw["prefer_online"] as? Bool ?? false,
            initialTtl: UInt8(raw["initialTtl"] as? Int ?? raw["initial_ttl"] as? Int ?? 8)
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
                "internetEnabled": config.internetEnabled
            ])
            
            // Set up event callback
            proto.setEventCallback(callback: EventCallbackImpl(emitter: self))

            applyInitialRuntimeConfig(proto, rawConfig: parsed.raw)

            protocolInstance = proto
            
            // Initialize BLE manager if BLE is enabled
            if config.bleEnabled {
                bleManager = BleManager(protocol: proto, deviceId: config.userId)
                bleManager?.delegate = self
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
                print("[OfflineProtocolModule] Internet Manager initialized for user: \(config.userId)")
                emitDiagnostic(level: "info", message: "Internet manager initialized", context: [
                    "userId": config.userId
                ])
            } else {
                emitDiagnostic(level: "info", message: "Internet disabled in configuration", context: [
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
            
            // Start BLE manager if available
            if let manager = bleManager {
                do {
                    try manager.start()
                    print("[OfflineProtocolModule] BLE Manager started")
                    emitDiagnostic(level: "info", message: "BLE manager started")
                    
                    // CRITICAL FIX: Ensure bleStatusChanged(true) is called even if timing is off
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { [weak self] in
                        print("[OfflineProtocolModule] Backup bleStatusChanged(true) call")
                        self?.emitDiagnostic(level: "info", message: "Backup call to protocol.bleStatusChanged(true)")
                        try? self?.protocolInstance?.bleStatusChanged(isAvailable: true)
                        self?.emitDiagnostic(level: "info", message: "Backup bleStatusChanged(true) completed")
                    }
                } catch {
                    print("[OfflineProtocolModule] Warning: Failed to start BLE Manager: \(error.localizedDescription)")
                    emitDiagnostic(level: "error", message: "Failed to start BLE manager", context: [
                        "error": error.localizedDescription
                    ])
                    // Don't fail the entire start if BLE fails
                }
            }
            
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
            // Pause BLE manager for background mode
            bleManager?.pause()
            
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
            
            // Resume BLE manager
            bleManager?.resume()
            
            resolver(nil)
        } catch {
            rejecter("ERROR_RESUME", "Failed to resume protocol: \(error.localizedDescription)", error)
        }
    }
    
    @objc func sendMessage(_ recipient: String,
                          content: String,
                          priority: Int,
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
            
            let messageId = try proto.sendMessage(recipient: recipient, content: content, priority: msgPriority)
            resolver(messageId)
        } catch {
            rejecter("ERROR_SEND", "Failed to send message: \(error.localizedDescription)", error)
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
        bleManager?.stop()
        bleManager = nil
        do {
            try protocolInstance?.stop()
        } catch {
            // Ignore stop failures during destroy
        }
        protocolInstance = nil
        currentConfig = nil
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
                guard let manager = internetManager else {
                    // Create manager if not already created
                    let newManager = InternetManager(protocol: proto, deviceId: currentConfig?.userId ?? "unknown")
                    newManager.delegate = self
                    internetManager = newManager
                    emitDiagnostic(level: "info", message: "Internet manager created on demand")
                    try configureAndStartInternet(manager: newManager, config: config)
                    break
                }
                try configureAndStartInternet(manager: manager, config: config)
            case "wifidirect", "wifi_direct":
                try proto.addWifiDirectTransport()
            case "ble":
                break // BLE managed automatically
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
        
        // Internet transport is already registered during protocol initialization
        // Just configure and start the WebSocket manager
        try manager.configure(serverUrl: wsUrl, autoReconnect: autoReconnect, maxReconnectAttempts: maxRetries)
        try manager.start()
        
        emitDiagnostic(level: "info", message: "Internet transport enabled", context: [
            "serverUrl": wsUrl,
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
            let transport = try transportType(from: type)
            
            // Stop corresponding transport manager
            if type.lowercased() == "internet" {
                internetManager?.stop()
                emitDiagnostic(level: "info", message: "Internet manager stopped via disableTransport")
            }
            
            try proto.removeTransport(transportType: transport)
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
    
    @objc func sendFile(_ filePath: String,
                        recipient: String,
                        fileName: String,
                        resolver: @escaping RCTPromiseResolveBlock,
                        rejecter: @escaping RCTPromiseRejectBlock) {
        guard let proto = protocolInstance else {
            rejecter("ERROR_SEND_FILE", "Protocol not initialized", nil)
            return
        }
        do {
            let fileId = try proto.sendFile(recipient: recipient, filePath: filePath, fileName: fileName)
            resolver(fileId)
        } catch {
            rejecter("ERROR_SEND_FILE", "Failed to send file: \(error.localizedDescription)", error)
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
                "file_name": progress.fileId,
                "file_size": 0,
                "chunks_completed": Int(progress.chunksSent),
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
            rejecter("ERROR_FILE_CANCEL", "Failed to cancel file transfer: \(error.localizedDescription)", error)
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
        case .starting:
            stateString = "Starting"
        case .running:
            stateString = "Running"
        case .paused:
            stateString = "Paused"
        case .stopping:
            stateString = "Stopping"
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
            
            let dorsConfig = DorsConfig(
                preferOnline: config["preferOnline"] as? Bool ?? false,
                switchHysteresis: max((config["switchHysteresis"] as? NSNumber)?.floatValue ?? 15.0, 0),
                switchCooldownSecs: max((config["switchCooldownSecs"] as? NSNumber)?.uint64Value ?? 20, 0),
                bleToWifiRetryThreshold: (config["bleToWifiRetryThreshold"] as? NSNumber)?.uint32Value ?? 2,
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
    
    private func processProtocol() {
        guard let instance = protocolInstance else { return }
        do {
            try instance.process()
            while instance.receiveMessage() != nil {}
        } catch {
            print("Process error: \(error)")
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
        emitter?.sendEventToJS(OfflineProtocolModule.Events.onEvent, body: ["eventJson": eventJson])
    }
}

// Make Events accessible
extension OfflineProtocolModule {
    fileprivate struct Events {
        static let onEvent = "OfflineProtocol_Event"
    }
}

