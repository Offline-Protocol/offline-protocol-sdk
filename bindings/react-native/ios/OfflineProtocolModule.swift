//
//  OfflineProtocolModule.swift
//  OfflineProtocol
//
//  React Native module for Offline Protocol SDK
//

import Foundation
import React

@objc(OfflineProtocolModule)
class OfflineProtocolModule: RCTEventEmitter {
    private var protocolHandle: OpaquePointer?
    private var eventCallbackContext: UnsafeMutableRawPointer?
    private var bleManager: BleManager?
    private var deviceId: String = ""
    private let bleFragmentQueue = DispatchQueue(label: "offlineprotocol.ble.fragment-pump")
    private var bleFragmentTimer: DispatchSourceTimer?
    private var bleRecipientBuffer = [CChar](repeating: 0, count: 256)
    private var bleFragmentBuffer = [UInt8](repeating: 0, count: 512)
    private var hasListeners = false
    
    // Event names
    private enum Events {
        static let onEvent = "OfflineProtocol_Event"
    }
    
    override init() {
        super.init()
    }
    
    deinit {
        // Clean up protocol handle if not already destroyed
        if let handle = protocolHandle {
            offline_protocol_destroy(handle)
            protocolHandle = nil
        }
        
        // Clean up callback context
        if let context = eventCallbackContext {
            Unmanaged<OfflineProtocolModule>.fromOpaque(context).release()
            eventCallbackContext = nil
        }

        stopBleFragmentPump()
    }
    
    override class func requiresMainQueueSetup() -> Bool {
        return false
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
    
    // MARK: - Exported Methods
    
    @objc func create(_ configJson: String,
                     resolver: @escaping RCTPromiseResolveBlock,
                     rejecter: @escaping RCTPromiseRejectBlock) {
        // Clean up existing handle if any
        if let handle = protocolHandle {
            stopBleFragmentPump()
            offline_protocol_destroy(handle)
            protocolHandle = nil
        }
        
        // Parse config to extract userId
        if let jsonData = configJson.data(using: .utf8),
           let config = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] {
            deviceId = (config["userId"] as? String) ?? (config["user_id"] as? String) ?? ""
        }
        
        if deviceId.isEmpty {
            rejecter("ERROR_INVALID_CONFIG", "userId is required", nil)
            return
        }
        
        // Initialize BLE manager
        initializeBleManager()
        
        // Create new protocol instance
        guard let handle = configJson.withCString({ offline_protocol_create($0) }) else {
            rejecter("ERROR_CREATE_FAILED", "Failed to create protocol instance", nil)
            return
        }
        
        protocolHandle = handle
        
        // Set up event callback
        let unmanagedSelf = Unmanaged.passRetained(self)
        eventCallbackContext = unmanagedSelf.toOpaque()
        
        let result = offline_protocol_set_event_callback(
            handle,
            eventCallbackHandler,
            eventCallbackContext
        )
        
        if result != SUCCESS {
            Unmanaged<OfflineProtocolModule>.fromOpaque(eventCallbackContext!).release()
            eventCallbackContext = nil
            offline_protocol_destroy(handle)
            protocolHandle = nil
            rejecter("ERROR_CALLBACK_FAILED", "Failed to set event callback", nil)
            return
        }
        
        resolver(nil)
    }
    
    @objc func destroy(_ resolver: @escaping RCTPromiseResolveBlock,
                      rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        
        // Stop and cleanup BLE
        stopBleFragmentPump()
        bleManager?.stop()
        bleManager = nil
        
        offline_protocol_destroy(handle)
        protocolHandle = nil
        
        // Clean up callback context
        if let context = eventCallbackContext {
            Unmanaged<OfflineProtocolModule>.fromOpaque(context).release()
            eventCallbackContext = nil
        }
        
        resolver(nil)
    }
    
    @objc func start(_ resolver: @escaping RCTPromiseResolveBlock,
                    rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        
        // Start BLE operations
        let bleStarted = bleManager?.start() ?? false
        if !bleStarted {
            NSLog("[OfflineProtocol] Failed to start BLE")
            rejecter("ERROR_BLE_START_FAILED", "Failed to start BLE. Check permissions and Bluetooth state.", nil)
            return
        }
        
        let result = offline_protocol_start(handle)
        
        switch result {
        case SUCCESS:
            startBleFragmentPump()
            resolver(nil)
        case ERROR_ALREADY_STARTED:
            rejecter("ERROR_ALREADY_STARTED", "Protocol already started", nil)
        default:
            rejecter("ERROR_START_FAILED", "Failed to start protocol", nil)
        }
    }
    
