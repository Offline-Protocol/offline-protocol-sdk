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

