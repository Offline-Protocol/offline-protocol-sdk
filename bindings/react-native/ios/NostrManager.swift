//
// NostrManager.swift
// OfflineProtocol
//
// Nostr transport implementation using WebSocket connections to Nostr relays.
// All event creation, signing (BIP-340 Schnorr), and subscription filters
// are handled by the Rust core via UniFFI. This class is a thin WebSocket
// bridge that sends pre-signed events and routes received events to Rust.
//

import Foundation

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

    // Nostr public key (obtained from Rust core)
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

    // Pending relay confirmations: Nostr event_id → protocol message_id.
    // Populated when a WebSocket send succeeds; removed on relay ["OK", ...].
    // Guarded by stateLock.
    private var _pendingEventConfirmations: [String: String] = [:]

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
        // NOT the public `stop()`: its `disconnectAll` hop captures
        // `[weak self]`, and forming a weak reference to an object already
        // inside `dealloc` is a hard runtime abort (`_objc_fatal` → SIGABRT),
        // not a benign nil. Latent rather than fatal today only because the
        // guard below short-circuits the second `stop()` that `destroy()`
        // triggers — but a manager released while still `.running` (module
        // teardown without `destroy()`) walks straight into it.
        stop(fromDeinit: true)
    }

    // MARK: - Configuration

    /// Configure the Nostr relay connection.
    public func configure(
        relayUrls: [String],
        autoReconnect: Bool = true,
        maxReconnectAttempts: Int = 0,
        connectionTimeout: TimeInterval = 30.0
    ) {
        self.relayUrls = relayUrls
        self.autoReconnect = autoReconnect
        self.maxReconnectAttempts = maxReconnectAttempts

        // Get the Nostr signing pubkey from the Rust core. This is a
        // per-install key that settles when MLS initialization installs the
        // persisted signing secret; sendSubscription() refreshes it on every
        // relay (re)connect in case that happens after configure().
        self.publicKeyHex = protocolInstance.nostrGetPublicKey() ?? ""

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
        stop(fromDeinit: false)
    }

    /// - Parameter fromDeinit: `true` only from `deinit`, which selects the
    ///   `disconnectAll` variant that names no `self` in a capture list.
    private func stop(fromDeinit: Bool) {
        guard state == .running || state == .starting else {
            return
        }

        updateState(.stopping)

        // Cancel all reconnect attempts (on connectionQueue for thread safety)
        connectionQueue.sync {
            for (_, workItem) in reconnectWorkItems {
                workItem.cancel()
            }
            reconnectWorkItems.removeAll()
        }

        // Stop timers
        stopMessagePolling()
        stopPingTimer()

        // Close all relay connections
        disconnectAll(fromDeinit: fromDeinit)

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

        // Pass the task directly to avoid deadlock on reconnect path
        if let wsTask = task {
            receiveMessage(from: relayUrl, task: wsTask)
        }
    }

    private func disconnectRelay(_ relayUrl: String) {
        connectionQueue.async { [weak self] in
            self?.relayConnections[relayUrl]?.cancel(with: .goingAway, reason: nil)
            self?.relayConnections.removeValue(forKey: relayUrl)
            self?.relayConnected[relayUrl] = false
        }
    }

    private func disconnectAll(fromDeinit: Bool) {
        if fromDeinit {
            // `deinit` cannot form a weak reference to `self` (hard abort) and
            // must not form a strong one either (resurrection, and it would
            // defer dealloc onto this queue). `connectionQueue.sync` takes a
            // NON-escaping closure, so it uses `self` directly without
            // creating any managed reference — the one shape that is safe
            // here. Running the cleanup synchronously also keeps the state
            // confined to `connectionQueue` exactly as the async path does,
            // and cancelling the sockets still happens rather than being
            // left to whatever the tasks do when their owner disappears.
            connectionQueue.sync {
                for (_, task) in relayConnections {
                    task.cancel(with: .goingAway, reason: nil)
                }
                relayConnections.removeAll()
                relayConnected.removeAll()
                subscriptionIds.removeAll()
            }
        } else {
            connectionQueue.async { [weak self] in
                guard let self = self else { return }
                for (_, task) in self.relayConnections {
                    task.cancel(with: .goingAway, reason: nil)
                }
                self.relayConnections.removeAll()
                self.relayConnected.removeAll()
                self.subscriptionIds.removeAll()
            }
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
            messageQueue.async { [weak self] in
                guard let self = self else { return }
                self.updateState(.running)
                try? self.protocolInstance.nostrStatusChanged(isConnected: true)
                self.consecutiveSendFailures = 0
                self.startMessagePolling()
                self.startPingTimer()
                self.pollAndSendMessages()
            }
        } else if !anyConnected && wasConnected {
            messageQueue.async { [weak self] in
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

        connectionQueue.async { [weak self] in
            guard let self = self else { return }
            self.reconnectWorkItems[relayUrl]?.cancel()
            let workItem = DispatchWorkItem { [weak self] in
                self?.connectToRelay(relayUrl)
            }
            self.reconnectWorkItems[relayUrl] = workItem
            self.connectionQueue.asyncAfter(deadline: .now() + delay, execute: workItem)
        }
    }

    // MARK: - Nostr Protocol (NIP-01)

    /// Send subscription request to receive DMs addressed to this device's pubkey.
    /// The subscription filter is created by the Rust core (real secp256k1 pubkey).
    private func sendSubscription(to relayUrl: String) {
        // Re-read the signing pubkey used for self-event filtering: it
        // rotates when MLS initialization installs the persisted signing
        // secret, and this runs on every relay (re)connect — before any
        // event can arrive on this subscription.
        if let freshKey = protocolInstance.nostrGetPublicKey(), !freshKey.isEmpty {
            publicKeyHex = freshKey
        }

        let subId = UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(16)
        let subIdStr = String(subId)

        connectionQueue.async { [weak self] in
            self?.subscriptionIds[relayUrl] = subIdStr
        }

        // Get the subscription filter from Rust (uses the real BIP-340 pubkey)
        guard let reqJson = protocolInstance.nostrGetSubscriptionFilter(subscriptionId: subIdStr) else {
            emitDiagnostic("error", "Failed to get subscription filter from Rust core")
            return
        }

        sendToRelay(relayUrl, message: reqJson)

        emitDiagnostic("debug", "Sent subscription to relay", context: [
            "relayUrl": relayUrl,
            "subscriptionId": subIdStr,
            "publicKey": publicKeyHex
        ])
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

                    // Pass the Nostr pubkey as sender_id — Rust extracts
                    // the real protocol-level sender from Message.sender
                    let bytes = [UInt8](messageData)
                    try self.protocolInstance.nostrMessageReceived(senderId: senderPubkey, data: bytes)

                    self.emitDiagnostic("debug", "Message received from Nostr", context: [
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
            // Relay acceptance/rejection: ["OK", event_id, accepted, reason?]
            if json.count >= 3, let eventId = json[1] as? String, let accepted = json[2] as? Bool {
                let reason = json.count >= 4 ? json[3] as? String : nil

                // Look up the protocol message_id for this Nostr event_id
                stateLock.lock()
                let messageId = _pendingEventConfirmations.removeValue(forKey: eventId)
                stateLock.unlock()

                if let msgId = messageId {
                    if accepted {
                        protocolInstance.nostrConfirmSent(messageId: msgId)
                    } else {
                        protocolInstance.nostrSendFailedWithReason(
                            messageId: msgId,
                            reason: reason ?? "Relay rejected event"
                        )
                    }
                }

                emitDiagnostic("debug", "Relay event response", context: [
                    "eventId": String(eventId.prefix(16)) + "...",
                    "accepted": accepted,
                    "reason": reason ?? "none",
                    "tracked": messageId != nil
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

    private func receiveMessage(from relayUrl: String, task wsTask: URLSessionWebSocketTask) {
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
                // Continue receiving with the same task reference
                self.receiveMessage(from: relayUrl, task: wsTask)

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

    /// Must be called on `connectionQueue` (the ping timer fires there).
    private func sendPings() {
        let tasks: [(String, URLSessionWebSocketTask)] = relayConnections.compactMap { (url, task) in
            guard relayConnected[url] == true else { return nil }
            return (url, task)
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

            // event_json is the complete signed ["EVENT", {...}] from Rust
            let eventMessage = message.eventJson

            let tasks: [(String, URLSessionWebSocketTask)] = connectionQueue.sync {
                relayConnections.compactMap { (url, task) in
                    guard relayConnected[url] == true else { return nil }
                    return (url, task)
                }
            }

            guard !tasks.isEmpty else {
                protocolInstance.nostrSendFailed(messageId: message.messageId)
                continue
            }

            // Send to first connected relay with confirmation
            let (primaryUrl, primaryTask) = tasks[0]
            let otherTasks = Array(tasks.dropFirst())
            let msgId = message.messageId
            let evtId = message.eventId

            primaryTask.send(.string(eventMessage)) { [weak self] error in
                guard let self = self else { return }

                if let error = error {
                    self.consecutiveSendFailures += 1
                    self.protocolInstance.nostrSendFailed(messageId: msgId)
                    self.emitDiagnostic("error", "Failed to send Nostr message", context: [
                        "error": error.localizedDescription,
                        "messageId": msgId,
                        "relayUrl": primaryUrl,
                        "consecutiveFailures": self.consecutiveSendFailures
                    ])

                    if self.consecutiveSendFailures >= self.MAX_CONSECUTIVE_FAILURES {
                        self.handleRelayDisconnected(primaryUrl, error: error)
                    }
                } else {
                    self.consecutiveSendFailures = 0
                    self.bytesSent += UInt64(eventMessage.utf8.count)
                    self.messagesSent += 1

                    // Track event_id → message_id so we can confirm/fail
                    // when the relay sends ["OK", event_id, accepted, reason].
                    self.stateLock.lock()
                    self._pendingEventConfirmations[evtId] = msgId
                    self.stateLock.unlock()

                    self.emitDiagnostic("debug", "Message sent via Nostr", context: [
                        "messageId": msgId,
                        "eventId": String(evtId.prefix(16)) + "...",
                        "contentLength": eventMessage.count
                    ])
                }
            }

            // Fan out to other relays (best-effort, no confirmation tracking)
            for (_, task) in otherTasks {
                task.send(.string(eventMessage)) { _ in }
            }

            sent += 1
        }

        if sent > 1 {
            emitDiagnostic("debug", "Batch sent messages via Nostr", context: [
                "count": sent
            ])
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
