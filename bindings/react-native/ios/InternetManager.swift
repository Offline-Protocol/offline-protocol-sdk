//
// InternetManager.swift
// OfflineProtocol
//
// Internet transport implementation using WebSocket (URLSessionWebSocketTask)
// Connects to a relay server for internet-based message routing
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
    private let PING_INTERVAL: TimeInterval = 10.0  // Reduced from 30s for faster failure detection
    private let CONNECTION_TIMEOUT: TimeInterval = 10.0
    
    // MARK: - Properties
    
    private let protocolInstance: OfflineProtocol
    private let deviceId: String
    private var serverUrl: URL?
    
    // WebSocket components
    private var webSocketTask: URLSessionWebSocketTask?
    private var urlSession: URLSession?
    
    // Message polling
    private var messageTimer: DispatchSourceTimer?
    private var pingTimer: DispatchSourceTimer?
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
    
    // Failure tracking for DORS
    private var consecutiveSendFailures: Int = 0
    private var consecutivePingFailures: Int = 0
    private let MAX_CONSECUTIVE_FAILURES = 2  // Trigger disconnect after 2 consecutive failures
    
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
        consecutiveSendFailures = 0
        consecutivePingFailures = 0
        
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
        
        // Notify protocol - this will trigger outbox flush for pending messages
        try? protocolInstance.internetStatusChanged(isConnected: true)
        
        // Start polling and pinging
        startMessagePolling()
        startPingTimer()
        
        // Immediately poll for messages to flush outbox after reconnection
        // This ensures messages queued during disconnection are sent promptly
        messageQueue.async { [weak self] in
            self?.pollAndSendMessages()
        }
        
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
        
        // Stop polling and pinging immediately to prevent sending on dead connection
        stopMessagePolling()
        stopPingTimer()
        
        // Always notify protocol of disconnection
        // This ensures the protocol knows the transport is unavailable
        // even if we weren't fully authenticated
        if wasConnected || wasAuthenticated {
            // Notify protocol of disconnection - this keeps outbox messages pending
            do {
                try protocolInstance.internetStatusChanged(isConnected: false)
            } catch {
                emitDiagnostic("error", "Failed to notify protocol of disconnection", context: [
                    "error": error.localizedDescription
                ])
            }
        }
        
        emitDiagnostic("warning", "WebSocket disconnected", context: [
            "error": error?.localizedDescription ?? "none",
            "wasConnected": wasConnected,
            "wasAuthenticated": wasAuthenticated
        ])
        
        // Attempt reconnection if enabled
        // Messages in outbox will be flushed on successful reconnection
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
            
        case "MessageSent":
            // Handle MessageSent event from WebSocket server
            // This contains the server-generated message_id that we should use
            if let messageId = json["message_id"] as? String,
               !messageId.isEmpty {
                let recipient = json["recipient"] as? String ?? ""
                let timestamp = json["timestamp"] as? String ?? ""
                
                // The server has confirmed the message was sent with this message_id
                // We need to notify the protocol so it can update the message ID
                // The protocol will emit a message_sent event with this server-generated ID
                emitDiagnostic("debug", "MessageSent from relay server", context: [
                    "messageId": messageId,
                    "recipient": recipient,
                    "timestamp": timestamp
                ])
                // Note: The protocol SDK will handle the message_sent event internally
                // The frontend will receive it via the normal event stream
            }
            
        case "MessageReceived":
            // Handle incoming direct message
            guard let senderId = json["sender"] as? String,
                  let content = json["content"] as? String else {
                emitDiagnostic("warning", "Invalid MessageReceived format")
                return
            }
            
            let replyToMsg = json["reply_to_msg"] as? String
            let messageId = json["message_id"] as? String
            
            messagesReceived += 1
            
            messageQueue.async { [weak self] in
                guard let self = self else { return }
                
                do {
                    // The protocol expects the full serialized Message JSON bytes
                    // The WebSocket server sends the message content, which should be the full Message JSON
                    // But we need to ensure reply_to_msg and message_id are included if present
                    var messageData: Data
                    var isFullMessage = false
                    
                    // Try to parse content as JSON to see if it's already a full Message
                    if let contentData = content.data(using: .utf8),
                       let contentJson = try? JSONSerialization.jsonObject(with: contentData) as? [String: Any],
                       contentJson["sender"] != nil && contentJson["recipient"] != nil {
                        // It's already a full Message JSON
                        isFullMessage = true
                        var messageDict = contentJson
                        // Ensure message_id is included if present in WebSocket event
                        if let messageId = messageId, !messageId.isEmpty {
                            if messageDict["id"] == nil && messageDict["message_id"] == nil {
                                messageDict["id"] = messageId
                            }
                        }
                        // Ensure reply_to_msg is included if present in WebSocket event
                        if let replyToMsg = replyToMsg, !replyToMsg.isEmpty, messageDict["reply_to_msg"] == nil {
                            messageDict["reply_to_msg"] = replyToMsg
                        }
                        messageData = try JSONSerialization.data(withJSONObject: messageDict)
                    } else {
                        // Content is plain text, reconstruct full Message JSON
                        var messageDict: [String: Any] = [
                            "sender": senderId,
                            "recipient": self.deviceId, // Will be corrected by protocol
                            "content": content,
                            "app_id": "offline-messenger",
                            "priority": "Medium",
                            "ttl": 8,
                            "hop_count": 0,
                            "requires_ack": true
                        ]
                        if let messageId = messageId, !messageId.isEmpty {
                            messageDict["id"] = messageId
                        }
                        if let replyToMsg = replyToMsg, !replyToMsg.isEmpty {
                            messageDict["reply_to_msg"] = replyToMsg
                        }
                        messageData = try JSONSerialization.data(withJSONObject: messageDict)
                    }
                    
                    let bytes = [UInt8](messageData)
                    try self.protocolInstance.internetMessageReceived(senderId: senderId, data: bytes)
                    
                    self.emitDiagnostic("debug", "Message received from relay", context: [
                        "senderId": senderId,
                        "messageId": messageId ?? "none",
                        "contentLength": content.count,
                        "hasReplyToMsg": replyToMsg != nil && !replyToMsg!.isEmpty,
                        "isFullMessage": isFullMessage
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
            
        case "ConnectionRequestReceived":
            // Forward connection request to JavaScript with full data
            let sender = json["sender"] as? String ?? ""
            let senderName = json["sender_name"] as? String ?? sender
            let timestamp = json["timestamp"] as? String ?? ""
            var requestContext: [String: Any] = [
                "type": messageType,
                "sender": sender,
                "sender_name": senderName,
                "timestamp": timestamp
            ]
            // Include key package if present
            if let keyPackage = json["key_package"] as? [Int] {
                requestContext["key_package"] = keyPackage
            }
            emitDiagnostic("debug", "Received relay message", context: requestContext)
            
        case "ConnectionAccepted":
            // Forward connection accepted to JavaScript with full data
            let acceptedBy = json["accepted_by"] as? String ?? json["sender"] as? String ?? ""
            let acceptedByName = json["accepted_by_name"] as? String ?? json["sender_name"] as? String ?? acceptedBy
            var acceptContext: [String: Any] = [
                "type": messageType,
                "accepted_by": acceptedBy,
                "accepted_by_name": acceptedByName
            ]
            // Include key package if present
            if let keyPackage = json["key_package"] as? [Int] {
                acceptContext["key_package"] = keyPackage
            }
            emitDiagnostic("debug", "Received relay message", context: acceptContext)
            
        case "ConnectionRejected":
            // Forward connection rejected to JavaScript with full data
            let rejectedBy = json["rejected_by"] as? String ?? json["sender"] as? String ?? ""
            emitDiagnostic("debug", "Received relay message", context: [
                "type": messageType,
                "rejected_by": rejectedBy
            ])
            
        case "ConnectionRequestError":
            // Forward connection request error to JavaScript with full data
            let recipient = json["recipient"] as? String ?? ""
            let reason = json["reason"] as? String ?? "Unknown error"
            emitDiagnostic("debug", "Received relay message", context: [
                "type": messageType,
                "recipient": recipient,
                "reason": reason
            ])
            
        default:
            emitDiagnostic("debug", "Received relay message", context: [
                "type": messageType
            ])
        }
    }
    
    private func startMessagePolling() {
        stopMessagePolling()
        
        let timer = DispatchSource.makeTimerSource(queue: messageQueue)
        timer.schedule(deadline: .now(), repeating: MESSAGE_POLL_INTERVAL)
        timer.setEventHandler { [weak self] in
            self?.pollAndSendMessages()
        }
        timer.resume()
        messageTimer = timer
    }
    
    private func stopMessagePolling() {
        messageTimer?.cancel()
        messageTimer = nil
    }
    
    private func pollAndSendMessages() {
        // Double-check connection state to handle race conditions
        // This prevents sending messages right after transport disconnect
        guard isConnected, isAuthenticated else {
            return
        }
        
        // Timer already runs on messageQueue, no need for extra dispatch
        // Poll for next message from protocol - batch send up to 10 messages per poll
        // to efficiently flush the outbox after reconnection
        var messagesSent = 0
        let maxBatchSize = 10
        
        while messagesSent < maxBatchSize {
            // Re-check connection state between messages to handle mid-batch disconnects
            guard isConnected, isAuthenticated else {
                emitDiagnostic("warning", "Connection lost mid-batch, stopping message send", context: [
                    "messagesSent": messagesSent
                ])
                break
            }
            
            guard let message = self.protocolInstance.internetGetNextMessage() else {
                break
            }
            
            self.sendMessage(recipientId: message.recipientId, data: Data(message.data))
            messagesSent += 1
        }
        
        if messagesSent > 1 {
            emitDiagnostic("debug", "Batch sent messages", context: [
                "count": messagesSent
            ])
        }
    }
    
    private func sendMessage(recipientId: String, data: Data) {
        // Re-check connection state right before sending
        // This handles race conditions where connection drops between poll and send
        guard isConnected, isAuthenticated, let task = webSocketTask else {
            emitDiagnostic("warning", "Cannot send message - not connected or not authenticated", context: [
                "recipientId": recipientId,
                "isConnected": isConnected,
                "isAuthenticated": isAuthenticated,
                "hasTask": webSocketTask != nil
            ])
            // The message remains in the protocol's outbox and will be retried
            // when connection is restored
            return
        }
        
        // Convert data to string content for the relay protocol
        let content = String(data: data, encoding: .utf8) ?? data.base64EncodedString()
        
        // Try to parse the message JSON to extract reply_to_msg if present
        var replyToMsg: String? = nil
        if let contentData = content.data(using: .utf8),
           let messageJson = try? JSONSerialization.jsonObject(with: contentData) as? [String: Any] {
            if let replyToMsgValue = messageJson["reply_to_msg"] {
                // reply_to_msg can be a string (MessageId as string) or an object
                if let replyToMsgString = replyToMsgValue as? String {
                    replyToMsg = replyToMsgString
                } else if let replyToMsgDict = replyToMsgValue as? [String: Any] {
                    // If it's an object, try to extract a string representation
                    // MessageId might be serialized as an object with nested fields
                    if let stringValue = replyToMsgDict.values.first as? String {
                        replyToMsg = stringValue
                    } else if let replyToMsgData = try? JSONSerialization.data(withJSONObject: replyToMsgDict),
                                let replyToMsgString = String(data: replyToMsgData, encoding: .utf8) {
                        replyToMsg = replyToMsgString
                    }
                }
            }
        }
        
        // Wrap in relay server protocol format
        var relayMessage: [String: Any] = [
            "type": "SendMessage",
            "recipient": recipientId,
            "content": content
        ]
        if let replyToMsg = replyToMsg {
            relayMessage["reply_to_msg"] = replyToMsg
        }
        
        guard let jsonData = try? JSONSerialization.data(withJSONObject: relayMessage),
              let jsonString = String(data: jsonData, encoding: .utf8) else {
            emitDiagnostic("error", "Failed to create relay message")
            return
        }
        
        task.send(.string(jsonString)) { [weak self] error in
            guard let self = self else { return }
            
            if let error = error {
                self.consecutiveSendFailures += 1
                self.emitDiagnostic("error", "Failed to send WebSocket message", context: [
                    "error": error.localizedDescription,
                    "recipientId": recipientId,
                    "consecutiveFailures": self.consecutiveSendFailures
                ])
                
                // If send fails, the message stays in outbox and will be retried
                // If too many consecutive send failures, the connection is likely dead
                // Trigger disconnect so DORS can switch to another transport
                if self.consecutiveSendFailures >= self.MAX_CONSECUTIVE_FAILURES {
                    self.emitDiagnostic("warning", "Too many consecutive send failures, triggering reconnect for DORS", context: [
                        "failures": self.consecutiveSendFailures
                    ])
                    DispatchQueue.main.async {
                        self.handleConnectionClosed(error: error)
                    }
                }
            } else {
                // Reset failure counter on successful send
                self.consecutiveSendFailures = 0
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
        
        let timer = DispatchSource.makeTimerSource(queue: messageQueue)
        timer.schedule(deadline: .now() + PING_INTERVAL, repeating: PING_INTERVAL)
        timer.setEventHandler { [weak self] in
            self?.sendPing()
        }
        timer.resume()
        pingTimer = timer
    }
    
    private func stopPingTimer() {
        pingTimer?.cancel()
        pingTimer = nil
    }
    
    private func sendPing() {
        webSocketTask?.sendPing { [weak self] error in
            guard let self = self else { return }
            
            if let error = error {
                self.consecutivePingFailures += 1
                self.emitDiagnostic("warning", "Ping failed", context: [
                    "error": error.localizedDescription,
                    "consecutiveFailures": self.consecutivePingFailures
                ])
                
                // If ping fails, the connection is likely dead
                // Trigger disconnect so DORS can switch to another transport
                if self.consecutivePingFailures >= self.MAX_CONSECUTIVE_FAILURES {
                    self.emitDiagnostic("warning", "Too many consecutive ping failures, triggering reconnect for DORS", context: [
                        "failures": self.consecutivePingFailures
                    ])
                    DispatchQueue.main.async {
                        self.handleConnectionClosed(error: error)
                    }
                }
            } else {
                // Reset failure counter on successful ping
                self.consecutivePingFailures = 0
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

