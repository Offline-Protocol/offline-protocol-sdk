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
    private var isAuthenticated = false
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
        isAuthenticated = false
    }
    
    private func handleConnectionOpened() {
        isConnected = true
        isConnecting = false
        isAuthenticated = false
        reconnectAttempts = 0
        currentReconnectDelay = RECONNECT_INITIAL_DELAY
        
        emitDiagnostic("info", "WebSocket connected, authenticating...", context: [
            "serverUrl": serverUrl?.absoluteString ?? "unknown"
        ])
        
        // Authenticate with the relay server using deviceId as the user ID
        sendAuthentication()
    }
    
    private func sendAuthentication() {
        // In test mode, the token becomes the user ID
        let authMessage: [String: Any] = [
            "type": "Authenticate",
            "token": deviceId
        ]
        
        guard let jsonData = try? JSONSerialization.data(withJSONObject: authMessage),
              let jsonString = String(data: jsonData, encoding: .utf8) else {
            emitDiagnostic("error", "Failed to create auth message")
            return
        }
        
        webSocketTask?.send(.string(jsonString)) { [weak self] error in
            if let error = error {
                self?.emitDiagnostic("error", "Failed to send auth message", context: [
                    "error": error.localizedDescription
                ])
            } else {
                self?.emitDiagnostic("debug", "Auth message sent", context: [
                    "userId": self?.deviceId ?? "unknown"
                ])
            }
        }
    }
    
    private func handleAuthenticated(userId: String, username: String) {
        isAuthenticated = true
        
        updateState(.running)
        
        // Notify protocol
        try? protocolInstance.internetStatusChanged(isConnected: true)
        
        // Start polling and pinging
        startMessagePolling()
        startPingTimer()
        
        emitDiagnostic("info", "Authenticated with relay server", context: [
            "userId": userId,
            "username": username
        ])
    }
    
    private func handleConnectionClosed(error: Error?) {
        let wasConnected = isConnected
        let wasAuthenticated = isAuthenticated
        isConnected = false
        isConnecting = false
        isAuthenticated = false
        
        stopMessagePolling()
        stopPingTimer()
        
        if wasConnected || wasAuthenticated {
            // Notify protocol of disconnection
            try? protocolInstance.internetStatusChanged(isConnected: false)
        }
        
        emitDiagnostic("warning", "WebSocket disconnected", context: [
            "error": error?.localizedDescription ?? "none",
            "wasConnected": wasConnected,
            "wasAuthenticated": wasAuthenticated
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
        
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let messageType = json["type"] as? String else {
            emitDiagnostic("warning", "Received non-JSON or invalid message", context: [
                "size": data.count
            ])
            return
        }
        
        switch messageType {
        case "Authenticated":
            // Handle authentication success
            let userId = json["user_id"] as? String ?? deviceId
            let username = json["username"] as? String ?? deviceId
            handleAuthenticated(userId: userId, username: username)
            
        case "AuthError":
            // Handle authentication error
            let reason = json["reason"] as? String ?? "Unknown error"
            emitDiagnostic("error", "Authentication failed", context: [
                "reason": reason
            ])
            handleConnectionClosed(error: nil)
            
        case "MessageReceived":
            // Handle incoming direct message
            guard let senderId = json["sender"] as? String,
                  let content = json["content"] as? String else {
                emitDiagnostic("warning", "Invalid MessageReceived format")
                return
            }
            
            messagesReceived += 1
            
            messageQueue.async { [weak self] in
                guard let self = self else { return }
                
                do {
                    // Convert content string to bytes for the protocol
                    let contentData = content.data(using: .utf8) ?? Data()
                    let bytes = [UInt8](contentData)
                    try self.protocolInstance.internetMessageReceived(senderId: senderId, data: bytes)
                    
                    self.emitDiagnostic("debug", "Message received from relay", context: [
                        "senderId": senderId,
                        "contentLength": content.count
                    ])
                } catch {
                    self.emitDiagnostic("error", "Error processing relay message", context: [
                        "error": error.localizedDescription
                    ])
                }
            }
            
        case "DeliveryError":
            // Handle delivery error
            let recipient = json["recipient"] as? String ?? "unknown"
            let reason = json["reason"] as? String ?? "Unknown error"
            emitDiagnostic("warning", "Message delivery failed", context: [
                "recipient": recipient,
                "reason": reason
            ])
            
        case "PresenceStatus", "PresenceStatusWithLastSeen":
            // Handle presence updates (optional logging)
            let userId = json["user_id"] as? String ?? "unknown"
            let online = json["online"] as? Bool ?? false
            emitDiagnostic("debug", "Presence update", context: [
                "userId": userId,
                "online": online
            ])
            
        default:
            emitDiagnostic("debug", "Received relay message", context: [
                "type": messageType
            ])
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
        guard isConnected, isAuthenticated else { return }
        
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            
            // Poll for next message from protocol
            if let message = self.protocolInstance.internetGetNextMessage() {
                self.sendMessage(recipientId: message.recipientId, data: Data(message.data))
            }
        }
    }
    
    private func sendMessage(recipientId: String, data: Data) {
        guard isConnected, isAuthenticated, let task = webSocketTask else {
            emitDiagnostic("warning", "Cannot send message - not connected or not authenticated")
            return
        }
        
        // Convert data to string content for the relay protocol
        let content = String(data: data, encoding: .utf8) ?? data.base64EncodedString()
        
        // Wrap in relay server protocol format
        let relayMessage: [String: Any] = [
            "type": "SendMessage",
            "recipient": recipientId,
            "content": content
        ]
        
        guard let jsonData = try? JSONSerialization.data(withJSONObject: relayMessage),
              let jsonString = String(data: jsonData, encoding: .utf8) else {
            emitDiagnostic("error", "Failed to create relay message")
            return
        }
        
        task.send(.string(jsonString)) { [weak self] error in
            guard let self = self else { return }
            
            if let error = error {
                self.emitDiagnostic("error", "Failed to send WebSocket message", context: [
                    "error": error.localizedDescription,
                    "recipientId": recipientId
                ])
            } else {
                self.bytesSent += UInt64(jsonData.count)
                self.messagesSent += 1
                
                self.emitDiagnostic("debug", "Message sent via relay", context: [
                    "recipientId": recipientId,
                    "contentLength": content.count
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

