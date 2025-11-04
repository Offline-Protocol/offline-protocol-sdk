import Foundation
import React

/**
 * React Native module for Offline Protocol SDK.
 * Bridges JavaScript calls to the native C FFI library.
 */
@objc(OfflineProtocolModule)
class OfflineProtocolModule: RCTEventEmitter {

    private var protocolHandle: OpaquePointer?
    private var eventPollingTimer: Timer?
    private var isPolling = false

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
    func start(_ configJson: String, resolver resolve: @escaping RCTPromiseResolveBlock, rejecter reject: @escaping RCTPromiseRejectBlock) {
        if protocolHandle != nil {
            reject("ALREADY_STARTED", "Protocol is already started", nil)
            return
        }

        guard let configCString = configJson.cString(using: .utf8) else {
            reject("INVALID_CONFIG", "Failed to convert config to C string", nil)
            return
        }

        // Create protocol instance
        let handle = offline_protocol_create(configCString)
        if handle == nil {
            reject("CREATE_FAILED", "Failed to create protocol instance", nil)
            return
        }

        // Start the protocol
        let result = offline_protocol_start(handle)
        if result != SUCCESS {
            offline_protocol_destroy(handle)
            reject("START_FAILED", "Failed to start protocol: error code \(result)", nil)
            return
        }

        protocolHandle = handle
        startEventPolling()
        resolve(nil)
    }

    /**
     * Stops the protocol.
     */
    @objc
    func stop(_ resolve: @escaping RCTPromiseResolveBlock, rejecter reject: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            reject("NOT_STARTED", "Protocol is not started", nil)
            return
        }

        stopEventPolling()

        let result = offline_protocol_stop(handle)
        if result != SUCCESS {
            reject("STOP_FAILED", "Failed to stop protocol: error code \(result)", nil)
            return
        }

        offline_protocol_destroy(handle)
        protocolHandle = nil
        resolve(nil)
    }

    /**
     * Pauses the protocol (for background mode).
     */
    @objc
    func pause(_ resolve: @escaping RCTPromiseResolveBlock, rejecter reject: @escaping RCTPromiseRejectBlock) {
        guard protocolHandle != nil else {
            reject("NOT_STARTED", "Protocol is not started", nil)
            return
        }

        stopEventPolling()
        resolve(nil)
    }

    /**
     * Resumes the protocol from pause.
     */
    @objc
    func resume(_ resolve: @escaping RCTPromiseResolveBlock, rejecter reject: @escaping RCTPromiseRejectBlock) {
        guard protocolHandle != nil else {
            reject("NOT_STARTED", "Protocol is not started", nil)
            return
        }

        startEventPolling()
        resolve(nil)
    }

    /**
     * Sends a message.
     */
    @objc
    func sendMessage(_ recipient: String, content: String, priority: NSNumber, resolver resolve: @escaping RCTPromiseResolveBlock, rejecter reject: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            reject("NOT_STARTED", "Protocol is not started", nil)
            return
        }

        guard let recipientCString = recipient.cString(using: .utf8),
              let contentCString = content.cString(using: .utf8) else {
            reject("INVALID_UTF8", "Failed to convert strings to UTF-8", nil)
            return
        }

        // Allocate buffer for message ID
        let messageIdBufferSize = 256
        let messageIdBuffer = UnsafeMutablePointer<CChar>.allocate(capacity: messageIdBufferSize)
        defer { messageIdBuffer.deallocate() }
        messageIdBuffer.initialize(repeating: 0, count: messageIdBufferSize)

        let result = offline_protocol_send_message(
            handle,
            recipientCString,
            contentCString,
            priority.int32Value,
            messageIdBuffer,
            messageIdBufferSize
        )

        if result != SUCCESS {
            reject("SEND_FAILED", "Failed to send message: error code \(result)", nil)
            return
        }

        // Extract message ID from buffer
        let messageId = String(cString: messageIdBuffer)
        resolve(messageId)
    }

    /**
     * Sends a file.
     * Note: File transfer functionality is not yet implemented in the FFI layer.
     */
    @objc
    func sendFile(_ recipient: String, filePath: String, priority: NSNumber, resolver resolve: @escaping RCTPromiseResolveBlock, rejecter reject: @escaping RCTPromiseRejectBlock) {
        reject("NOT_IMPLEMENTED", "File transfer is not yet implemented", nil)
    }

    /**
     * Starts polling for events from the native layer.
     */
    private func startEventPolling() {
        if isPolling {
            return
        }

        isPolling = true
        eventPollingTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            self?.pollAndEmitEvents()
        }
    }

    /**
     * Stops event polling.
     */
    private func stopEventPolling() {
        eventPollingTimer?.invalidate()
        eventPollingTimer = nil
        isPolling = false
    }

    /**
     * Polls for events and emits them to JavaScript.
     */
    private func pollAndEmitEvents() {
        guard let handle = protocolHandle else {
            return
        }

        // Allocate buffer for event JSON
        let eventBufferSize = 4096
        let eventBuffer = UnsafeMutablePointer<CChar>.allocate(capacity: eventBufferSize)
        defer { eventBuffer.deallocate() }
        eventBuffer.initialize(repeating: 0, count: eventBufferSize)

        let result = offline_protocol_poll_event(handle, eventBuffer, eventBufferSize)

        if result == 0 {
            // No event available
            return
        }

        if result < 0 {
            // Error occurred
            return
        }

        // Extract event JSON from buffer
        let eventJson = String(cString: eventBuffer)
        if !eventJson.isEmpty {
            do {
                guard let jsonData = eventJson.data(using: .utf8),
                      let jsonObject = try JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                    return
                }

                let eventType = jsonObject["type"] as? String ?? ""
                
                // Map snake_case event types to JavaScript event names
                let jsEventType = mapEventType(eventType)

                var eventDict: [String: Any] = [
                    "type": jsEventType
                ]

                // Copy all other fields from JSON
                for (key, value) in jsonObject {
                    if key != "type" {
                        eventDict[key] = value
                    }
                }

                // Emit event to JavaScript
                sendEvent(withName: "OfflineProtocolEvent", body: eventDict)
            } catch {
                // Ignore parsing errors
            }
        }
    }

    /**
     * Maps Rust event types (snake_case) to JavaScript event names.
     */
    private func mapEventType(_ rustType: String) -> String {
        switch rustType {
        case "message_sent": return "message:sent"
        case "message_received": return "message:received"
        case "message_delivered": return "message:delivered"
        case "message_failed": return "message:failed"
        case "transport_switched": return "transport:switched"
        case "relay_promoted": return "relay:promoted"
        case "relay_demoted": return "relay:demoted"
        case "neighbor_discovered": return "neighbor:discovered"
        case "neighbor_lost": return "neighbor:lost"
        case "network_metrics": return "network:metrics"
        case "file_progress": return "file:progress"
        case "file_received": return "file:received"
        default: return rustType
        }
    }

    deinit {
        stopEventPolling()
        if let handle = protocolHandle {
            offline_protocol_destroy(handle)
        }
    }
}
