//
//  InternetManager.swift
//  OfflineProtocol
//
//  Internet transport implementation using WebSocket (URLSessionWebSocketTask)
//  Connects to a relay server for internet-based message routing
//

import Foundation

/// Internet Manager implementing TransportManager for WebSocket communication
public class InternetManager: NSObject, TransportManager {
    
    // MARK: - TransportManager Protocol
    
    public let transportId = "internet"
    public let transportName = "Internet (WebSocket)"
    public private(set) var state: TransportState = .unavailable
    public weak var delegate: TransportManagerDelegate?
    
    // MARK: - Constants
    
    private let MESSAGE_POLL_INTERVAL: TimeInterval = 0.1 // 100ms
    private let RECONNECT_INITIAL_DELAY: TimeInterval = 1.0
    private let RECONNECT_MAX_DELAY: TimeInterval = 30.0
    private let RECONNECT_BACKOFF_MULTIPLIER: Double = 2.0
    private let PING_INTERVAL: TimeInterval = 30.0
    private let CONNECTION_TIMEOUT: TimeInterval = 10.0
    
    // MARK: - Properties
    
    private let protocolInstance: OfflineProtocol
    private let deviceId: String
    private var serverUrl: URL?
    
    // WebSocket components
    private var webSocketTask: URLSessionWebSocketTask?
    private var urlSession: URLSession?
    
    // Message polling
    private var messageTimer: Timer?
    private var pingTimer: Timer?
    private let messageQueue = DispatchQueue(label: "com.offlineprotocol.internet.messages")
    
    // Reconnection
    private var reconnectAttempts: Int = 0
    private var currentReconnectDelay: TimeInterval = 1.0
    private var reconnectWorkItem: DispatchWorkItem?
    private var maxReconnectAttempts: Int = 0 // 0 = infinite
    private var autoReconnect: Bool = true
    
    // State tracking
    private var isConnected = false
    private var isConnecting = false
    private var transportStartAt: Date?
    
    // Metrics
    private var bytesSent: UInt64 = 0
    private var bytesReceived: UInt64 = 0
    private var messagesSent: UInt64 = 0
    private var messagesReceived: UInt64 = 0
    
    // MARK: - Initialization
    
    public init(protocol protocolInstance: OfflineProtocol, deviceId: String, serverUrl: String? = nil) {
        self.protocolInstance = protocolInstance
        self.deviceId = deviceId
        if let urlString = serverUrl, let url = URL(string: urlString) {
            self.serverUrl = url
        }
        super.init()
    }
    
    deinit {
        stop()
    }
    
    // MARK: - Configuration
    
    /// Configure the relay server URL
    public func configure(serverUrl: String, autoReconnect: Bool = true, maxReconnectAttempts: Int = 0) throws {
        guard let url = URL(string: serverUrl) else {
            throw TransportError.invalidState("Invalid server URL: \(serverUrl)")
        }
        
        self.serverUrl = url
        self.autoReconnect = autoReconnect
        self.maxReconnectAttempts = maxReconnectAttempts
        
        emitDiagnostic("info", "Internet transport configured", context: [
            "serverUrl": serverUrl,
            "autoReconnect": autoReconnect,
            "maxReconnectAttempts": maxReconnectAttempts
        ])
    }
    
    // MARK: - TransportManager Implementation
    
    public func isAvailable() -> Bool {
        return serverUrl != nil
    }
    
    public func start() throws {
        guard state != .running else {
            throw TransportError.alreadyRunning
        }
        
        guard let url = serverUrl else {
            throw TransportError.notAvailable("Server URL not configured. Call configure(serverUrl:) first.")
        }
        
        emitDiagnostic("info", "Starting Internet transport", context: [
            "deviceId": deviceId,
            "serverUrl": url.absoluteString
        ])
        
        updateState(.starting)
        transportStartAt = Date()
        
        // Create URL session
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = CONNECTION_TIMEOUT
        config.waitsForConnectivity = true
        urlSession = URLSession(configuration: config, delegate: self, delegateQueue: nil)
        
        // Connect to WebSocket
        connect()
    }
    
    public func stop() {
        guard state == .running || state == .starting else {
            return
        }
        
        updateState(.stopping)
        
        // Cancel reconnect attempts
        reconnectWorkItem?.cancel()
        reconnectWorkItem = nil
        
        // Stop timers
        stopMessagePolling()
        stopPingTimer()
        
        // Close WebSocket
        disconnect()
        
        // Notify protocol
        try? protocolInstance.internetStatusChanged(isConnected: false)
        
        updateState(.stopped)
        emitDiagnostic("info", "Internet transport stopped")
    }
    
