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
    }
    
    override class func requiresMainQueueSetup() -> Bool {
        return false
    }
    
    override func supportedEvents() -> [String]! {
        return [Events.onEvent]
    }
    
    // MARK: - Exported Methods
    
    @objc func create(_ configJson: String,
                     resolver: @escaping RCTPromiseResolveBlock,
                     rejecter: @escaping RCTPromiseRejectBlock) {
        // Clean up existing handle if any
        if let handle = protocolHandle {
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
                    256
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
    
    // MARK: - Event Callback
    
    fileprivate func handleEvent(_ eventJson: String) {
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
            
            if let messageJson = String(data: messageData, encoding: .utf8) {
                self?.handleEvent(messageJson)
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

