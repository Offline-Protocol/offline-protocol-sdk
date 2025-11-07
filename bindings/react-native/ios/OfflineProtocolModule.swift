//
//  OfflineProtocolModule.swift
//  OfflineProtocol
//
//  UniFFI-based implementation
//

import Foundation
import React

// Import generated UniFFI bindings
// The generated file is in ios/Generated/offline_protocol.swift
// We need to ensure it's included in the Xcode project

@objc(OfflineProtocolModule)
class OfflineProtocolModule: RCTEventEmitter {
    private var protocolInstance: OfflineProtocol?
    private var bleManager: BleManager?
    private var hasListeners = false
    private let processQueue = DispatchQueue(label: "offlineprotocol.processor")
    private var processTimer: DispatchSourceTimer?
    
    override init() {
        super.init()
    }
    
    deinit {
        stopProcessTimer()
        bleManager?.stop()
        bleManager = nil
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
    
    private func parseConfig(_ configJson: String) throws -> ProtocolConfig {
        guard let jsonData = configJson.data(using: .utf8),
              let config = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
            throw NSError(domain: "OfflineProtocol", code: -1, 
                         userInfo: [NSLocalizedDescriptionKey: "Invalid JSON"])
        }
        
        return ProtocolConfig(
            appId: config["appId"] as? String ?? config["app_id"] as? String ?? "",
            userId: config["userId"] as? String ?? config["user_id"] as? String ?? "",
            bleEnabled: config["bleEnabled"] as? Bool ?? config["ble_enabled"] as? Bool ?? true,
            wifiDirectEnabled: config["wifiDirectEnabled"] as? Bool ?? config["wifi_direct_enabled"] as? Bool ?? true,
            internetEnabled: config["internetEnabled"] as? Bool ?? config["internet_enabled"] as? Bool ?? true,
            preferOnline: config["preferOnline"] as? Bool ?? config["prefer_online"] as? Bool ?? false,
            initialTtl: UInt8(config["initialTtl"] as? Int ?? config["initial_ttl"] as? Int ?? 8)
        )
    }
    
    // MARK: - Exported Methods
    