    public func pause() {
        stopMessagePolling()
        stopPingTimer()
    }
    
    public func resume() {
        if state == .running && isConnected {
            startMessagePolling()
            startPingTimer()
        }
    }
    
    public func getMetrics() -> [String: Any] {
        return [
            "bytes_sent": bytesSent,
            "bytes_received": bytesReceived,
            "messages_sent": messagesSent,
            "messages_received": messagesReceived,
            "is_connected": isConnected,
            "reconnect_attempts": reconnectAttempts
        ]
    }
    
    // MARK: - Connection Management
    
    private func connect() {
        guard let url = serverUrl else { return }
        guard !isConnecting && !isConnected else { return }
        
        isConnecting = true
        
        // Create WebSocket task
        var request = URLRequest(url: url)
        request.timeoutInterval = CONNECTION_TIMEOUT
        // Add device ID header for identification
        request.setValue(deviceId, forHTTPHeaderField: "X-Device-ID")
        
        webSocketTask = urlSession?.webSocketTask(with: request)
        webSocketTask?.resume()
        
        emitDiagnostic("info", "Connecting to WebSocket", context: [
            "url": url.absoluteString
        ])
        
        // Start receiving messages
        receiveMessage()
    }
    
    private func disconnect() {
        webSocketTask?.cancel(with: .goingAway, reason: nil)
        webSocketTask = nil
        isConnected = false
        isConnecting = false
    }
    
    private func handleConnectionOpened() {
        isConnected = true
        isConnecting = false
        reconnectAttempts = 0
        currentReconnectDelay = RECONNECT_INITIAL_DELAY
        
        updateState(.running)
        
        // Notify protocol
        try? protocolInstance.internetStatusChanged(isConnected: true)
        
        // Start polling and pinging
        startMessagePolling()
        startPingTimer()
        
        emitDiagnostic("info", "WebSocket connected", context: [
            "serverUrl": serverUrl?.absoluteString ?? "unknown"
        ])
    }
    
    private func handleConnectionClosed(error: Error?) {
        let wasConnected = isConnected
        isConnected = false
        isConnecting = false
        
        stopMessagePolling()
        stopPingTimer()
        
        if wasConnected {
            // Notify protocol of disconnection
            try? protocolInstance.internetStatusChanged(isConnected: false)
        }
        
        emitDiagnostic("warning", "WebSocket disconnected", context: [
            "error": error?.localizedDescription ?? "none",
            "wasConnected": wasConnected
        ])
        
        // Attempt reconnection if enabled
        if autoReconnect && state != .stopping && state != .stopped {
            scheduleReconnect()
        } else {
            updateState(.stopped)
        }
    }
    
