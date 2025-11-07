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
    private let processQueue = DispatchQueue(label: "offlineprotocol.processor")
    private var processTimer: DispatchSourceTimer?
    private var bleRecipientBuffer = [CChar](repeating: 0, count: OfflineProtocolModule.bleRecipientBufferSize)
    private var bleFragmentBuffer = [UInt8](repeating: 0, count: OfflineProtocolModule.bleFragmentBufferSize)
    private var hasListeners = false
    private var bleSendSuccessCount: UInt32 = 0
    private var bleSendFailureCount: UInt32 = 0
    private var bleLastRssi: Int16 = -1
    private var blePeerRefreshTimestamps: [String: Date] = [:]
    private let blePeerUpdateThrottle: TimeInterval = 2.0
    
    // Event names
    private enum Events {
        static let onEvent = "OfflineProtocol_Event"
    }

    private static let bleRecipientBufferSize = 512
    private static let bleFragmentBufferSize = 65536
    
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
        stopProcessTimer()
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
        
        blePeerRefreshTimestamps.removeAll()
        
        // Initialize BLE manager on main thread
        // CoreBluetooth managers must be created on the same queue they'll be used on
        DispatchQueue.main.async {
            self.initializeBleManager()
        }
        
        // Create new protocol instance
        guard let handle = configJson.withCString({ offline_protocol_create($0) }) else {
            rejecter("ERROR_CREATE_FAILED", "Failed to create protocol instance", nil)
            return
        }
        
        protocolHandle = handle
        
        // Start process timer for retries and cleanup
        startProcessTimer()
        
        // Set up event callback
        let unmanagedSelf = Unmanaged.passRetained(self)
        eventCallbackContext = unmanagedSelf.toOpaque()
        
        var callbackOption = Option_EventCallback(is_some: true, value: eventCallbackHandler)
        let result = offline_protocol_set_event_callback(
            handle,
            callbackOption,
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
        
        // Optionally enable Internet and WiFi Direct transports based on config
        if let jsonData = configJson.data(using: .utf8),
           let config = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any],
           let transports = config["transports"] as? [String: Any] {
            
            // Enable Internet transport if configured
            if let internetConfig = transports["internet"] as? [String: Any],
               let enabled = internetConfig["enabled"] as? Bool, enabled {
                do {
                    let internetConfigData = try JSONSerialization.data(withJSONObject: internetConfig)
                    if let internetConfigJson = String(data: internetConfigData, encoding: .utf8) {
                        let result = internetConfigJson.withCString { configPtr in
                            offline_protocol_add_internet_transport(handle, configPtr)
                        }
                        if result == SUCCESS {
                            NSLog("[OfflineProtocol] Internet transport enabled")
                        } else {
                            NSLog("[OfflineProtocol] Failed to enable Internet transport: \(result)")
                        }
                    }
                } catch {
                    NSLog("[OfflineProtocol] Error serializing Internet config: \(error)")
                }
            }
            
            // Enable WiFi Direct transport if configured
            if let wifiDirectConfig = transports["wifiDirect"] as? [String: Any],
               let enabled = wifiDirectConfig["enabled"] as? Bool, enabled {
                do {
                    let wifiDirectConfigData = try JSONSerialization.data(withJSONObject: wifiDirectConfig)
                    if let wifiDirectConfigJson = String(data: wifiDirectConfigData, encoding: .utf8) {
                        let result = wifiDirectConfigJson.withCString { configPtr in
                            offline_protocol_add_wifi_direct_transport(handle, configPtr)
                        }
                        if result == SUCCESS {
                            NSLog("[OfflineProtocol] WiFi Direct transport enabled")
                        } else {
                            NSLog("[OfflineProtocol] Failed to enable WiFi Direct transport: \(result)")
                        }
                    }
                } catch {
                    NSLog("[OfflineProtocol] Error serializing WiFi Direct config: \(error)")
                }
            }
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
        stopProcessTimer()
        bleManager?.stop()
        bleManager = nil
        blePeerRefreshTimestamps.removeAll()
        
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
        blePeerRefreshTimestamps.removeAll()
        
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
        
        NSLog("[OfflineProtocol] Calling sendMessage(recipient:\(recipient), priority:\(priority))")
        let buffer = UnsafeMutablePointer<CChar>.allocate(capacity: 256)
        defer { buffer.deallocate() }
        
        // Initialize buffer to prevent crashes on error paths
        buffer.initialize(repeating: 0, count: 256)
        
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
            NSLog("[OfflineProtocol] sendMessage succeeded with id \(messageId)")
            resolver(messageId)
        case ERROR_NOT_STARTED:
            NSLog("[OfflineProtocol] sendMessage failed: protocol not started")
            rejecter("ERROR_NOT_STARTED", "Protocol not started", nil)
        case ERROR_SEND_FAILED:
            NSLog("[OfflineProtocol] sendMessage failed: transport send error")
            rejecter("ERROR_SEND_FAILED", "Failed to send message", nil)
        default:
            NSLog("[OfflineProtocol] sendMessage failed with unknown error code \(result)")
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

    @objc func sendFile(_ filePath: String,
                       recipient: String,
                       fileName: String,
                       resolver: @escaping RCTPromiseResolveBlock,
                       rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }

        // Read file data
        guard let fileData = try? Data(contentsOf: URL(fileURLWithPath: filePath)) else {
            rejecter("ERROR_FILE_NOT_FOUND", "File not found: \(filePath)", nil)
            return
        }

        let fileIdBuffer = UnsafeMutablePointer<CChar>.allocate(capacity: 256)
        defer { fileIdBuffer.deallocate() }

        let result = fileData.withUnsafeBytes { (buffer: UnsafeRawBufferPointer) -> Int32 in
            guard let baseAddress = buffer.baseAddress?.assumingMemoryBound(to: UInt8.self) else {
                return ERROR_OTHER
            }
            return fileName.withCString { fileNamePtr in
                recipient.withCString { recipientPtr in
                    offline_protocol_send_file(
                        handle,
                        baseAddress,
                        UInt(fileData.count),
                        fileNamePtr,
                        recipientPtr,
                        fileIdBuffer,
                        UInt(256)
                    )
                }
            }
        }

        if result == SUCCESS {
            let fileId = String(cString: fileIdBuffer)
            resolver(fileId)
        } else {
            rejecter("ERROR_SEND_FILE_FAILED", "Failed to send file", nil)
        }
    }

    @objc func getFileProgress(_ fileId: String,
                              resolver: @escaping RCTPromiseResolveBlock,
                              rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }

        let buffer = UnsafeMutablePointer<CChar>.allocate(capacity: 4096)
        defer { buffer.deallocate() }

        let result = fileId.withCString { fileIdPtr in
            offline_protocol_get_file_progress(handle, fileIdPtr, buffer, UInt(4096))
        }

        if result == SUCCESS {
            let progressJson = String(cString: buffer)
            if let data = progressJson.data(using: .utf8),
               let jsonObject = try? JSONSerialization.jsonObject(with: data) {
                resolver(jsonObject)
            } else {
                resolver(progressJson)
            }
        } else if result == 0 {
            resolver(NSNull())
        } else {
            rejecter("ERROR_GET_PROGRESS_FAILED", "Failed to get file progress", nil)
        }
    }

    @objc func cancelFileTransfer(_ fileId: String,
                                  resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }

        let result = fileId.withCString { fileIdPtr in
            offline_protocol_cancel_file_transfer(handle, fileIdPtr)
        }

        resolver(result > 0)
    }

    @objc func receiveMessage(_ resolver: @escaping RCTPromiseResolveBlock,
                             rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }

        let buffer = UnsafeMutablePointer<CChar>.allocate(capacity: 65536)
        defer { buffer.deallocate() }

        let result = offline_protocol_receive_message(handle, buffer, UInt(65536))

        if result == SUCCESS {
            let messageJson = String(cString: buffer)
            if let data = messageJson.data(using: .utf8),
               let jsonObject = try? JSONSerialization.jsonObject(with: data) {
                resolver(jsonObject)
            } else {
                resolver(messageJson)
            }
        } else if result == NO_MESSAGE_AVAILABLE {
            resolver(NSNull())
        } else {
            rejecter("ERROR_RECEIVE_FAILED", "Failed to receive message", nil)
        }
    }

    @objc func pause(_ resolver: @escaping RCTPromiseResolveBlock,
                    rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }

        let result = offline_protocol_pause(handle)

        switch result {
        case SUCCESS:
            resolver(nil)
        case ERROR_NOT_STARTED:
            rejecter("ERROR_NOT_STARTED", "Protocol not started", nil)
        default:
            rejecter("ERROR_PAUSE_FAILED", "Failed to pause protocol", nil)
        }
    }

    @objc func resume(_ resolver: @escaping RCTPromiseResolveBlock,
                     rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }

        let result = offline_protocol_resume(handle)

        switch result {
        case SUCCESS:
            resolver(nil)
        default:
            rejecter("ERROR_RESUME_FAILED", "Failed to resume protocol", nil)
        }
    }

    @objc func getState(_ resolver: @escaping RCTPromiseResolveBlock,
                       rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }

        let state = offline_protocol_get_state(handle)

        if state < 0 {
            rejecter("ERROR_GET_STATE_FAILED", "Failed to get protocol state", nil)
        } else {
            resolver(NSNumber(value: state))
        }
    }

    @objc func enableTransport(_ type: String,
                              config: NSDictionary?,
                              resolver: @escaping RCTPromiseResolveBlock,
                              rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }

        let configJson: String? = config.flatMap { dict in
            guard let data = try? JSONSerialization.data(withJSONObject: dict),
                  let json = String(data: data, encoding: .utf8) else {
                return nil
            }
            return json
        }

        let result: Int32
        switch type {
        case "internet":
            result = configJson.flatMap { json in
                json.withCString { ptr in
                    offline_protocol_add_internet_transport(handle, ptr)
                }
            } ?? offline_protocol_add_internet_transport(handle, nil)
        case "wifiDirect":
            result = configJson.flatMap { json in
                json.withCString { ptr in
                    offline_protocol_add_wifi_direct_transport(handle, ptr)
                }
            } ?? offline_protocol_add_wifi_direct_transport(handle, nil)
        case "ble":
            result = SUCCESS // BLE is always enabled
        default:
            rejecter("ERROR_INVALID_TRANSPORT", "Invalid transport type: \(type)", nil)
            return
        }

        if result == SUCCESS {
            resolver(nil)
        } else {
            rejecter("ERROR_ENABLE_TRANSPORT_FAILED", "Failed to enable transport", nil)
        }
    }

    @objc func disableTransport(_ type: String,
                               resolver: @escaping RCTPromiseResolveBlock,
                               rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }

        let transportType: Int32
        switch type {
        case "internet":
            transportType = 0
        case "ble":
            transportType = 1
        case "wifiDirect":
            transportType = 2
        default:
            rejecter("ERROR_INVALID_TRANSPORT", "Invalid transport type: \(type)", nil)
            return
        }

        let result = offline_protocol_remove_transport(handle, transportType)

        if result == SUCCESS {
            resolver(nil)
        } else {
            rejecter("ERROR_DISABLE_TRANSPORT_FAILED", "Failed to disable transport", nil)
        }
    }

    @objc func getActiveTransports(_ resolver: @escaping RCTPromiseResolveBlock,
                                  rejecter: @escaping RCTPromiseRejectBlock) {
        guard let handle = protocolHandle else {
            rejecter("ERROR_NOT_INITIALIZED", "Protocol not initialized", nil)
            return
        }

        let buffer = UnsafeMutablePointer<CChar>.allocate(capacity: 4096)
        defer { buffer.deallocate() }

        let result = offline_protocol_get_active_transports(handle, buffer, UInt(4096))

        if result == SUCCESS {
            let transportsJson = String(cString: buffer)
            if let data = transportsJson.data(using: .utf8),
               let jsonArray = try? JSONSerialization.jsonObject(with: data) {
                resolver(jsonArray)
            } else {
                resolver(transportsJson)
            }
        } else {
            rejecter("ERROR_GET_TRANSPORTS_FAILED", "Failed to get active transports", nil)
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

    private func startProcessTimer() {
        guard processTimer == nil else { return }

        let timer = DispatchSource.makeTimerSource(queue: processQueue)
        timer.schedule(deadline: .now(), repeating: .milliseconds(500))
        timer.setEventHandler { [weak self] in
            guard let self = self,
                  let handle = self.protocolHandle else {
                return
            }
            _ = offline_protocol_process(handle)
        }
        timer.resume()
        processTimer = timer
    }

    private func stopProcessTimer() {
        processTimer?.cancel()
        processTimer = nil
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
                recordBleSendFailure()
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
            } else {
                recordBleSendSuccess()
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
    
    private func updateBleMetrics(rssi: Int16 = -1) {
        guard let handle = protocolHandle else { return }
        
        // Update RSSI if provided
        if rssi != -1 {
            bleLastRssi = rssi
        }
        
        // Transport type 1 = BLE
        let result = offline_protocol_update_transport_metrics(
            handle,
            1, // BLE
            bleLastRssi,
            0, // latencyMs - not tracking yet
            150_000, // bandwidthBps - typical BLE ~150 KB/s
            0.0, // congestion
            UInt(0), // queueDepth - BLE queue is managed in Rust
            bleSendSuccessCount,
            bleSendFailureCount
        )
        
        if result != SUCCESS {
            NSLog("[OfflineProtocol] Failed to update BLE metrics: \(result)")
        }
    }
    
    private func recordBleSendSuccess() {
        bleSendSuccessCount += 1
        updateBleMetrics()
    }
    
    private func recordBleSendFailure() {
        bleSendFailureCount += 1
        updateBleMetrics()
    }
    
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
                
                // Update BLE metrics with RSSI value
                self?.updateBleMetrics(rssi: Int16(rssi))
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
        
        manager.onPeerUpdated = { [weak self] peerId, address, rssi in
            guard let self = self else { return }

            let now = Date()
            if let last = self.blePeerRefreshTimestamps[peerId], now.timeIntervalSince(last) < self.blePeerUpdateThrottle {
                return
            }
            self.blePeerRefreshTimestamps[peerId] = now

            guard let handle = self.protocolHandle else { return }

            peerId.withCString { peerIdPtr in
                address.withCString { addressPtr in
                    let result = offline_protocol_ble_peer_discovered(handle, peerIdPtr, addressPtr, Int16(rssi))
                    if result != SUCCESS {
                        NSLog("[OfflineProtocol] Failed to refresh BLE peer discovery for \(peerId): \(result)")
                    }
                }
            }

            self.updateBleMetrics(rssi: Int16(rssi))
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
            
            // Note: transport_switched events are now emitted by Rust DORS core
            // No need to synthesize them here
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

