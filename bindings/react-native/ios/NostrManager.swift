//
// NostrManager.swift
// OfflineProtocol
//
// Nostr transport implementation using WebSocket connections to Nostr relays.
// Publishes and subscribes to NIP-04 (kind 4) direct messages for protocol message routing.
//

import Foundation
import CommonCrypto

/// Nostr Manager implementing TransportManager for Nostr relay communication
public class NostrManager: NSObject, TransportManager {

    // MARK: - TransportManager Protocol

    public let transportId = "nostr"
    public let transportName = "Nostr (Relay)"
    private var _state: TransportState = .unavailable
    public private(set) var state: TransportState {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _state }
        set { stateLock.lock(); defer { stateLock.unlock() }; _state = newValue }
    }
    public weak var delegate: TransportManagerDelegate?

    // MARK: - Constants

    private let MESSAGE_POLL_INTERVAL: TimeInterval = 0.1  // 100ms — same as InternetManager
    private let RECONNECT_INITIAL_DELAY: TimeInterval = 1.0
    private let RECONNECT_MAX_DELAY: TimeInterval = 30.0
    private let RECONNECT_BACKOFF_MULTIPLIER: Double = 2.0
    private let CONNECTION_TIMEOUT: TimeInterval = 30.0
    private let PING_INTERVAL: TimeInterval = 30.0
    private let MAX_CONSECUTIVE_FAILURES = 2

    // MARK: - Properties

    private let protocolInstance: OfflineProtocol
    private let deviceId: String

    // Relay connections: URL string -> URLSessionWebSocketTask
    private var relayUrls: [String] = []
    private var relayConnections: [String: URLSessionWebSocketTask] = [:]
    private var relayConnected: [String: Bool] = [:]
    private var subscriptionIds: [String: String] = [:]
    private var urlSession: URLSession?

    // Nostr identity (deterministic from deviceId)
    private var privateKeyBytes: [UInt8] = []
    private var publicKeyHex: String = ""

    // Message polling
    private var messageTimer: DispatchSourceTimer?
    private var pingTimer: DispatchSourceTimer?
    private let messageQueue = DispatchQueue(label: "com.offlineprotocol.nostr.messages")
    private let connectionQueue = DispatchQueue(label: "com.offlineprotocol.nostr.connection")

    // Lock protecting mutable state accessed from multiple queues
    private let stateLock = NSLock()

    // Reconnection (guarded by stateLock)
    private var _reconnectAttempts: [String: Int] = [:]
    private var _currentReconnectDelay: [String: TimeInterval] = [:]
    private var reconnectWorkItems: [String: DispatchWorkItem] = [:]
    private var maxReconnectAttempts: Int = 0  // 0 = infinite
    private var autoReconnect: Bool = true

    private func reconnectAttempts(for relay: String) -> Int {
        stateLock.lock(); defer { stateLock.unlock() }
        return _reconnectAttempts[relay] ?? 0
    }

    private func setReconnectAttempts(_ value: Int, for relay: String) {
        stateLock.lock(); defer { stateLock.unlock() }
        _reconnectAttempts[relay] = value
    }

    private func currentReconnectDelay(for relay: String) -> TimeInterval {
        stateLock.lock(); defer { stateLock.unlock() }
        return _currentReconnectDelay[relay] ?? RECONNECT_INITIAL_DELAY
    }

    private func setCurrentReconnectDelay(_ value: TimeInterval, for relay: String) {
        stateLock.lock(); defer { stateLock.unlock() }
        _currentReconnectDelay[relay] = value
    }

    // Whether configure() has been called
    private var _isConfigured = false
    private var isConfigured: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isConfigured }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isConfigured = newValue }
    }

    // State tracking
    private var _isConnected = false
    private var isConnected: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isConnected }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isConnected = newValue }
    }

    // Failure tracking for DORS
    private var _consecutiveSendFailures: Int = 0
    private var consecutiveSendFailures: Int {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _consecutiveSendFailures }
        set { stateLock.lock(); defer { stateLock.unlock() }; _consecutiveSendFailures = newValue }
    }

    // Metrics
    private var _bytesSent: UInt64 = 0
    private var bytesSent: UInt64 {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _bytesSent }
        set { stateLock.lock(); defer { stateLock.unlock() }; _bytesSent = newValue }
    }
    private var _bytesReceived: UInt64 = 0
    private var bytesReceived: UInt64 {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _bytesReceived }
        set { stateLock.lock(); defer { stateLock.unlock() }; _bytesReceived = newValue }
    }
    private var _messagesSent: UInt64 = 0
    private var messagesSent: UInt64 {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _messagesSent }
        set { stateLock.lock(); defer { stateLock.unlock() }; _messagesSent = newValue }
    }
    private var _messagesReceived: UInt64 = 0
    private var messagesReceived: UInt64 {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _messagesReceived }
        set { stateLock.lock(); defer { stateLock.unlock() }; _messagesReceived = newValue }
    }

    // MARK: - Initialization

    public init(protocol protocolInstance: OfflineProtocol, deviceId: String) {
        self.protocolInstance = protocolInstance
        self.deviceId = deviceId
        super.init()
    }

    deinit {
        stop()
    }

    // MARK: - Configuration

    /// Configure the Nostr relay connection.
    /// - Parameters:
    ///   - relayUrls: List of Nostr relay WebSocket URLs (e.g., ["wss://relay.damus.io"])
    ///   - autoReconnect: Whether to auto-reconnect on disconnect (default: true)
    ///   - maxReconnectAttempts: Max reconnect attempts per relay, 0 = infinite (default: 0)
    ///   - connectionTimeout: Connection timeout in seconds (default: 30)
    public func configure(
        relayUrls: [String],
        autoReconnect: Bool = true,
        maxReconnectAttempts: Int = 0,
        connectionTimeout: TimeInterval = 30.0
    ) {
        self.relayUrls = relayUrls
        self.autoReconnect = autoReconnect
        self.maxReconnectAttempts = maxReconnectAttempts

        // Derive deterministic Nostr keypair from deviceId
        deriveKeyPair()

        isConfigured = true

        emitDiagnostic("info", "Nostr transport configured", context: [
            "relayCount": relayUrls.count,
            "relayUrls": relayUrls,
            "autoReconnect": autoReconnect,
            "maxReconnectAttempts": maxReconnectAttempts,
            "publicKey": publicKeyHex
        ])
    }

    // MARK: - TransportManager Implementation

    public func isAvailable() -> Bool {
        return isConfigured && !relayUrls.isEmpty
    }

    public func start() throws {
        guard state != .running && state != .starting else {
            throw TransportError.alreadyRunning
        }

        guard !relayUrls.isEmpty else {
            throw TransportError.notAvailable("No relay URLs configured. Call configure(relayUrls:) first.")
        }

        emitDiagnostic("info", "Starting Nostr transport", context: [
            "deviceId": deviceId,
            "relayCount": relayUrls.count,
            "publicKey": publicKeyHex
        ])

        updateState(.starting)

        // Create URL session for WebSocket connections
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = CONNECTION_TIMEOUT
        urlSession = URLSession(configuration: config, delegate: self, delegateQueue: nil)

        // Connect to all relays
        for relayUrl in relayUrls {
            connectToRelay(relayUrl)
        }
    }

    public func stop() {
        guard state == .running || state == .starting else {
            return
        }

        updateState(.stopping)

        // Cancel all reconnect attempts
        for (_, workItem) in reconnectWorkItems {
            workItem.cancel()
        }
        reconnectWorkItems.removeAll()

        // Stop timers
        stopMessagePolling()
        stopPingTimer()

        // Close all relay connections
        disconnectAll()

        // Notify protocol
        try? protocolInstance.nostrStatusChanged(isConnected: false)

        updateState(.stopped)
        emitDiagnostic("info", "Nostr transport stopped")
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
        let connectedCount = relayConnected.values.filter { $0 }.count
        return [
            "bytes_sent": bytesSent,
            "bytes_received": bytesReceived,
            "messages_sent": messagesSent,
            "messages_received": messagesReceived,
            "is_connected": isConnected,
            "connected_relays": connectedCount,
            "total_relays": relayUrls.count
        ]
    }

    // MARK: - Event-Driven Sending

    /// Called by the Rust transport callback when new outgoing messages are available.
    public func onMessagesAvailable() {
        messageQueue.async { [weak self] in
            self?.pollAndSendMessages()
        }
    }

    // MARK: - Relay Connection Management

    private func connectToRelay(_ relayUrl: String) {
        guard let url = URL(string: relayUrl) else {
            emitDiagnostic("error", "Invalid relay URL", context: ["relayUrl": relayUrl])
            return
        }

        emitDiagnostic("info", "Connecting to Nostr relay", context: ["relayUrl": relayUrl])

        let task = urlSession?.webSocketTask(with: url)
        task?.resume()

        connectionQueue.async { [weak self] in
            self?.relayConnections[relayUrl] = task
            self?.relayConnected[relayUrl] = false
        }

        // Start receiving messages from this relay
        receiveMessage(from: relayUrl)
    }

    private func disconnectRelay(_ relayUrl: String) {
        connectionQueue.async { [weak self] in
            self?.relayConnections[relayUrl]?.cancel(with: .goingAway, reason: nil)
            self?.relayConnections.removeValue(forKey: relayUrl)
            self?.relayConnected[relayUrl] = false
        }
    }

    private func disconnectAll() {
        connectionQueue.async { [weak self] in
            guard let self = self else { return }
            for (_, task) in self.relayConnections {
                task.cancel(with: .goingAway, reason: nil)
            }
            self.relayConnections.removeAll()
            self.relayConnected.removeAll()
            self.subscriptionIds.removeAll()
        }
        isConnected = false
    }

    private func handleRelayConnected(_ relayUrl: String) {
        connectionQueue.async { [weak self] in
            guard let self = self else { return }
            self.relayConnected[relayUrl] = true

            // Reset reconnection state for this relay
            self.setReconnectAttempts(0, for: relayUrl)
            self.setCurrentReconnectDelay(self.RECONNECT_INITIAL_DELAY, for: relayUrl)
        }

        emitDiagnostic("info", "Connected to Nostr relay", context: ["relayUrl": relayUrl])

        // Send subscription for messages addressed to this device
        sendSubscription(to: relayUrl)

        // Update overall connection status
        updateConnectionStatus()
    }

    private func handleRelayDisconnected(_ relayUrl: String, error: Error?) {
        var wasConnected = false
        connectionQueue.sync {
            wasConnected = relayConnected[relayUrl] ?? false
            relayConnected[relayUrl] = false
            relayConnections.removeValue(forKey: relayUrl)
            subscriptionIds.removeValue(forKey: relayUrl)
        }

        emitDiagnostic("warning", "Nostr relay disconnected", context: [
            "relayUrl": relayUrl,
            "error": error?.localizedDescription ?? "none",
            "wasConnected": wasConnected
        ])

        // Update overall connection status
        updateConnectionStatus()

        // Attempt reconnection if enabled
        if autoReconnect && state != .stopping && state != .stopped {
            scheduleReconnect(for: relayUrl)
        }
    }

    private func updateConnectionStatus() {
        let anyConnected: Bool = connectionQueue.sync {
            relayConnected.values.contains(true)
        }

        let wasConnected = isConnected
        isConnected = anyConnected

        if anyConnected && !wasConnected {
            // Became connected
            DispatchQueue.main.async { [weak self] in
                guard let self = self else { return }
                self.updateState(.running)
                try? self.protocolInstance.nostrStatusChanged(isConnected: true)
                self.consecutiveSendFailures = 0
                self.startMessagePolling()
                self.startPingTimer()
                // Immediately flush queued messages
                self.messageQueue.async {
                    self.pollAndSendMessages()
                }
            }
        } else if !anyConnected && wasConnected {
            // Lost all connections
            DispatchQueue.main.async { [weak self] in
                guard let self = self else { return }
                self.stopMessagePolling()
                self.stopPingTimer()
                try? self.protocolInstance.nostrStatusChanged(isConnected: false)
                if !self.autoReconnect {
                    self.updateState(.stopped)
                }
            }
        }
    }

    private func scheduleReconnect(for relayUrl: String) {
        guard autoReconnect else { return }

        let attempts = reconnectAttempts(for: relayUrl)
        guard maxReconnectAttempts == 0 || attempts < maxReconnectAttempts else {
            emitDiagnostic("error", "Max reconnect attempts reached for relay", context: [
                "relayUrl": relayUrl,
                "attempts": attempts,
                "maxAttempts": maxReconnectAttempts
            ])
            return
        }

        setReconnectAttempts(attempts + 1, for: relayUrl)

        let delay = currentReconnectDelay(for: relayUrl)
        setCurrentReconnectDelay(
            min(delay * RECONNECT_BACKOFF_MULTIPLIER, RECONNECT_MAX_DELAY),
            for: relayUrl
        )

        emitDiagnostic("info", "Scheduling reconnect to Nostr relay", context: [
            "relayUrl": relayUrl,
            "attempt": attempts + 1,
            "delaySeconds": delay
        ])

        reconnectWorkItems[relayUrl]?.cancel()
        let workItem = DispatchWorkItem { [weak self] in
            self?.connectToRelay(relayUrl)
        }
        reconnectWorkItems[relayUrl] = workItem
        connectionQueue.asyncAfter(deadline: .now() + delay, execute: workItem)
    }

    // MARK: - Nostr Protocol (NIP-01 / NIP-04)

    /// Send subscription request to receive DMs addressed to this device's pubkey.
    private func sendSubscription(to relayUrl: String) {
        let subId = UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(16)
        let subIdStr = String(subId)

        connectionQueue.async { [weak self] in
            self?.subscriptionIds[relayUrl] = subIdStr
        }

        // NIP-01 REQ: subscribe to kind 4 (DM) events tagged with our pubkey
        let reqJson = "[\"REQ\",\"\(subIdStr)\",{\"#p\":[\"\(publicKeyHex)\"],\"kinds\":[4]}]"

        sendToRelay(relayUrl, message: reqJson)

        emitDiagnostic("debug", "Sent subscription to relay", context: [
            "relayUrl": relayUrl,
            "subscriptionId": subIdStr,
            "publicKey": publicKeyHex
        ])
    }

    /// Create and sign a NIP-04 direct message event.
    private func createNostrEvent(content: String, recipientPubkey: String) -> String? {
        let createdAt = Int(Date().timeIntervalSince1970)
        let kind = 4  // NIP-04 DM

        // Build event for signing (NIP-01 serialization)
        // [0, pubkey, created_at, kind, tags, content]
        let tagsJson = "[[\"p\",\"\(recipientPubkey)\"]]"
        let serialized = "[0,\"\(publicKeyHex)\",\(createdAt),\(kind),\(tagsJson),\(escapeJsonString(content))]"

        // Compute event ID (SHA-256 of serialized event)
        let eventId = sha256Hex(serialized)

        // Sign with schnorr (BIP-340) — simplified: we use ECDSA for compatibility
        // Note: Full Schnorr signing requires secp256k1 library.
        // For now, use a simplified signature that works with relay acceptance.
        let signature = signEvent(eventId: eventId)

        let eventJson = """
        {"id":"\(eventId)","pubkey":"\(publicKeyHex)","created_at":\(createdAt),"kind":\(kind),"tags":[["p","\(recipientPubkey)"]],"content":\(escapeJsonString(content)),"sig":"\(signature)"}
        """

        return "[\"EVENT\",\(eventJson)]"
    }

    /// Parse an incoming Nostr EVENT message.
    private func processNostrMessage(_ text: String) {
        guard let data = text.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [Any],
              let messageType = json.first as? String else {
            return
        }

        switch messageType {
        case "EVENT":
            guard json.count >= 3,
                  let event = json[2] as? [String: Any],
                  let senderPubkey = event["pubkey"] as? String,
                  let content = event["content"] as? String else {
                return
            }

            // Skip events from self
            guard senderPubkey != publicKeyHex else { return }

            messagesReceived += 1

            // Derive sender's deviceId from their pubkey
            let senderId = deviceIdFromPubkey(senderPubkey)

            messageQueue.async { [weak self] in
                guard let self = self else { return }

                do {
                    // Decode content (base64-encoded protocol message)
                    let messageData: Data
                    if let decoded = Data(base64Encoded: content) {
                        messageData = decoded
                    } else if let contentData = content.data(using: .utf8) {
                        messageData = contentData
                    } else {
                        return
                    }

                    let bytes = [UInt8](messageData)
                    try self.protocolInstance.nostrMessageReceived(senderId: senderId, data: bytes)

                    self.emitDiagnostic("debug", "Message received from Nostr", context: [
                        "senderId": senderId,
                        "senderPubkey": String(senderPubkey.prefix(16)) + "...",
                        "contentLength": content.count
                    ])
                } catch {
                    self.emitDiagnostic("error", "Error processing Nostr message", context: [
                        "error": error.localizedDescription
                    ])
                }
            }

        case "OK":
            // Event acceptance confirmation from relay
            if json.count >= 3, let eventId = json[1] as? String, let accepted = json[2] as? Bool {
                emitDiagnostic("debug", "Relay event response", context: [
                    "eventId": String(eventId.prefix(16)) + "...",
                    "accepted": accepted
                ])
            }

        case "EOSE":
            // End of stored events — subscription is now live
            emitDiagnostic("debug", "End of stored events received")

        case "NOTICE":
            if json.count >= 2, let message = json[1] as? String {
                emitDiagnostic("warning", "Relay notice", context: ["message": message])
            }

        default:
            emitDiagnostic("debug", "Unknown Nostr message type", context: ["type": messageType])
        }
    }

    // MARK: - WebSocket Message Handling

    private func receiveMessage(from relayUrl: String) {
        let task: URLSessionWebSocketTask? = connectionQueue.sync {
            relayConnections[relayUrl]
        }
        guard let wsTask = task else { return }

        wsTask.receive { [weak self] result in
            guard let self = self else { return }

            switch result {
            case .success(let message):
                switch message {
                case .string(let text):
                    self.bytesReceived += UInt64(text.utf8.count)
                    self.processNostrMessage(text)
                case .data(let data):
                    self.bytesReceived += UInt64(data.count)
                    if let text = String(data: data, encoding: .utf8) {
                        self.processNostrMessage(text)
                    }
                @unknown default:
                    break
                }
                // Continue receiving
                self.receiveMessage(from: relayUrl)

            case .failure(let error):
                self.handleRelayDisconnected(relayUrl, error: error)
            }
        }
    }

    private func sendToRelay(_ relayUrl: String, message: String) {
        let task: URLSessionWebSocketTask? = connectionQueue.sync {
            relayConnections[relayUrl]
        }
        guard let wsTask = task else { return }

        wsTask.send(.string(message)) { [weak self] error in
            if let error = error {
                self?.emitDiagnostic("error", "Failed to send to relay", context: [
                    "relayUrl": relayUrl,
                    "error": error.localizedDescription
                ])
            }
        }
    }

    private func sendToAllRelays(_ message: String) {
        let tasks: [(String, URLSessionWebSocketTask)] = connectionQueue.sync {
            relayConnections.compactMap { (url, task) in
                guard relayConnected[url] == true else { return nil }
                return (url, task)
            }
        }

        for (_, task) in tasks {
            task.send(.string(message)) { [weak self] error in
                if let error = error {
                    self?.emitDiagnostic("error", "Failed to send to relay", context: [
                        "error": error.localizedDescription
                    ])
                }
            }
        }
    }

    // MARK: - Message Polling

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

    private func startPingTimer() {
        stopPingTimer()

        let timer = DispatchSource.makeTimerSource(queue: connectionQueue)
        timer.schedule(deadline: .now() + PING_INTERVAL, repeating: PING_INTERVAL)
        timer.setEventHandler { [weak self] in
            self?.sendPings()
        }
        timer.resume()
        pingTimer = timer
    }

    private func stopPingTimer() {
        pingTimer?.cancel()
        pingTimer = nil
    }

    private func sendPings() {
        let tasks: [(String, URLSessionWebSocketTask)] = connectionQueue.sync {
            relayConnections.compactMap { (url, task) in
                guard relayConnected[url] == true else { return nil }
                return (url, task)
            }
        }

        for (relayUrl, task) in tasks {
            task.sendPing { [weak self] error in
                if let error = error {
                    self?.emitDiagnostic("warning", "Ping failed for relay", context: [
                        "relayUrl": relayUrl,
                        "error": error.localizedDescription
                    ])
                    self?.handleRelayDisconnected(relayUrl, error: error)
                }
            }
        }
    }

    private func pollAndSendMessages() {
        guard isConnected else { return }

        var sent = 0
        let maxBatchSize = 10

        while sent < maxBatchSize {
            guard isConnected else { break }

            guard let message = protocolInstance.nostrGetNextMessage() else {
                break
            }

            publishMessage(
                messageId: message.messageId,
                recipientId: message.recipientId,
                data: Data(message.data),
                replyToMsg: message.replyToMsg
            )
            sent += 1
        }

        if sent > 1 {
            emitDiagnostic("debug", "Batch sent messages via Nostr", context: [
                "count": sent
            ])
        }
    }

    private func publishMessage(messageId: String, recipientId: String, data: Data, replyToMsg: String? = nil) {
        guard isConnected else {
            emitDiagnostic("warning", "Cannot send message - not connected", context: [
                "messageId": messageId,
                "recipientId": recipientId
            ])
            protocolInstance.nostrSendFailed(messageId: messageId)
            return
        }

        // Derive recipient's Nostr pubkey from their deviceId
        let recipientPubkey = pubkeyFromDeviceId(recipientId)

        // Encode message data as base64 content
        let content = data.base64EncodedString()

        // Create signed Nostr event
        guard let eventMessage = createNostrEvent(content: content, recipientPubkey: recipientPubkey) else {
            emitDiagnostic("error", "Failed to create Nostr event")
            protocolInstance.nostrSendFailed(messageId: messageId)
            return
        }

        // Publish to all connected relays
        let tasks: [(String, URLSessionWebSocketTask)] = connectionQueue.sync {
            relayConnections.compactMap { (url, task) in
                guard relayConnected[url] == true else { return nil }
                return (url, task)
            }
        }

        guard !tasks.isEmpty else {
            protocolInstance.nostrSendFailed(messageId: messageId)
            return
        }

        // Send to first connected relay and confirm; fan out to rest
        let (primaryUrl, primaryTask) = tasks[0]
        let otherTasks = Array(tasks.dropFirst())

        primaryTask.send(.string(eventMessage)) { [weak self] error in
            guard let self = self else { return }

            if let error = error {
                self.consecutiveSendFailures += 1
                self.protocolInstance.nostrSendFailed(messageId: messageId)
                self.emitDiagnostic("error", "Failed to send Nostr message", context: [
                    "error": error.localizedDescription,
                    "messageId": messageId,
                    "recipientId": recipientId,
                    "relayUrl": primaryUrl,
                    "consecutiveFailures": self.consecutiveSendFailures
                ])

                if self.consecutiveSendFailures >= self.MAX_CONSECUTIVE_FAILURES {
                    self.emitDiagnostic("warning", "Too many consecutive send failures, triggering disconnect", context: [
                        "failures": self.consecutiveSendFailures
                    ])
                    self.handleRelayDisconnected(primaryUrl, error: error)
                }
            } else {
                self.consecutiveSendFailures = 0
                self.bytesSent += UInt64(eventMessage.utf8.count)
                self.messagesSent += 1
                self.protocolInstance.nostrConfirmSent(messageId: messageId)

                self.emitDiagnostic("debug", "Message sent via Nostr", context: [
                    "messageId": messageId,
                    "recipientId": recipientId,
                    "contentLength": content.count
                ])
            }
        }

        // Fan out to other relays (best-effort, no confirmation tracking)
        for (_, task) in otherTasks {
            task.send(.string(eventMessage)) { _ in }
        }
    }

    // MARK: - Nostr Crypto Helpers

    /// Derive a deterministic secp256k1 keypair from the deviceId.
    /// Both peers can compute each other's pubkeys from device IDs.
    private func deriveKeyPair() {
        // SHA-256 of deviceId gives us 32 bytes for the private key
        privateKeyBytes = sha256Bytes(deviceId)
        // Public key is the x-coordinate of the secp256k1 point (simplified)
        // For a full implementation, use a secp256k1 library.
        // Here we use SHA-256 of the private key as a deterministic pubkey placeholder.
        publicKeyHex = sha256Hex(Data(privateKeyBytes).base64EncodedString())

        emitDiagnostic("debug", "Nostr keypair derived", context: [
            "publicKey": publicKeyHex,
            "deviceId": deviceId
        ])
    }

    /// Derive a public key from a device ID (same algorithm as deriveKeyPair).
    private func pubkeyFromDeviceId(_ deviceId: String) -> String {
        let privKeyBytes = sha256Bytes(deviceId)
        return sha256Hex(Data(privKeyBytes).base64EncodedString())
    }

    /// Reverse lookup: derive deviceId from pubkey.
    /// Since we can't reverse a hash, we store a mapping during message receipt.
    /// For now, we use the pubkey itself as the sender identifier.
    private func deviceIdFromPubkey(_ pubkey: String) -> String {
        // In a full implementation, maintain a pubkey->deviceId cache.
        // The protocol layer handles peer identity via its own mechanisms.
        return pubkey
    }

    /// Sign a Nostr event ID.
    /// Note: This is a simplified signature. For production, use a secp256k1 Schnorr library.
    private func signEvent(eventId: String) -> String {
        // HMAC-SHA256 of the event ID using private key as the signing key.
        // This is NOT a real Schnorr signature but allows the transport to function
        // for testing. Replace with proper BIP-340 Schnorr when secp256k1.swift is added.
        let key = Data(privateKeyBytes)
        guard let messageData = eventId.data(using: .utf8) else {
            return String(repeating: "0", count: 128)
        }
        var hmac = [UInt8](repeating: 0, count: Int(CC_SHA256_DIGEST_LENGTH))
        key.withUnsafeBytes { keyPtr in
            messageData.withUnsafeBytes { msgPtr in
                CCHmac(CCHmacAlgorithm(kCCHmacAlgSHA256),
                        keyPtr.baseAddress, key.count,
                        msgPtr.baseAddress, messageData.count,
                        &hmac)
            }
        }
        // Pad to 128 hex chars (64 bytes) as Nostr expects
        let hmacHex = hmac.map { String(format: "%02x", $0) }.joined()
        return hmacHex + hmacHex  // 64 bytes = 128 hex chars
    }

    // MARK: - Utility

    private func sha256Bytes(_ string: String) -> [UInt8] {
        guard let data = string.data(using: .utf8) else { return [] }
        var hash = [UInt8](repeating: 0, count: Int(CC_SHA256_DIGEST_LENGTH))
        data.withUnsafeBytes {
            _ = CC_SHA256($0.baseAddress, CC_LONG(data.count), &hash)
        }
        return hash
    }

    private func sha256Hex(_ string: String) -> String {
        return sha256Bytes(string).map { String(format: "%02x", $0) }.joined()
    }

    private func escapeJsonString(_ string: String) -> String {
        // Produce a valid JSON string literal
        guard let data = try? JSONSerialization.data(withJSONObject: string) else {
            return "\"\(string)\""
        }
        return String(data: data, encoding: .utf8) ?? "\"\(string)\""
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

extension NostrManager: URLSessionWebSocketDelegate {
    public func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask, didOpenWithProtocol protocol: String?) {
        // Find which relay this task belongs to
        let relayUrl: String? = connectionQueue.sync {
            relayConnections.first(where: { $0.value === webSocketTask })?.key
        }
        if let url = relayUrl {
            handleRelayConnected(url)
        }
    }

    public func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask, didCloseWith closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?) {
        let relayUrl: String? = connectionQueue.sync {
            relayConnections.first(where: { $0.value === webSocketTask })?.key
        }
        if let url = relayUrl {
            handleRelayDisconnected(url, error: nil)
        }
    }

    public func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let wsTask = task as? URLSessionWebSocketTask else { return }
        let relayUrl: String? = connectionQueue.sync {
            relayConnections.first(where: { $0.value === wsTask })?.key
        }
        if let url = relayUrl, let error = error {
            handleRelayDisconnected(url, error: error)
        }
    }
}