    @objc func stop(_ resolver: @escaping RCTPromiseResolveBlock,
                   rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        
        // Stop BLE first
        stopBleFragmentPump()
        bleManager?.stop()
        
        let result = offline_protocol_stop(handle)
        
        switch result {
        case SUCCESS:
            resolver(nil)
        case ERROR_NOT_STARTED:
            rejecter("ERROR_NOT_STARTED", "Protocol not started", nil)
        default:
            rejecter("ERROR_STOP_FAILED", "Failed to stop protocol", nil)
        }
    }
    
    @objc func sendMessage(_ recipient: String,
                          content: String,
                          priority: NSNumber,
                          resolver: @escaping RCTPromiseResolveBlock,
                          rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        
        let buffer = UnsafeMutablePointer<CChar>.allocate(capacity: 256)
        defer { buffer.deallocate() }
        
        let result = recipient.withCString { recipientPtr in
            content.withCString { contentPtr in
                offline_protocol_send_message(
                    handle,
                    recipientPtr,
                    contentPtr,
                    priority.int32Value,
                    buffer,
                    UInt(256)
                )
            }
        }
        
        switch result {
        case SUCCESS:
            let messageId = String(cString: buffer)
            resolver(messageId)
        case ERROR_NOT_STARTED:
            rejecter("ERROR_NOT_STARTED", "Protocol not started", nil)
        case ERROR_SEND_FAILED:
            rejecter("ERROR_SEND_FAILED", "Failed to send message", nil)
        default:
            rejecter("ERROR_UNKNOWN", "Unknown error occurred", nil)
        }
    }
    
    @objc func getTopology(_ resolver: @escaping RCTPromiseResolveBlock,
                          rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        
        let buffer = UnsafeMutablePointer<CChar>.allocate(capacity: 65536)
        defer { buffer.deallocate() }
        
        let result = offline_protocol_get_topology(handle, buffer, UInt(65536))
        
        if result == SUCCESS {
            let topologyJson = String(cString: buffer)
            resolver(topologyJson)
        } else {
            rejecter("ERROR_GET_TOPOLOGY_FAILED", "Failed to get topology", nil)
        }
    }
    
    @objc func getMessageStats(_ resolver: @escaping RCTPromiseResolveBlock,
                              rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        
        let buffer = UnsafeMutablePointer<CChar>.allocate(capacity: 65536)
        defer { buffer.deallocate() }
        
        let result = offline_protocol_get_message_stats(handle, buffer, UInt(65536))
        
        if result == SUCCESS {
            let statsJson = String(cString: buffer)
            resolver(statsJson)
        } else {
            rejecter("ERROR_GET_STATS_FAILED", "Failed to get message stats", nil)
        }
    }
    
    @objc func getDeliverySuccessRate(_ resolver: @escaping RCTPromiseResolveBlock,
                                     rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        
        var rate: Float = 0.0
        let result = offline_protocol_get_delivery_success_rate(handle, &rate)
        
        if result == SUCCESS {
            resolver(NSNumber(value: rate))
        } else {
            rejecter("ERROR_GET_RATE_FAILED", "Failed to get delivery success rate", nil)
        }
    }
    
    @objc func getMedianLatency(_ resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        
        var latency: UInt64 = 0
        let result = offline_protocol_get_median_latency(handle, &latency)
        
        if result == SUCCESS {
            resolver(NSNumber(value: latency))
        } else if result == 0 {
            resolver(NSNull())
        } else {
            rejecter("ERROR_GET_LATENCY_FAILED", "Failed to get median latency", nil)
        }
    }
    
    @objc func getMedianHops(_ resolver: @escaping RCTPromiseResolveBlock,
                            rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }
        
        var hops: UInt8 = 0
        let result = offline_protocol_get_median_hops(handle, &hops)
        