    @objc func create(_ configJson: String,
                     resolver: @escaping RCTPromiseResolveBlock,
                     rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            let config = try parseConfig(configJson)
            let proto = try OfflineProtocol(config: config)
            
            // Set up event callback
            proto.setEventCallback(callback: EventCallbackImpl(emitter: self))
            
            protocolInstance = proto
            
            // Initialize BLE manager if BLE is enabled
            if config.bleEnabled {
                bleManager = BleManager(protocol: proto, deviceId: config.userId)
                print("[OfflineProtocolModule] BLE Manager initialized for user: \(config.userId)")
            }
            
            // Start process timer
            startProcessTimer()
            
            resolver(nil)
        } catch {
            rejecter("ERROR_CREATE", "Failed to create protocol: \(error.localizedDescription)", error)
        }
    }
    
    // MARK: - Event Handling
    
    fileprivate func sendEventToJS(_ eventName: String, body: Any?) {
        if hasListeners {
            sendEvent(withName: eventName, body: body)
        }
    }
    
    @objc func start(_ resolver: @escaping RCTPromiseResolveBlock,
                   rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            try protocolInstance?.start()
            
            // Start BLE manager if available
            if let manager = bleManager {
                do {
                    try manager.start()
                    print("[OfflineProtocolModule] BLE Manager started")
                } catch {
                    print("[OfflineProtocolModule] Warning: Failed to start BLE Manager: \(error.localizedDescription)")
                    // Don't fail the entire start if BLE fails
                }
            }
            
            resolver(nil)
        } catch {
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
        
        do {
            try protocolInstance?.stop()
            resolver(nil)
        } catch {
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
    
    // MARK: - BLE Transport Methods
    
    @objc func blePeerDiscovered(_ peerId: String,
                                 rssi: Int,
                                 resolver: @escaping RCTPromiseResolveBlock,
                                 rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            try protocolInstance?.blePeerDiscovered(peerId: peerId, rssi: Int16(rssi))
            resolver(nil)
        } catch {
            rejecter("ERROR_BLE", "BLE peer discovered failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func blePeerLost(_ peerId: String,
                          resolver: @escaping RCTPromiseResolveBlock,
                          rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            try protocolInstance?.blePeerLost(peerId: peerId)
            resolver(nil)
        } catch {
            rejecter("ERROR_BLE", "BLE peer lost failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func bleStatusChanged(_ isAvailable: Bool,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            try protocolInstance?.bleStatusChanged(isAvailable: isAvailable)
            resolver(nil)
        } catch {
            rejecter("ERROR_BLE", "BLE status changed failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func bleFragmentReceived(_ senderId: String,
                                   fragmentData: [NSNumber],
                                   resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            let fragment = fragmentData.map { UInt8($0.intValue) }
            try protocolInstance?.bleFragmentReceived(senderId: senderId, fragment: fragment)
            resolver(nil)
        } catch {
            rejecter("ERROR_BLE", "BLE fragment received failed: \(error.localizedDescription)", error)
        }
    }
    
    @objc func bleGetNextFragment(_ resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        if let fragment = protocolInstance?.bleGetNextFragment() {
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
        protocolInstance?.bleReturnFragment()
        resolver(nil)
    }
    
    @objc func bleGetPeerCount(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        let count = protocolInstance?.bleGetPeerCount() ?? 0
        resolver(NSNumber(value: count))
    }
    
    @objc func getActiveTransports(_ resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
        let transports = protocolInstance?.getActiveTransports() ?? []
        resolver(transports)
    }
    
    @objc func getState(_ resolver: @escaping RCTPromiseResolveBlock,
                       rejecter: @escaping RCTPromiseRejectBlock) {
        if let state = protocolInstance?.getState() {
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
        } else {
            resolver("Stopped")
        }
    }
    
    // MARK: - Battery Management
    
    @objc func setBatteryLevel(_ level: Int,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        protocolInstance?.setBatteryLevel(level: UInt8(min(100, max(0, level))))
        resolver(nil)
    }
    
    @objc func getBatteryLevel(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        if let level = protocolInstance?.getBatteryLevel() {
            resolver(NSNumber(value: level))
        } else {
            resolver(NSNull())
        }
    }
    
    // MARK: - Relay Management
    
    @objc func setRelayPriority(_ priorityString: String,
                                resolver: @escaping RCTPromiseResolveBlock,
                                rejecter: @escaping RCTPromiseRejectBlock) {
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
            
            try protocolInstance?.setRelayPriority(priority: priority)
            resolver(nil)
        } catch {
            rejecter("ERROR_RELAY", "Failed to set relay priority: \(error.localizedDescription)", error)
        }
    }
    
    @objc func getRelayPriority(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        if let priority = protocolInstance?.getRelayPriority() {
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
        } else {
            resolver("medium")
        }
    }
    
    @objc func isRelay(_ resolver: @escaping RCTPromiseResolveBlock,
                      rejecter: @escaping RCTPromiseRejectBlock) {
        let isRelay = protocolInstance?.isRelay() ?? false
        resolver(NSNumber(value: isRelay))
    }
    
    // MARK: - Transport Metrics
    
    @objc func getTransportMetrics(_ transportType: String,
                                   resolver: @escaping RCTPromiseResolveBlock,
                                   rejecter: @escaping RCTPromiseRejectBlock) {
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
        
        if let metrics = protocolInstance?.getTransportMetrics(transportType: type) {
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
            
            try protocolInstance?.forceTransport(transportType: type)
            resolver(nil)
        } catch {
            rejecter("ERROR_TRANSPORT", "Failed to force transport: \(error.localizedDescription)", error)
        }
    }
    
    @objc func releaseTransportLock(_ resolver: @escaping RCTPromiseResolveBlock,
                                    rejecter: @escaping RCTPromiseRejectBlock) {
        protocolInstance?.releaseTransportLock()
        resolver(nil)
    }
    
    // MARK: - DORS Configuration
    
    @objc func updateDorsConfig(_ configJson: String,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        do {
            guard let jsonData = configJson.data(using: .utf8),
                  let config = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                throw NSError(domain: "OfflineProtocol", code: -1,
                            userInfo: [NSLocalizedDescriptionKey: "Invalid JSON"])
            }
            
            let dorsConfig = DorsConfig(
                preferOnline: config["preferOnline"] as? Bool ?? false,
                switchHysteresis: config["switchHysteresis"] as? Float ?? 15.0,
                switchCooldownSecs: config["switchCooldownSecs"] as? UInt64 ?? 20,
                bleToWifiRetryThreshold: config["bleToWifiRetryThreshold"] as? UInt32 ?? 2,
                rssiSwitchThreshold: config["rssiSwitchThreshold"] as? Int16 ?? -85,
                congestionQueueThreshold: config["congestionQueueThreshold"] as? UInt64 ?? 50,
                stabilityWindowSecs: config["stabilityWindowSecs"] as? UInt64 ?? 8
            )
            
            try protocolInstance?.updateDorsConfig(config: dorsConfig)
            resolver(nil)
        } catch {
            rejecter("ERROR_CONFIG", "Failed to update DORS config: \(error.localizedDescription)", error)
        }
    }
    
    @objc func getDorsConfig(_ resolver: @escaping RCTPromiseResolveBlock,
                            rejecter: @escaping RCTPromiseRejectBlock) {
        if let config = protocolInstance?.getDorsConfig() {
            let configDict: [String: Any] = [
                "preferOnline": config.preferOnline,
                "switchHysteresis": config.switchHysteresis,
                "switchCooldownSecs": config.switchCooldownSecs,
                "bleToWifiRetryThreshold": config.bleToWifiRetryThreshold,
                "rssiSwitchThreshold": config.rssiSwitchThreshold,
                "congestionQueueThreshold": config.congestionQueueThreshold,
                "stabilityWindowSecs": config.stabilityWindowSecs
            ]
            resolver(configDict)
        } else {
            resolver(NSNull())
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
        do {
            try protocolInstance?.process()
        } catch {
            print("Process error: \(error)")
        }
    }
}

// MARK: - EventCallback Implementation

class EventCallbackImpl: EventCallback {
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