    private func scheduleReconnect() {
        guard autoReconnect else { return }
        guard maxReconnectAttempts == 0 || reconnectAttempts < maxReconnectAttempts else {
            emitDiagnostic("error", "Max reconnect attempts reached", context: [
                "attempts": reconnectAttempts,
                "maxAttempts": maxReconnectAttempts
            ])
            updateState(.stopped)
            return
        }
        
        reconnectAttempts += 1
        
        let delay = currentReconnectDelay
        currentReconnectDelay = min(currentReconnectDelay * RECONNECT_BACKOFF_MULTIPLIER, RECONNECT_MAX_DELAY)
        
        emitDiagnostic("info", "Scheduling reconnect", context: [
            "attempt": reconnectAttempts,
            "delaySeconds": delay
        ])
        
        reconnectWorkItem?.cancel()
        let workItem = DispatchWorkItem { [weak self] in
            self?.connect()
        }
        reconnectWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: workItem)
    }
    
    // MARK: - Message Handling
    
    private func receiveMessage() {
        webSocketTask?.receive { [weak self] result in
            guard let self = self else { return }
            
            switch result {
            case .success(let message):
                self.handleReceivedMessage(message)
                // Continue receiving
                self.receiveMessage()
                
            case .failure(let error):
                self.handleConnectionClosed(error: error)
            }
        }
    }
    
    private func handleReceivedMessage(_ message: URLSessionWebSocketTask.Message) {
        switch message {
        case .data(let data):
            processReceivedData(data)
        case .string(let text):
            if let data = text.data(using: .utf8) {
                processReceivedData(data)
            }
        @unknown default:
            emitDiagnostic("warning", "Unknown WebSocket message type")
        }
    }
    
    private func processReceivedData(_ data: Data) {
        bytesReceived += UInt64(data.count)
        messagesReceived += 1
        
        // Try to extract sender ID from the message
        // Assuming JSON format with "sender" field
        var senderId = "relay-server"
        if let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let sender = json["sender"] as? String {
            senderId = sender
        }
        
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            
            do {
                let bytes = [UInt8](data)
                try self.protocolInstance.internetMessageReceived(senderId: senderId, data: bytes)
                
                self.emitDiagnostic("debug", "Internet message received", context: [
                    "senderId": senderId,
                    "size": data.count
                ])
            } catch {
                self.emitDiagnostic("error", "Error processing internet message", context: [
                    "error": error.localizedDescription
                ])
            }
        }
    }
    
    private func startMessagePolling() {
        stopMessagePolling()
        
        messageTimer = Timer.scheduledTimer(
            withTimeInterval: MESSAGE_POLL_INTERVAL,
            repeats: true
        ) { [weak self] _ in
            self?.pollAndSendMessages()
        }
        
        RunLoop.current.add(messageTimer!, forMode: .common)
    }
    
    private func stopMessagePolling() {
        messageTimer?.invalidate()
        messageTimer = nil
    }
    
    private func pollAndSendMessages() {
        guard isConnected else { return }
        
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            
            // Poll for next message from protocol
            if let message = self.protocolInstance.internetGetNextMessage() {
                self.sendMessage(recipientId: message.recipientId, data: Data(message.data))
            }
        }
    }
    
    private func sendMessage(recipientId: String, data: Data) {
        guard isConnected, let task = webSocketTask else {
            emitDiagnostic("warning", "Cannot send message - not connected")
            return
        }
        
        let message = URLSessionWebSocketTask.Message.data(data)
        
        task.send(message) { [weak self] error in
            guard let self = self else { return }
            
            if let error = error {
                self.emitDiagnostic("error", "Failed to send WebSocket message", context: [
                    "error": error.localizedDescription,
                    "recipientId": recipientId
                ])
            } else {
                self.bytesSent += UInt64(data.count)
                self.messagesSent += 1
                
                self.emitDiagnostic("debug", "Message sent via WebSocket", context: [
                    "recipientId": recipientId,
                    "size": data.count
                ])
            }
        }
    }
    
    // MARK: - Ping/Pong
    
    private func startPingTimer() {
        stopPingTimer()
        
        pingTimer = Timer.scheduledTimer(
            withTimeInterval: PING_INTERVAL,
            repeats: true
        ) { [weak self] _ in
            self?.sendPing()
        }
        
        RunLoop.current.add(pingTimer!, forMode: .common)
    }
    
    private func stopPingTimer() {
        pingTimer?.invalidate()
        pingTimer = nil
    }
    
    private func sendPing() {
        webSocketTask?.sendPing { [weak self] error in
            if let error = error {
                self?.emitDiagnostic("warning", "Ping failed", context: [
                    "error": error.localizedDescription
                ])
            }
        }
    }
    
    // MARK: - State Management
    
    private func updateState(_ newState: TransportState) {
        state = newState
        delegate?.transportManager(self, didChangeState: newState)
    }
    
    // MARK: - Diagnostics
    
    private func emitDiagnostic(_ level: String, _ message: String, context: [String: Any] = [:]) {
        delegate?.transportManager(self, didEmitDiagnostic: level, message: message, context: context)
    }
}

// MARK: - URLSessionWebSocketDelegate

extension InternetManager: URLSessionWebSocketDelegate {
    
    public func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask, didOpenWithProtocol protocol: String?) {
        DispatchQueue.main.async { [weak self] in
            self?.handleConnectionOpened()
        }
    }
    
    public func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask, didCloseWith closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?) {
        DispatchQueue.main.async { [weak self] in
            let reasonString = reason.flatMap { String(data: $0, encoding: .utf8) }
            self?.emitDiagnostic("info", "WebSocket closed", context: [
                "closeCode": closeCode.rawValue,
                "reason": reasonString ?? "none"
            ])
            self?.handleConnectionClosed(error: nil)
        }
    }
    
    public func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        if let error = error {
            DispatchQueue.main.async { [weak self] in
                self?.handleConnectionClosed(error: error)
            }
        }
    }
}

extension InternetManager: @unchecked Sendable {}