        if result == SUCCESS {
            resolver(NSNumber(value: hops))
        } else if result == 0 {
            resolver(NSNull())
        } else {
            rejecter("ERROR_GET_HOPS_FAILED", "Failed to get median hops", nil)
        }
    }

    // MARK: - BLE Fragment Handling

    private func startBleFragmentPump() {
        guard bleFragmentTimer == nil else { return }

        let timer = DispatchSource.makeTimerSource(queue: bleFragmentQueue)
        timer.schedule(deadline: .now(), repeating: .milliseconds(150))
        timer.setEventHandler { [weak self] in
            self?.flushBleFragments()
        }
        timer.resume()
        bleFragmentTimer = timer
    }

    private func stopBleFragmentPump() {
        bleFragmentTimer?.cancel()
        bleFragmentTimer = nil
    }

    private func flushBleFragments() {
        guard let handle = protocolHandle,
              let manager = bleManager else {
            return
        }

        while true {
            var fragmentLength: UInt = 0

            let result: Int32 = bleRecipientBuffer.withUnsafeMutableBufferPointer { recipientPtr in
                bleFragmentBuffer.withUnsafeMutableBufferPointer { fragmentPtr in
                    guard let recipientBase = recipientPtr.baseAddress,
                          let fragmentBase = fragmentPtr.baseAddress else {
                        return ERROR_OTHER
                    }

                    return offline_protocol_ble_get_next_fragment(
                        handle,
                        recipientBase,
                        UInt(recipientPtr.count),
                        fragmentBase,
                        UInt(fragmentPtr.count),
                        &fragmentLength
                    )
                }
            }

            if result == NO_FRAGMENT_AVAILABLE || fragmentLength == 0 {
                break
            }

            if result != SUCCESS {
                NSLog("[OfflineProtocol] Failed to fetch BLE fragment: \(result)")
                break
            }

            guard let recipient = String(validatingUTF8: bleRecipientBuffer) else {
                NSLog("[OfflineProtocol] Invalid recipient string for BLE fragment")
                continue
            }

            let length = Int(fragmentLength)
            let messageData = Data(bytes: bleFragmentBuffer, count: length)

            let sendSucceeded = manager.sendMessage(recipientId: recipient, messageData: messageData)
            if !sendSucceeded {
                recipient.withCString { recipientPtr in
                    messageData.withUnsafeBytes { buffer in
                        if let baseAddress = buffer.baseAddress?.assumingMemoryBound(to: UInt8.self) {
                            let requeueResult = offline_protocol_ble_return_fragment(
                                handle,
                                recipientPtr,
                                baseAddress,
                                fragmentLength
                            )
                            if requeueResult != SUCCESS {
                                NSLog("[OfflineProtocol] Failed to requeue BLE fragment: \(requeueResult)")
                            }
                        }
                    }
                }
                break
            }
        }
    }
    
    // MARK: - Event Callback
    
    fileprivate func handleEvent(_ eventJson: String) {
        guard hasListeners else {
            return
        }
        sendEvent(withName: Events.onEvent, body: ["eventJson": eventJson])
    }
    
    // MARK: - BLE Manager
    
    private func initializeBleManager() {
        if bleManager != nil {
            return // Already initialized
        }
        
        let manager = BleManager(deviceId: deviceId)
        
        manager.onPeerDiscovered = { [weak self] peerId, address, rssi in
            NSLog("[OfflineProtocol] Peer discovered: \(peerId) at \(address) (RSSI: \(rssi))")
            
            // Notify the Rust transport layer
            if let handle = self?.protocolHandle {
                peerId.withCString { peerIdPtr in
                    address.withCString { addressPtr in
                        let result = offline_protocol_ble_peer_discovered(handle, peerIdPtr, addressPtr, Int16(rssi))
                        if result != SUCCESS {
                            NSLog("[OfflineProtocol] Failed to notify BLE transport of peer discovery: \(result)")
                        } else {
                            NSLog("[OfflineProtocol] Successfully notified Rust transport of peer: \(peerId)")
                        }
                    }
                }
            }
            
            // Emit neighbor_discovered event (matches NetworkScreen expectations)
            let eventJson = """
            {
                "type": "neighbor_discovered",
                "peer_id": "\(peerId)",
                "transport": "ble",
                "rssi": \(rssi),
                "timestamp": \(Date().timeIntervalSince1970 * 1000)
            }
            """
            
            self?.handleEvent(eventJson)
        }
        
        manager.onPeerLost = { [weak self] peerId in
            NSLog("[OfflineProtocol] Peer lost: \(peerId)")
            
            // Notify the Rust transport layer
            if let handle = self?.protocolHandle {
                peerId.withCString { peerIdPtr in
                    let result = offline_protocol_ble_peer_lost(handle, peerIdPtr)
                    if result != SUCCESS {
                        NSLog("[OfflineProtocol] Failed to notify BLE transport of peer loss: \(result)")
                    }
                }
            }
            
            // Emit neighbor_lost event (matches NetworkScreen expectations)
            let eventJson = """
            {
                "type": "neighbor_lost",
                "peer_id": "\(peerId)",
                "timestamp": \(Date().timeIntervalSince1970 * 1000)
            }
            """
            
            self?.handleEvent(eventJson)
        }
        
        manager.onMessageReceived = { [weak self] messageData in
            NSLog("[OfflineProtocol] Message received: \(messageData.count) bytes")

            guard let handle = self?.protocolHandle else {
                return
            }

            let result = messageData.withUnsafeBytes { buffer -> Int32 in
                guard let baseAddress = buffer.baseAddress?.assumingMemoryBound(to: UInt8.self) else {
                    return ERROR_OTHER
                }
                return offline_protocol_ble_fragment_received(handle, baseAddress, UInt(messageData.count))
            }
            if result != SUCCESS {
                NSLog("[OfflineProtocol] Failed to forward BLE fragment to Rust: \(result)")

                if let messageJson = String(data: messageData, encoding: .utf8) {
                    self?.handleEvent(messageJson)
                }
            }
        }
        
        manager.onStatusChanged = { [weak self] status in
            NSLog("[OfflineProtocol] BLE status changed: \(status.rawValue)")
            
            // Notify the Rust transport layer
            if let handle = self?.protocolHandle {
                let statusCode: Int32
                switch status {
                case .unavailable:
                    statusCode = 0
                case .available, .scanning, .advertising, .connected:
                    statusCode = 1
                case .disconnected:
                    statusCode = 2
                }
                
                let result = offline_protocol_ble_status_changed(handle, statusCode)
                if result != SUCCESS {
                    NSLog("[OfflineProtocol] Failed to notify BLE transport of status change: \(result)")
                }
            }
            
            // Emit transport_switched event when BLE becomes available
            if status == .available || status == .scanning || status == .advertising {
                let eventJson = """
                {
                    "type": "transport_switched",
                    "from": null,
                    "to": "ble",
                    "reason": "BLE transport became available",
                    "timestamp": \(Date().timeIntervalSince1970 * 1000)
                }
                """
                self?.handleEvent(eventJson)
            } else if status == .disconnected {
                let eventJson = """
                {
                    "type": "transport_switched",
                    "from": "ble",
                    "to": "none",
                    "reason": "BLE transport disconnected",
                    "timestamp": \(Date().timeIntervalSince1970 * 1000)
                }
                """
                self?.handleEvent(eventJson)
            }
        }
        
        manager.onDiagnostic = { [weak self] message in
            NSLog("[OfflineProtocol] \(message)")
            
            // Emit diagnostic event to React Native
            let eventJson = """
            {
                "type": "diagnostic",
                "message": "\(message)",
                "timestamp": \(Date().timeIntervalSince1970 * 1000)
            }
            """
            self?.handleEvent(eventJson)
        }
        
        bleManager = manager
        NSLog("[OfflineProtocol] BLE manager initialized for device: \(deviceId)")
    }
}

// Global event callback function
private func eventCallbackHandler(eventJson: UnsafePointer<CChar>?, userData: UnsafeMutableRawPointer?) {
    guard let eventJson = eventJson,
          let userData = userData else {
        return
    }
    
    let jsonString = String(cString: eventJson)
    let module = Unmanaged<OfflineProtocolModule>.fromOpaque(userData).takeUnretainedValue()
    
    // Dispatch to main queue or use a serial queue
    DispatchQueue.main.async {
        module.handleEvent(jsonString)
    }
}

