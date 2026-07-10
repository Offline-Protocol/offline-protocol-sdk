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
    private var authToken: String? = nil
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

    // Correlates the relay's recipient-keyed failure signals (DeliveryError /
    // ConnectionRequestError carry no message_id) back to in-flight sends.
    private let inFlightTracker = RecipientInFlightTracker()

    // Which peers to query via CheckPresence, and how many per tick.
    private let presenceWatch = PresenceWatchPolicy()
    private var presenceWatchTimer: DispatchSourceTimer?

    // Translates core-tagged server-plane control frames (controlOp on
    // InternetMessage) into relay-native ops.
    private lazy var controlOpTranslator = RelayControlOpTranslator(selfId: deviceId)

    /// Receives raw relay frames that are app/server concerns rather than SDK
    /// concerns (invite links, role changes, rate limiting, unknown types) —
    /// the module forwards them to JS as the `internet_server_message` event.
    public var serverMessageEmitter: ((String) -> Void)?
    
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
    
    /// Set the auth token for authentication
    /// If the WebSocket is already connected, this will trigger re-authentication
    public func setAuthToken(_ token: String?) {
        let wasAuthenticated = isAuthenticated
        self.authToken = token
        
        emitDiagnostic("info", "Auth token updated", context: [
            "hasToken": token != nil,
            "wasAuthenticated": wasAuthenticated
        ])
        
        // If already connected, (re-)authenticate with the latest token.
        // This ensures token rotations take effect immediately.
        if isConnected {
            sendAuthentication()
        }
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
        // Use auth token if available, otherwise fall back to deviceId
        let token = authToken ?? deviceId
        
        let authMessage: [String: Any] = [
            "type": "Authenticate",
            "token": token
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
        
        // Start polling, pinging, and the presence watch
        startMessagePolling()
        startPingTimer()
        startPresenceWatch()

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
        stopPresenceWatch()
        // Wire outcomes for anything in flight are now owned by the
        // transport layer (fail_all_pending on disconnect).
        inFlightTracker.clear()
        // Registration diffs are per-connection: a reconnect re-registers
        // groups from scratch (sync_groups_to_relay re-sends on the
        // internet 0→1 transition).
        controlOpTranslator.reset()
        
        // Always notify protocol of disconnection so DORS excludes Internet from
        // available transports and can switch to BLE (or WiFi Direct). Without this,
        // the core would keep Internet in the available set and keep selecting it.
        do {
            try protocolInstance.internetStatusChanged(isConnected: false)
        } catch {
            emitDiagnostic("error", "Failed to notify protocol of disconnection", context: [
                "error": error.localizedDescription
            ])
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

                    // Inbound traffic proves the peer reachable — stop
                    // presence-polling them (core re-arms via the
                    // internetMessageReceived → reachability path).
                    self.presenceWatch.unwatch(senderId)

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
            // The relay's authoritative "recipient offline" signal. It
            // arrives well before the SDK's confirm timeout, so fail-fast
            // every in-flight message to this recipient with the
            // recipient_unreachable reason (parks welcomes instead of
            // burning their retry budget) and start watching presence.
            let recipient = json["recipient"] as? String ?? ""
            let reason = json["reason"] as? String ?? "Unknown error"
            handleRecipientUnreachable(recipient: recipient, reason: reason, source: "DeliveryError")

        case "PresenceStatus", "PresenceStatusWithLastSeen":
            // Relay presence answer: feed core (re-arms parked welcomes and
            // flushes queues on online; parks pending welcomes on offline)
            // and maintain the watch set.
            guard let userId = json["user_id"] as? String, !userId.isEmpty,
                  let onlineNumber = json["online"] as? NSNumber,
                  CFGetTypeID(onlineNumber) == CFBooleanGetTypeID() else {
                emitDiagnostic("warning", "Invalid presence format: missing user_id/online", context: [:])
                return
            }
            let online = onlineNumber.boolValue
            // last_seen may arrive as an ISO-8601 string OR a numeric
            // epoch-ms (the Android bridge coerces both; keep parity).
            let lastSeenMs: Int64? = {
                if let str = json["last_seen"] as? String {
                    return parseTimestampToMsOrNull(str)
                }
                if let num = json["last_seen"] as? NSNumber,
                   CFGetTypeID(num) != CFBooleanGetTypeID() {
                    return num.int64Value
                }
                return nil
            }()
            if online {
                presenceWatch.unwatch(userId)
            }
            protocolInstance.internetPeerPresence(peerId: userId, online: online, lastSeenMs: lastSeenMs)
            emitDiagnostic("debug", "Presence update", context: [
                "userId": userId,
                "online": online,
                "lastSeenMs": lastSeenMs ?? "none"
            ])

        case "TypingUpdate":
            // Bridge the relay's server-mediated typing event (produced by
            // SetTyping/ClearTyping relay clients) into the SDK's __TYPING__
            // path, so apps receive the same typing_indicator_received event
            // regardless of which stack the sender uses.
            let typingUserId = json["user_id"] as? String ?? ""
            let conversationId = json["conversation_id"] as? String ?? ""
            // Strict "typing" check: if the relay renames, drops, or retypes
            // the field, that must surface as a diagnostic instead of silently
            // degrading every event to typing=false. The CFBoolean type check
            // rejects JSON numbers (NSNumber 0/1 would otherwise bridge to
            // Bool), matching the Android bridge's strictness.
            guard !typingUserId.isEmpty, !conversationId.isEmpty,
                  let typingNumber = json["typing"] as? NSNumber,
                  CFGetTypeID(typingNumber) == CFBooleanGetTypeID() else {
                emitDiagnostic("warning", "Invalid TypingUpdate format: missing user_id/conversation_id/typing", context: [:])
                return
            }
            let typing = typingNumber.boolValue
            let typingPayload: [String: Any] = [
                "conversation_id": conversationId,
                "is_typing": typing,
                "timestamp_ms": Int64(Date().timeIntervalSince1970 * 1000)
            ]
            injectGroupInternalMessage(senderId: typingUserId, prefix: "__TYPING__", payload: typingPayload)
            emitDiagnostic("debug", "Typing update bridged from relay", context: [
                "userId": typingUserId,
                "typing": typing
            ])

        case "ConnectionRequestReceived":
            // Process like MessageReceived: build internal message and feed to protocol so it emits connection_request_received
            let senderId = json["sender"] as? String ?? ""
            let senderName = json["sender_name"] as? String ?? senderId
            let timestampStr = json["timestamp"] as? String ?? ""
            let keyPackage = json["key_package"] as? [Int]
            guard !senderId.isEmpty else {
                emitDiagnostic("warning", "Invalid ConnectionRequestReceived format: missing sender", context: [:])
                return
            }
            let timestampMs = parseTimestampToMs(timestampStr)
            var payloadDict: [String: Any] = [
                "sender_name": senderName,
                "timestamp_ms": timestampMs
            ]
            if let kp = keyPackage {
                payloadDict["key_package"] = kp.map { $0 & 0xFF }
            }
            guard let payloadData = try? JSONSerialization.data(withJSONObject: payloadDict),
                  let payloadStr = String(data: payloadData, encoding: .utf8) else { return }
            let content = "__CONN_REQ__" + payloadStr
            messageQueue.async { [weak self] in
                guard let self = self else { return }
                do {
                    let messageData = self.buildInternalMessageData(senderId: senderId, content: content)
                    let bytes = [UInt8](messageData)
                    try self.protocolInstance.internetMessageReceived(senderId: senderId, data: bytes)
                    self.emitDiagnostic("debug", "Connection request received from relay", context: [
                        "sender": senderId,
                        "sender_name": senderName
                    ])
                } catch {
                    self.emitDiagnostic("error", "Error processing ConnectionRequestReceived", context: [
                        "error": error.localizedDescription
                    ])
                }
            }
            
        case "ConnectionAccepted":
            let acceptedBy = json["accepted_by"] as? String ?? json["sender"] as? String ?? ""
            let acceptedByName = json["accepted_by_name"] as? String ?? json["sender_name"] as? String ?? acceptedBy
            let timestampStr = json["timestamp"] as? String ?? ""
            let keyPackage = json["key_package"] as? [Int]
            guard !acceptedBy.isEmpty else {
                emitDiagnostic("warning", "Invalid ConnectionAccepted format: missing accepted_by", context: [:])
                return
            }
            let timestampMs = parseTimestampToMs(timestampStr)
            var payloadDict: [String: Any] = [
                "accepted_by_name": acceptedByName,
                "timestamp_ms": timestampMs
            ]
            if let kp = keyPackage {
                payloadDict["key_package"] = kp.map { $0 & 0xFF }
            }
            guard let payloadData = try? JSONSerialization.data(withJSONObject: payloadDict),
                  let payloadStr = String(data: payloadData, encoding: .utf8) else { return }
            let content = "__CONN_ACC__" + payloadStr
            messageQueue.async { [weak self] in
                guard let self = self else { return }
                do {
                    let messageData = self.buildInternalMessageData(senderId: acceptedBy, content: content)
                    let bytes = [UInt8](messageData)
                    try self.protocolInstance.internetMessageReceived(senderId: acceptedBy, data: bytes)
                    self.emitDiagnostic("debug", "Connection accepted from relay", context: [
                        "accepted_by": acceptedBy,
                        "accepted_by_name": acceptedByName
                    ])
                } catch {
                    self.emitDiagnostic("error", "Error processing ConnectionAccepted", context: [
                        "error": error.localizedDescription
                    ])
                }
            }
            
        case "ConnectionRejected":
            let rejectedBy = json["rejected_by"] as? String ?? json["sender"] as? String ?? ""
            guard !rejectedBy.isEmpty else {
                emitDiagnostic("warning", "Invalid ConnectionRejected format: missing rejected_by", context: [:])
                return
            }
            let content = "__CONN_REJ__"
            messageQueue.async { [weak self] in
                guard let self = self else { return }
                do {
                    let messageData = self.buildInternalMessageData(senderId: rejectedBy, content: content)
                    let bytes = [UInt8](messageData)
                    try self.protocolInstance.internetMessageReceived(senderId: rejectedBy, data: bytes)
                    self.emitDiagnostic("debug", "Connection rejected from relay", context: [
                        "rejected_by": rejectedBy
                    ])
                } catch {
                    self.emitDiagnostic("error", "Error processing ConnectionRejected", context: [
                        "error": error.localizedDescription
                    ])
                }
            }
            
        case "ConnectionRequestError":
            // Same authoritative offline signal as DeliveryError, for
            // relay-native connection-request ops (the relay does not store
            // requests for offline recipients).
            let recipient = json["recipient"] as? String ?? ""
            let reason = json["reason"] as? String ?? "Unknown error"
            handleRecipientUnreachable(recipient: recipient, reason: reason, source: "ConnectionRequestError")
            
        case "GroupCreated":
            guard let groupId = json["group_id"] as? String, !groupId.isEmpty else { return }
            let name = json["name"] as? String ?? ""
            injectGroupInternalMessage(senderId: "relay", prefix: "__GROUP_CREATED__", payload: ["group_id": groupId, "name": name])
            
        case "GroupMessageReceived":
            guard let groupId = json["group_id"] as? String,
                  let messageId = json["message_id"] as? String, !groupId.isEmpty, !messageId.isEmpty else { return }
            let sender = json["sender"] as? String ?? ""
            let content = json["content"] as? String ?? ""
            let timestamp = json["timestamp"] as? String ?? ""
            let replyToMsg = json["reply_to_msg"] as? String
            var payload: [String: Any] = ["group_id": groupId, "sender": sender, "content": content, "timestamp": timestamp, "message_id": messageId]
            if let r = replyToMsg, !r.isEmpty { payload["reply_to_msg"] = r }
            injectGroupInternalMessage(senderId: sender.isEmpty ? "relay" : sender, prefix: "__GROUP_MSG__", payload: payload)
            
        case "GroupMemberAdded":
            guard let groupId = json["group_id"] as? String, !groupId.isEmpty else { return }
            let userId = json["user_id"] as? String ?? ""
            let addedBy = json["added_by"] as? String ?? ""
            injectGroupInternalMessage(senderId: addedBy.isEmpty ? "relay" : addedBy, prefix: "__GROUP_MEMBER_ADDED__", payload: ["group_id": groupId, "user_id": userId, "added_by": addedBy])
            
        case "GroupMemberRemoved":
            guard let groupId = json["group_id"] as? String, !groupId.isEmpty else { return }
            let userId = json["user_id"] as? String ?? ""
            let removedBy = json["removed_by"] as? String ?? ""
            injectGroupInternalMessage(senderId: removedBy.isEmpty ? "relay" : removedBy, prefix: "__GROUP_MEMBER_REMOVED__", payload: ["group_id": groupId, "user_id": userId, "removed_by": removedBy])
            
        case "GroupInfo":
            guard let groupId = json["group_id"] as? String, !groupId.isEmpty else { return }
            let name = json["name"] as? String ?? ""
            let createdBy = json["created_by"] as? String ?? ""
            let createdAt = json["created_at"] as? String ?? ""
            let membersRaw = json["members"] as? [[String: Any]] ?? []
            let members = membersRaw.map { m in
                ["user_id": m["user_id"] as? String ?? "", "role": m["role"] as? String ?? "member", "joined_at": m["joined_at"] as? String ?? ""]
            }
            injectGroupInternalMessage(senderId: "relay", prefix: "__GROUP_INFO__", payload: ["group_id": groupId, "name": name, "created_by": createdBy, "created_at": createdAt, "members": members])
            
        case "UserGroups":
            guard let groupsRaw = json["groups"] as? [[String: Any]] else { return }
            let groups = groupsRaw.map { g in
                ["group_id": g["group_id"] as? String ?? "", "name": g["name"] as? String ?? "", "created_at": g["created_at"] as? String ?? ""]
            }
            injectGroupInternalMessage(senderId: "relay", prefix: "__USER_GROUPS__", payload: ["groups": groups])
            
        case "GroupError":
            let reason = json["reason"] as? String ?? "Unknown error"
            let groupId = json["group_id"] as? String ?? ""
            // Admin-denied registration must stop member-delta attempts.
            controlOpTranslator.onGroupError(groupId: groupId, reason: reason)
            var payload: [String: Any] = ["reason": reason]
            // group_id lets the core revoke relay_synced so group sends fall
            // back to per-member delivery.
            if !groupId.isEmpty {
                payload["group_id"] = groupId
            }
            injectGroupInternalMessage(senderId: "relay", prefix: "__GROUP_ERROR__", payload: payload)
            // Dual-emit: apps correlating request_id-carrying errors
            // (invite-link ops ride the raw channel) need the full frame.
            emitServerMessage(json)

        // Server-plane frames that are app concerns, not SDK concerns —
        // forwarded verbatim as the internet_server_message event so the
        // invite-link lifecycle and misc server events can ride the SDK's
        // socket without a second WebSocket in the app.
        case "GroupInviteLinkCreated", "GroupInviteLinkRevoked", "GroupJoinedViaInvite",
             "GroupInviteJoinPending", "GroupRoleChanged", "GroupDeleted", "RateLimited":
            emitServerMessage(json)
            emitDiagnostic("debug", "Relay server message forwarded", context: [
                "type": messageType
            ])

        default:
            // Unknown types are forwarded too — future relay additions
            // surface to the app instead of being silently dropped.
            emitServerMessage(json)
            emitDiagnostic("debug", "Received relay message", context: [
                "type": messageType
            ])
        }
    }

    private func emitServerMessage(_ json: [String: Any]) {
        guard let emitter = serverMessageEmitter,
              let data = try? JSONSerialization.data(withJSONObject: json),
              let raw = String(data: data, encoding: .utf8) else { return }
        emitter(raw)
    }
    
    /// Like `parseTimestampToMs` but returns nil instead of now() when the
    /// value is absent or unparseable — a last-seen display must not invent
    /// a timestamp. Accepts ISO-8601 or epoch milliseconds.
    private func parseTimestampToMsOrNull(_ timestampStr: String) -> Int64? {
        guard !timestampStr.isEmpty else { return nil }
        if let epochMs = Int64(timestampStr) {
            return epochMs
        }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: timestampStr) {
            return Int64(date.timeIntervalSince1970 * 1000)
        }
        formatter.formatOptions = [.withInternetDateTime]
        if let date = formatter.date(from: timestampStr) {
            return Int64(date.timeIntervalSince1970 * 1000)
        }
        return nil
    }

    /// Parse ISO-8601 timestamp string to Unix ms, or return current time if invalid.
    private func parseTimestampToMs(_ timestampStr: String) -> Int64 {
        guard !timestampStr.isEmpty else {
            return Int64(Date().timeIntervalSince1970 * 1000)
        }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: timestampStr) {
            return Int64(date.timeIntervalSince1970 * 1000)
        }
        formatter.formatOptions = [.withInternetDateTime]
        if let date = formatter.date(from: timestampStr) {
            return Int64(date.timeIntervalSince1970 * 1000)
        }
        return Int64(Date().timeIntervalSince1970 * 1000)
    }
    
    /// Build serialized Message JSON data for an internal (relay) message, same shape as MessageReceived.
    private func buildInternalMessageData(senderId: String, content: String) -> Data {
        let messageDict: [String: Any] = [
            "id": UUID().uuidString,
            "sender": senderId,
            "recipient": deviceId,
            "content": content,
            "app_id": "offline-messenger",
            "priority": "Medium",
            "ttl": 8,
            "hop_count": 0,
            "requires_ack": true,
            "timestamp": Int64(Date().timeIntervalSince1970 * 1000)
        ]
        return try! JSONSerialization.data(withJSONObject: messageDict)
    }
    
    /// Inject a group (relay) internal message into the protocol so it emits the corresponding event.
    private func injectGroupInternalMessage(senderId: String, prefix: String, payload: [String: Any]) {
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            do {
                let payloadData = try JSONSerialization.data(withJSONObject: payload)
                guard let payloadStr = String(data: payloadData, encoding: .utf8) else { return }
                let content = prefix + payloadStr
                let messageData = self.buildInternalMessageData(senderId: senderId, content: content)
                let bytes = [UInt8](messageData)
                try self.protocolInstance.internetMessageReceived(senderId: senderId, data: bytes)
            } catch {
                self.emitDiagnostic("error", "Error injecting group message", context: [
                    "prefix": prefix,
                    "error": error.localizedDescription
                ])
            }
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

        inFlightTracker.prune(nowMs: Int64(Date().timeIntervalSince1970 * 1000))

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

            if let controlOp = message.controlOp {
                self.sendControlOp(
                    messageId: message.messageId,
                    recipientId: message.recipientId,
                    controlOp: controlOp,
                    controlPayload: message.controlPayload ?? "",
                    data: Data(message.data)
                )
            } else {
                self.sendMessage(messageId: message.messageId, recipientId: message.recipientId, data: Data(message.data))
            }
            messagesSent += 1
        }
        
        if messagesSent > 1 {
            emitDiagnostic("debug", "Batch sent messages", context: [
                "count": messagesSent
            ])
        }
    }
    
    private func sendMessage(messageId: String, recipientId: String, data: Data) {
        // Re-check connection state right before sending
        // This handles race conditions where connection drops between poll and send
        guard isConnected, isAuthenticated, let task = webSocketTask else {
            emitDiagnostic("warning", "Cannot send message - not connected or not authenticated", context: [
                "messageId": messageId,
                "recipientId": recipientId,
                "isConnected": isConnected,
                "isAuthenticated": isAuthenticated,
                "hasTask": webSocketTask != nil
            ])
            // Report failure so DORS metrics stay accurate
            protocolInstance.internetSendFailed(messageId: messageId)
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
            protocolInstance.internetSendFailed(messageId: messageId)
            return
        }
        
        task.send(.string(jsonString)) { [weak self] error in
            guard let self = self else { return }
            
            if let error = error {
                self.consecutiveSendFailures += 1
                self.protocolInstance.internetSendFailed(messageId: messageId)
                self.emitDiagnostic("error", "Failed to send WebSocket message", context: [
                    "error": error.localizedDescription,
                    "messageId": messageId,
                    "recipientId": recipientId,
                    "consecutiveFailures": self.consecutiveSendFailures
                ])
                
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
                // Track for recipient-keyed failure correlation: a later
                // DeliveryError for this recipient fails-fast this message id.
                self.inFlightTracker.recordSent(
                    recipient: recipientId,
                    messageId: messageId,
                    nowMs: Int64(Date().timeIntervalSince1970 * 1000)
                )
                self.protocolInstance.internetConfirmSent(messageId: messageId)
                
                self.emitDiagnostic("debug", "Message sent via relay", context: [
                    "messageId": messageId,
                    "recipientId": recipientId,
                    "contentLength": content.count
                ])
            }
        }
    }
    
    // MARK: - Control-Op Translation

    /// Sends a core-tagged server-plane control frame via the relay-native
    /// protocol (see RelayControlOpTranslator). Wire-outcome contract is the
    /// same as sendMessage: the original message id is confirmed on the
    /// primary frame's socket-write success, failed otherwise.
    private func sendControlOp(
        messageId: String,
        recipientId: String,
        controlOp: String,
        controlPayload: String,
        data: Data
    ) {
        let translation = controlOpTranslator.translate(
            controlOp: controlOp,
            controlPayload: controlPayload,
            recipientId: recipientId
        )
        switch translation {
        case .passThrough:
            sendMessage(messageId: messageId, recipientId: recipientId, data: data)

        case .tap(let frames, let commit):
            // Verbatim delivery owns the message id outcome; the extra
            // relay-native frames are best-effort. The translator's state
            // commits only once every extra frame was written — a dropped
            // frame must be re-sent by a later translation, not assumed
            // applied.
            sendMessage(messageId: messageId, recipientId: recipientId, data: data)
            sendRelayFramesBestEffort(frames, controlOp: controlOp, onAllSent: commit)

        case .replace(let frames, let commit):
            guard isConnected, isAuthenticated, let task = webSocketTask else {
                protocolInstance.internetSendFailed(messageId: messageId)
                return
            }
            guard let primary = frames.first else {
                // Nothing to send (fully deduped) — the intent is already
                // reflected server-side; confirm so the core moves on.
                commit?()
                protocolInstance.internetConfirmSent(messageId: messageId)
                return
            }
            guard let primaryData = try? JSONSerialization.data(withJSONObject: primary),
                  let primaryJson = String(data: primaryData, encoding: .utf8) else {
                // A non-empty frame that cannot serialize is a failure, not
                // a dedup: never confirm a message nothing was written for.
                protocolInstance.internetSendFailed(messageId: messageId)
                emitDiagnostic("error", "Unserializable relay-native control op", context: [
                    "controlOp": controlOp,
                    "messageId": messageId
                ])
                return
            }
            task.send(.string(primaryJson)) { [weak self] error in
                guard let self = self else { return }
                if let error = error {
                    self.consecutiveSendFailures += 1
                    self.protocolInstance.internetSendFailed(messageId: messageId)
                    self.emitDiagnostic("error", "Failed to send relay-native control op", context: [
                        "controlOp": controlOp,
                        "messageId": messageId,
                        "error": error.localizedDescription,
                        "consecutiveFailures": self.consecutiveSendFailures
                    ])
                    if self.consecutiveSendFailures >= self.MAX_CONSECUTIVE_FAILURES {
                        DispatchQueue.main.async {
                            self.handleConnectionClosed(error: error)
                        }
                    }
                } else {
                    self.consecutiveSendFailures = 0
                    self.bytesSent += UInt64(primaryData.count)
                    self.messagesSent += 1
                    self.inFlightTracker.recordSent(
                        recipient: recipientId,
                        messageId: messageId,
                        nowMs: Int64(Date().timeIntervalSince1970 * 1000)
                    )
                    self.protocolInstance.internetConfirmSent(messageId: messageId)
                    self.sendRelayFramesBestEffort(
                        Array(frames.dropFirst()),
                        controlOp: controlOp,
                        onAllSent: commit
                    )
                    self.emitDiagnostic("debug", "Control op sent relay-native", context: [
                        "controlOp": controlOp,
                        "messageId": messageId,
                        "frames": frames.count
                    ])
                }
            }
        }
    }

    /// Sends the frames sequentially; `onAllSent` runs only when every frame
    /// was written (the translator's commit hook — see
    /// `RelayControlOpTranslator.Translation`). A dropped or unserializable
    /// frame aborts the chain WITHOUT committing, so the next translation
    /// re-sends the missing state instead of assuming it applied.
    private func sendRelayFramesBestEffort(
        _ frames: [[String: Any]],
        controlOp: String,
        onAllSent: (() -> Void)? = nil
    ) {
        guard !frames.isEmpty else {
            onAllSent?()
            return
        }
        guard isConnected, isAuthenticated, let task = webSocketTask else { return }
        var remaining = frames
        let frame = remaining.removeFirst()
        guard let frameData = try? JSONSerialization.data(withJSONObject: frame),
              let frameJson = String(data: frameData, encoding: .utf8) else {
            emitDiagnostic("warning", "Best-effort relay frame unserializable", context: [
                "controlOp": controlOp,
                "frameType": frame["type"] as? String ?? "unknown"
            ])
            return
        }
        task.send(.string(frameJson)) { [weak self] error in
            guard let self = self else { return }
            if let error = error {
                self.emitDiagnostic("warning", "Best-effort relay frame dropped", context: [
                    "controlOp": controlOp,
                    "frameType": frame["type"] as? String ?? "unknown",
                    "error": error.localizedDescription
                ])
                return
            }
            self.sendRelayFramesBestEffort(remaining, controlOp: controlOp, onAllSent: onAllSent)
        }
    }

    /// Sends a raw, caller-built relay command verbatim (RN
    /// `internetSendRawCommand`). The JSON must parse; returns false when
    /// invalid or not connected+authenticated. Responses the SDK doesn't
    /// consume arrive as `internet_server_message` events.
    public func sendRawCommand(json: String) -> Bool {
        guard isConnected, isAuthenticated, let task = webSocketTask else { return false }
        guard let data = json.data(using: .utf8),
              (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] != nil else {
            emitDiagnostic("warning", "Rejected invalid raw server command", context: [:])
            return false
        }
        task.send(.string(json)) { _ in }
        return true
    }

    // MARK: - Presence Watch

    /// Fail-fast handler for the relay's recipient-keyed offline signals
    /// (DeliveryError / ConnectionRequestError). Fails every live in-flight
    /// message to the recipient with the recipient_unreachable reason (the
    /// core classifies it as per-peer no-carrier and parks welcomes without
    /// burning budget), ingests an authoritative offline presence, and adds
    /// the recipient to the presence watch set.
    private func handleRecipientUnreachable(recipient: String, reason: String, source: String) {
        guard !recipient.isEmpty else {
            emitDiagnostic("warning", "Recipient-unreachable signal without recipient", context: [
                "source": source,
                "reason": reason
            ])
            return
        }
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let failedIds = inFlightTracker.drainRecipient(recipient, nowMs: now)
        for id in failedIds {
            protocolInstance.internetSendFailedWithReason(
                messageId: id,
                reason: "recipient_unreachable: \(reason)"
            )
        }
        presenceWatch.watch(recipient, nowMs: now)
        protocolInstance.internetPeerPresence(peerId: recipient, online: false, lastSeenMs: nil)
        emitDiagnostic("warning", "Recipient unreachable", context: [
            "recipient": recipient,
            "reason": reason,
            "source": source,
            "failedInFlight": failedIds.count
        ])
    }

    private func startPresenceWatch() {
        stopPresenceWatch()

        let timer = DispatchSource.makeTimerSource(queue: messageQueue)
        timer.schedule(
            deadline: .now() + PresenceWatchPolicy.defaultTickInterval,
            repeating: PresenceWatchPolicy.defaultTickInterval
        )
        timer.setEventHandler { [weak self] in
            self?.presenceWatchTick()
        }
        timer.resume()
        presenceWatchTimer = timer
    }

    private func stopPresenceWatch() {
        presenceWatchTimer?.cancel()
        presenceWatchTimer = nil
    }

    private func presenceWatchTick() {
        guard isConnected, isAuthenticated, let task = webSocketTask else { return }

        let coreWatchlist = protocolInstance.internetPresenceWatchlist()
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let peers = presenceWatch.peersToQuery(coreWatchlist: coreWatchlist, nowMs: now)
            .filter { $0 != deviceId }
        guard !peers.isEmpty else { return }

        for peer in peers {
            let checkMessage: [String: Any] = [
                "type": "CheckPresence",
                "username": peer
            ]
            guard let jsonData = try? JSONSerialization.data(withJSONObject: checkMessage),
                  let jsonString = String(data: jsonData, encoding: .utf8) else { continue }
            task.send(.string(jsonString)) { _ in }
        }
        emitDiagnostic("debug", "Presence watch tick", context: [
            "queried": peers.count,
            "watched": presenceWatch.watchedPeers().count
        ])
    }

    /// App-facing one-shot presence query (RN `checkInternetPresence`). The
    /// answer arrives as the SDK's `presence_updated` event — fire-and-event,
    /// matching relay semantics. Returns true if the query was written to
    /// the socket.
    public func checkPresence(userId: String) -> Bool {
        guard !userId.isEmpty, isConnected, isAuthenticated, let task = webSocketTask else {
            return false
        }
        let checkMessage: [String: Any] = [
            "type": "CheckPresence",
            "username": userId
        ]
        guard let jsonData = try? JSONSerialization.data(withJSONObject: checkMessage),
              let jsonString = String(data: jsonData, encoding: .utf8) else {
            return false
        }
        task.send(.string(jsonString)) { _ in }
        return true
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

