import Foundation
import React

/**
 * React Native module for Offline Protocol SDK (iOS)
 * 
 * This module wraps the Rust FFI bindings and provides a JavaScript-accessible API.
 */
@objc(OfflineProtocolModule)
class OfflineProtocolModule: RCTEventEmitter {
    
    private var protocolHandle: OpaquePointer?
    
    override init() {
        super.init()
    }
    
    override static func requiresMainQueueSetup() -> Bool {
        return false
    }
    
    override func supportedEvents() -> [String]! {
        return ["OfflineProtocolEvent"]
    }
    
    /**
     * Starts the protocol with the given configuration.
     */
    @objc
    func start(_ configJson: String, resolver: @escaping RCTPromiseResolveBlock, rejecter: @escaping RCTPromiseRejectBlock) {
        // Call Rust FFI to create protocol
        let handle = offline_protocol_create(configJson)
        
        if handle == nil {
            rejecter("INIT_ERROR", "Failed to create protocol instance", nil)
            return
        }
        
        protocolHandle = handle
        
        // Start protocol
        let result = offline_protocol_start(handle)
        
        if result == 0 {
            resolver(nil)
        } else {
            rejecter("START_ERROR", "Failed to start protocol: error code \\(result)", nil)
        }
    }
    
    /**
     * Stops the protocol.
     */
    @objc
    func stop(_ resolver: @escaping RCTPromiseResolveBlock, rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("NOT_STARTED", "Protocol not started", nil)
            return
        }
        
        let result = offline_protocol_stop(handle)
        
        if result == 0 {
            offline_protocol_destroy(handle)
            protocolHandle = nil
            resolver(nil)
        } else {
            rejecter("STOP_ERROR", "Failed to stop protocol: error code \\(result)", nil)
        }
    }
    
    /**
     * Pauses the protocol.
     */
    @objc
    func pause(_ resolver: @escaping RCTPromiseResolveBlock, rejecter: @escaping RCTPromiseRejectBlock) {
        // TODO: Implement pause via FFI
        resolver(nil)
    }
    
    /**
     * Resumes the protocol.
     */
    @objc
    func resume(_ resolver: @escaping RCTPromiseResolveBlock, rejecter: @escaping RCTPromiseRejectBlock) {
        // TODO: Implement resume via FFI
        resolver(nil)
    }
    
    /**
     * Sends a message.
     */
    @objc
    func sendMessage(_ recipient: String, content: String, priority: Int, resolver: @escaping RCTPromiseResolveBlock, rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("NOT_STARTED", "Protocol not started", nil)
            return
        }
        
        var messageIdBuffer = [CChar](repeating: 0, count: 256)
        
        let result = offline_protocol_send_message(
            handle,
            recipient,
            content,
            Int32(priority),
            &messageIdBuffer,
            messageIdBuffer.count
        )
        
        if result == 0 {
            let messageId = String(cString: messageIdBuffer)
            resolver(messageId)
        } else {
            rejecter("SEND_ERROR", "Failed to send message: error code \\(result)", nil)
        }
    }
    
    /**
     * Sends a file.
     */
    @objc
    func sendFile(_ recipient: String, filePath: String, priority: Int, resolver: @escaping RCTPromiseResolveBlock, rejecter: @escaping RCTPromiseRejectBlock) {
        // TODO: Implement file transfer
        rejecter("NOT_IMPLEMENTED", "File transfer not yet implemented", nil)
    }
}

