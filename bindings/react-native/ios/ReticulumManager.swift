//
// ReticulumManager.swift
// OfflineProtocol
//
// Reticulum transport implementation using TCP connection to a local Reticulum daemon.
// Enables long-range mesh networking (LoRa, serial, I2P, TCP, UDP) via the Reticulum stack.
//

import Foundation
import Network

/// Reticulum Manager implementing TransportManager for Reticulum daemon communication
public class ReticulumManager: NSObject, TransportManager {

    // MARK: - TransportManager Protocol

    public let transportId = "reticulum"
    public let transportName = "Reticulum (Mesh)"
    private var _state: TransportState = .unavailable
    public private(set) var state: TransportState {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _state }
        set { stateLock.lock(); defer { stateLock.unlock() }; _state = newValue }
    }
    public weak var delegate: TransportManagerDelegate?

    // MARK: - Constants

    private let MESSAGE_POLL_INTERVAL: TimeInterval = 5.0 // 5s fallback; primary path is event-driven
    private let RECONNECT_INITIAL_DELAY: TimeInterval = 1.0
    private let RECONNECT_MAX_DELAY: TimeInterval = 30.0
    private let RECONNECT_BACKOFF_MULTIPLIER: Double = 2.0
    private let CONNECTION_TIMEOUT: TimeInterval = 60.0 // 60s — Reticulum paths can be high-latency
    private let MAX_CONSECUTIVE_FAILURES = 3

    // MARK: - Properties

    private let protocolInstance: OfflineProtocol
    private let deviceId: String

    // Daemon connection
    private var daemonHost: String = "localhost"
    private var daemonPort: UInt16 = 4242
    private var connection: NWConnection?
    private let connectionQueue = DispatchQueue(label: "com.offlineprotocol.reticulum.connection")

    // Message polling
    private var messageTimer: DispatchSourceTimer?
    private let messageQueue = DispatchQueue(label: "com.offlineprotocol.reticulum.messages")

    // Reconnection (guarded by stateLock)
    private var _reconnectAttempts: Int = 0
    private var _currentReconnectDelay: TimeInterval = 1.0
    private var reconnectWorkItem: DispatchWorkItem?
    private var connectionTimeoutWorkItem: DispatchWorkItem?
    private var maxReconnectAttempts: Int = 0 // 0 = infinite
    private var autoReconnect: Bool = true

    // Lock protecting mutable state accessed from multiple queues
    private let stateLock = NSLock()

    /// Orders the two `reticulumStatusChanged` call sites that nothing else
    /// orders: the connected edge, which runs on [messageQueue], and `stop()`,
    /// which runs inline on whatever thread tore the transport down.
    ///
    /// Without it the connected edge is a check-then-act across two threads.
    /// It can read a live state, and a `stop()` can then run its whole body —
    /// including its own `false` — before the `true` it just cleared reaches
    /// the core. The core is left believing this transport is up moments after
    /// being told it was down, against a `.stopped` transport with no polling,
    /// and nothing corrects it: every correcting path is a teardown path that
    /// already ran. DORS then routes to a transport that will never drain.
    ///
    /// What makes the lock sufficient is that `stop()` publishes `.stopping`
    /// *before* contending for it. Either the announcement takes the lock
    /// first, and `stop()`'s `false` lands after its `true` — the correct
    /// final answer — or `stop()` takes it first and the announcement then
    /// reads a state that refuses it.
    ///
    /// Held across a UniFFI call, which nothing else in this file does. That
    /// is affordable here and only here: the sole contender is the one other
    /// flip site, so the wait is bounded by a single status call that the
    /// waiting thread was about to make anyway. It is taken *outside*
    /// [stateLock] and never the other way round.
    ///
    /// Android needs no equivalent. Its gate, its flip and `stopUnsafe` all
    /// run on the one confinement thread, so they are already atomic against
    /// each other; this is what that costs on a platform with three queues.
    private let statusFlipLock = NSLock()

    private var reconnectAttempts: Int {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _reconnectAttempts }
        set { stateLock.lock(); defer { stateLock.unlock() }; _reconnectAttempts = newValue }
    }
    private var currentReconnectDelay: TimeInterval {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _currentReconnectDelay }
        set { stateLock.lock(); defer { stateLock.unlock() }; _currentReconnectDelay = newValue }
    }

    // Whether configure() has been called (guarded by stateLock)
    private var _isConfigured = false
    private var isConfigured: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isConfigured }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isConfigured = newValue }
    }

    // State tracking (guarded by stateLock)
    /// True between `pause()` and `resume()`. Mirrors `InternetManager`'s flag
    /// of the same name, and exists for the same reason: stopping the poll
    /// timer is not the same as pausing the transport.
    ///
    /// Two paths re-arm the send loop behind a pause without it. The reconnect
    /// edge is the durable one — a daemon that drops and reconnects while the
    /// app is backgrounded reaches `handleConnectionOpened`, which restarted
    /// the poll for the whole background stay. The other is
    /// `onMessagesAvailable`, the *primary* send path: the timer this manager's
    /// pause stops is only the fallback, so a core callback still drained a
    /// batch straight through a paused transport. The Android manager carries
    /// the identical pair.
    private var _isPaused = false
    private var isPaused: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isPaused }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isPaused = newValue }
    }

    private var _isConnected = false
    private var isConnected: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isConnected }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isConnected = newValue }
    }
    private var _isConnecting = false
    private var isConnecting: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isConnecting }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isConnecting = newValue }
    }

    // Failure tracking for DORS (guarded by stateLock)
    private var _consecutiveSendFailures: Int = 0
    private var consecutiveSendFailures: Int {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _consecutiveSendFailures }
        set { stateLock.lock(); defer { stateLock.unlock() }; _consecutiveSendFailures = newValue }
    }

    // Receive buffer for line-delimited TCP (only accessed on connectionQueue)
    private var receiveBuffer = Data()

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

    /// Configure the Reticulum daemon connection.
    /// - Parameters:
    ///   - daemonAddress: TCP address in "host:port" format (default: "localhost:4242")
    ///   - autoReconnect: Whether to auto-reconnect on disconnect (default: true)
    ///   - maxReconnectAttempts: Max reconnect attempts, 0 = infinite (default: 0)
    public func configure(daemonAddress: String = "localhost:4242", autoReconnect: Bool = true, maxReconnectAttempts: Int = 0) {
        let parts = daemonAddress.split(separator: ":")
        self.daemonHost = parts.count > 0 ? String(parts[0]) : "localhost"
        self.daemonPort = parts.count > 1 ? UInt16(parts[1]) ?? 4242 : 4242
        self.autoReconnect = autoReconnect
        self.maxReconnectAttempts = maxReconnectAttempts

        // Warn when connecting to a non-localhost daemon — the TCP link is unencrypted
        let localhostAliases: Set<String> = ["localhost", "127.0.0.1", "::1"]
        if !localhostAliases.contains(daemonHost) {
            emitDiagnostic("warning", "Reticulum daemon is not on localhost — TCP connection is unencrypted", context: [
                "daemonHost": daemonHost
            ])
        }

        isConfigured = true

        emitDiagnostic("info", "Reticulum transport configured", context: [
            "daemonHost": daemonHost,
            "daemonPort": daemonPort,
            "autoReconnect": autoReconnect,
            "maxReconnectAttempts": maxReconnectAttempts
        ])
    }

    // MARK: - TransportManager Implementation

    public func isAvailable() -> Bool {
        // Only report available after configure() has been called, so DORS
        // doesn't select an unconfigured Reticulum transport.
        return isConfigured
    }

    public func start() throws {
        guard state != .running && state != .starting else {
            throw TransportError.alreadyRunning
        }

        emitDiagnostic("info", "Starting Reticulum transport", context: [
            "deviceId": deviceId,
            "daemonAddress": "\(daemonHost):\(daemonPort)"
        ])

        // An explicit start() means "run": a pause() from a previous session
        // must not leave this fresh transport connected-but-mute. Mirrors
        // `InternetManager.start()`.
        isPaused = false

        updateState(.starting)
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

        // Close connection
        disconnect()

        // Notify protocol. Under [statusFlipLock], and after `.stopping` is
        // already published above, which is what orders this against a
        // connected edge racing it from [messageQueue]: that edge either loses
        // the lock and then reads the state this already moved past, or wins
        // it and has its `true` overwritten by this `false`. Inline rather
        // than hopped onto messageQueue — `deinit` calls this, and a `sync`
        // hop would self-deadlock when the last reference is released on that
        // queue, while an `async` hop would let `stop()` return before the
        // core knows.
        statusFlipLock.lock()
        try? protocolInstance.reticulumStatusChanged(isConnected: false)
        statusFlipLock.unlock()

        updateState(.stopped)
        emitDiagnostic("info", "Reticulum transport stopped")
    }

    public func pause() {
        // Set before the timer is cancelled, and read by both paths a
        // cancellation was never going to reach — `onMessagesAvailable` and
        // the reconnect edge in `handleConnectionOpened`. See `isPaused`.
        isPaused = true
        stopMessagePolling()
    }

    public func resume() {
        isPaused = false
        if state == .running && isConnected {
            // Also drains whatever queued during the pause: the poll timer is
            // scheduled at `.now()`, and the core does not re-issue
            // `onMessagesAvailable` for messages it already announced.
            startMessagePolling()
        }
    }

    // MARK: - Connection Management

    private func connect() {
        stateLock.lock()
        let skip = _isConnecting || _isConnected
        if !skip { _isConnecting = true }
        stateLock.unlock()
        guard !skip else { return }

        emitDiagnostic("info", "Connecting to Reticulum daemon", context: [
            "host": daemonHost,
            "port": daemonPort
        ])

        let host = NWEndpoint.Host(daemonHost)
        let port = NWEndpoint.Port(rawValue: daemonPort) ?? NWEndpoint.Port(rawValue: 4242)!

        let conn = NWConnection(host: host, port: port, using: .tcp)

        conn.stateUpdateHandler = { [weak self] newState in
            guard let self = self else { return }
            switch newState {
            case .ready:
                self.handleConnectionOpened()
                self.startReceiving()
            case .failed(let error):
                self.emitDiagnostic("error", "Reticulum connection failed", context: [
                    "error": error.localizedDescription
                ])
                self.handleConnectionClosed(error: error)
            case .cancelled:
                // Intentional close, handled by disconnect()
                break
            case .waiting(let error):
                self.emitDiagnostic("warning", "Reticulum connection waiting", context: [
                    "error": error.localizedDescription
                ])
            default:
                break
            }
        }

        connection = conn
        conn.start(queue: connectionQueue)

        // Connection timeout (cancellable)
        connectionTimeoutWorkItem?.cancel()
        let timeoutItem = DispatchWorkItem { [weak self] in
            guard let self = self, self.isConnecting else { return }
            self.emitDiagnostic("error", "Connection timeout to Reticulum daemon")
            self.handleConnectionClosed(error: nil)
        }
        connectionTimeoutWorkItem = timeoutItem
        connectionQueue.asyncAfter(deadline: .now() + CONNECTION_TIMEOUT, execute: timeoutItem)
    }

    private func disconnect() {
        connectionTimeoutWorkItem?.cancel()
        connectionTimeoutWorkItem = nil
        connection?.cancel()
        connection = nil
        isConnected = false
        isConnecting = false
        // Reset receiveBuffer — safe because either we are already on
        // connectionQueue (reconnect path) or the connection has been
        // cancelled above so no receive callbacks can fire (stop path).
        receiveBuffer = Data()
    }

    private func handleConnectionOpened() {
        connectionTimeoutWorkItem?.cancel()
        connectionTimeoutWorkItem = nil
        isConnected = true
        isConnecting = false
        reconnectAttempts = 0
        currentReconnectDelay = RECONNECT_INITIAL_DELAY
        consecutiveSendFailures = 0
        receiveBuffer = Data()

        emitDiagnostic("info", "Connected to Reticulum daemon")

        // Send identification
        let identifyMsg: [String: Any] = [
            "type": "Identify",
            "device_id": deviceId
        ]
        if let jsonData = try? JSONSerialization.data(withJSONObject: identifyMsg),
           let jsonString = String(data: jsonData, encoding: .utf8) {
            sendRaw(jsonString + "\n")
        }

        // Start polling on main thread; notify protocol after state is .running
        // so that any protocol handler querying transport state sees the correct value.
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            // A stop() that landed while this connection was still being
            // established has already told the core we are down and moved to
            // .stopped. Announcing the connection now would put the state back
            // to .running and the core back to connected, against a transport
            // nothing will ever tear down again — and the next start() would
            // throw .alreadyRunning off it. The connection this opened is
            // stray, so close it here. The Android manager gates the same edge
            // the same way; both are pinned in the uniffi source guards.
            //
            // This gate covers the state write only. It cannot also cover the
            // status flip below, which is a queue hop away — that hop is a
            // second boundary a stop() can land in, and it carries its own
            // check under [statusFlipLock]. A gate here alone would leave the
            // wider of the two windows open.
            guard self.markRunningIfLive() else {
                self.disconnect()
                return
            }

            // Skipped while paused. The status flip below stands either way
            // (the daemon really is connected, and DORS needs to know), but
            // the timer does not: this is the durable half of what `isPaused`
            // closes, since a daemon that drops and reconnects during a
            // background stay reaches here and re-armed the poll for the rest
            // of it. Mirrors `InternetManager.handleAuthenticated`'s
            // `if !isPaused`.
            if !self.isPaused {
                self.startMessagePolling()
            }
            // The status flip is a UniFFI call and does not belong on main:
            // it takes the global protocol mutex, and on the false→true edge
            // takes it a second time to flush the entire outbox. That is the
            // heaviest call this manager makes, at the cadence of a daemon
            // that keeps reconnecting — the same work InternetManager already
            // refuses to do on main (see its handleAuthenticated hop), and
            // the scene-update watchdog does not care that the wait is the
            // core's fault. messageQueue is where every other FFI call in
            // this file runs.
            //
            // Enqueued from inside the main block on purpose: it puts the
            // status flip ahead of the flush below on the same serial queue,
            // preserving the ordering this block already had.
            //
            // The state re-check is not redundant with the gate above. This
            // block runs a hop later, and `stop()` flips `false` inline on
            // whichever thread tore the transport down — so between the gate
            // and here a whole stop can complete, and an unchecked flip would
            // tell the core "up" after it was told "down". Check and call are
            // one decision under [statusFlipLock]; see it for why that is
            // enough. Unlocked explicitly rather than by `defer`, which would
            // hold it across the flush below.
            //
            // `isConnected` is the other half of that decision, and it answers
            // what the state cannot: `stop()` is not the only thing that can
            // overtake this block. [handleConnectionClosed] reaches
            // `messageQueue` in ONE hop from `connectionQueue`, while this
            // `true` takes two — `connectionQueue`, then main, then here — so
            // a link that opens and dies immediately enqueues its `false`
            // *ahead* of this `true`. The state says nothing about it: with
            // `autoReconnect` on, that path leaves `.running` untouched, so
            // the core would be told "up" about a dead connection and route to
            // a transport that never drains until the next attempt resolves
            // the flags. (Before the hop below existed, both flips ran on
            // main — one queue, so enqueue order was causal. The hop is what
            // made this reachable, so it is what has to answer for it.)
            // `isConnected` is cleared before that `false` is enqueued and set
            // again only by a successful open, so it reads exactly "is there
            // still a connection to announce". Suppressing a flip can never
            // lose one: every path that clears it either reconnects, which
            // announces itself, or stops.
            //
            // [weak self] like every other closure here: a strong capture
            // would let this flip fire against a manager the module already
            // released through `destroy()`.
            self.messageQueue.async { [weak self] in
                guard let self = self else { return }
                self.statusFlipLock.lock()
                if self.isConnected && self.state != .stopping && self.state != .stopped {
                    try? self.protocolInstance.reticulumStatusChanged(isConnected: true)
                }
                self.statusFlipLock.unlock()
                // Immediately flush queued messages — unless paused, for the
                // same reason the poll restart above is skipped.
                guard !self.isPaused else { return }
                self.pollAndSendMessages()
            }
        }
    }

    private func startReceiving() {
        connection?.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] content, _, isComplete, error in
            guard let self = self else { return }

            if let data = content {
                self.receiveBuffer.append(data)

                // Process complete lines (newline-delimited JSON)
                let newlineByte = Data([0x0A])
                while let newlineRange = self.receiveBuffer.range(of: newlineByte) {
                    let lineData = self.receiveBuffer.subdata(in: self.receiveBuffer.startIndex..<newlineRange.lowerBound)
                    self.receiveBuffer.removeSubrange(self.receiveBuffer.startIndex..<newlineRange.upperBound)
                    if !lineData.isEmpty {
                        self.processReceivedData(lineData)
                    }
                }
            }

            if isComplete {
                self.handleConnectionClosed(error: nil)
                return
            }

            if let error = error {
                self.handleConnectionClosed(error: error)
                return
            }

            // Continue receiving
            self.startReceiving()
        }
    }

    private func handleConnectionClosed(error: NWError?) {
        stateLock.lock()
        let wasConnected = _isConnected
        let wasConnecting = _isConnecting
        _isConnected = false
        _isConnecting = false
        stateLock.unlock()

        // Prevent duplicate disconnect handling
        guard wasConnected || wasConnecting else { return }

        // Stop polling immediately
        DispatchQueue.main.async { [weak self] in
            self?.stopMessagePolling()
        }

        emitDiagnostic("warning", "Reticulum daemon disconnected", context: [
            "error": error?.localizedDescription ?? "none",
            "wasConnected": wasConnected
        ])

        // Notify the protocol off main, and handle reconnection on main —
        // consistent with handleConnectionOpened, which splits the same way.
        // The status flip is a UniFFI call, so it waits on the global protocol
        // mutex; the reconnect scheduling is timer and state work that the
        // rest of this manager already drives from main.
        //
        // The two now run concurrently, where one block ran them in order. The
        // ordering that mattered survives anyway: `messageQueue` is serial, so
        // this false always reaches the core ahead of the true a successful
        // reconnect enqueues, and the reconnect cannot land inside this call
        // regardless — its shortest delay is a full second.
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            do {
                try self.protocolInstance.reticulumStatusChanged(isConnected: false)
            } catch {
                self.emitDiagnostic("error", "Failed to notify protocol of disconnection", context: [
                    "error": error.localizedDescription
                ])
            }
        }

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            // Attempt reconnection if enabled
            if self.autoReconnect && self.state != .stopping && self.state != .stopped {
                self.scheduleReconnect()
            } else {
                self.updateState(.stopped)
            }
        }
    }

    private func scheduleReconnect() {
        guard autoReconnect else { return }
        guard maxReconnectAttempts == 0 || reconnectAttempts < maxReconnectAttempts else {
            emitDiagnostic("error", "Max reconnect attempts reached", context: [
                "attempts": reconnectAttempts,
                "maxAttempts": maxReconnectAttempts
            ])
            DispatchQueue.main.async { [weak self] in
                self?.updateState(.stopped)
            }
            return
        }

        reconnectAttempts += 1

        let delay = currentReconnectDelay
        currentReconnectDelay = min(currentReconnectDelay * RECONNECT_BACKOFF_MULTIPLIER, RECONNECT_MAX_DELAY)

        emitDiagnostic("info", "Scheduling reconnect to Reticulum daemon", context: [
            "attempt": reconnectAttempts,
            "delaySeconds": delay
        ])

        reconnectWorkItem?.cancel()
        let workItem = DispatchWorkItem { [weak self] in
            self?.disconnect()
            self?.connect()
        }
        reconnectWorkItem = workItem
        connectionQueue.asyncAfter(deadline: .now() + delay, execute: workItem)
    }

    // MARK: - Event-Driven Sending

    /// Called by the Rust transport callback when new outgoing messages are available.
    /// This is the primary send path, replacing timer-based polling.
    /// Called by the Rust transport callback when new outgoing messages are
    /// available.
    ///
    /// This is the *primary* send path — the timer `pause()` cancels is the
    /// fallback — so it carries the pause check itself. Without it a paused
    /// transport still drained a batch per callback, each message taking the
    /// core's global protocol mutex, for as long as the core kept announcing.
    /// The messages are not lost: they stay queued in the core and `resume()`
    /// drains them.
    public func onMessagesAvailable() {
        guard !isPaused else { return }
        messageQueue.async { [weak self] in
            guard let self = self, !self.isPaused else { return }
            self.pollAndSendMessages()
        }
    }

    // MARK: - Message Handling

    private func processReceivedData(_ data: Data) {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let messageType = json["type"] as? String else {
            // Non-JSON data with no sender information — cannot route, skip
            emitDiagnostic("warning", "Received non-JSON data from Reticulum daemon, skipping", context: [
                "size": data.count
            ])
            return
        }

        switch messageType {
        case "MessageReceived":
            guard let senderId = json["sender"] as? String,
                  let content = json["content"] as? String else {
                emitDiagnostic("warning", "Invalid MessageReceived: missing sender or content")
                return
            }

            guard !senderId.isEmpty else {
                emitDiagnostic("warning", "Invalid MessageReceived: empty sender")
                return
            }

            let encoding = json["encoding"] as? String

            messageQueue.async { [weak self] in
                guard let self = self else { return }

                do {
                    let messageData: Data
                    if encoding == "base64", let decoded = Data(base64Encoded: content) {
                        messageData = decoded
                    } else if let contentData = content.data(using: .utf8) {
                        messageData = contentData
                    } else {
                        return
                    }

                    let bytes = [UInt8](messageData)
                    try self.protocolInstance.reticulumMessageReceived(senderId: senderId, data: bytes)

                    self.emitDiagnostic("debug", "Message received from Reticulum", context: [
                        "senderId": senderId,
                        "contentLength": content.count
                    ])
                } catch {
                    self.emitDiagnostic("error", "Error processing Reticulum message", context: [
                        "error": error.localizedDescription
                    ])
                }
            }

        case "StatusUpdate":
            let daemonStatus = json["status"] as? String ?? "unknown"
            emitDiagnostic("debug", "Reticulum daemon status update", context: [
                "status": daemonStatus
            ])

        default:
            emitDiagnostic("debug", "Unknown Reticulum message type", context: [
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
        guard isConnected else { return }
        sendNextMessage(sent: 0, maxBatchSize: 10)
    }

    /// Sends messages one at a time, chaining the next send from each completion
    /// handler so that NWConnection writes are serialized (no concurrent sends).
    private func sendNextMessage(sent: Int, maxBatchSize: Int) {
        guard sent < maxBatchSize, isConnected else {
            if sent > 1 {
                emitDiagnostic("debug", "Batch sent messages via Reticulum", context: [
                    "count": sent
                ])
            }
            return
        }

        guard let message = protocolInstance.reticulumGetNextMessage() else {
            if sent > 1 {
                emitDiagnostic("debug", "Batch sent messages via Reticulum", context: [
                    "count": sent
                ])
            }
            return
        }

        sendMessage(
            messageId: message.messageId,
            recipientId: message.recipientId,
            data: Data(message.data),
            replyToMsg: message.replyToMsg
        ) { [weak self] in
            guard let self = self else { return }
            self.messageQueue.async {
                self.sendNextMessage(sent: sent + 1, maxBatchSize: maxBatchSize)
            }
        }
    }

    private func sendMessage(messageId: String, recipientId: String, data: Data, replyToMsg: String? = nil, completion: (() -> Void)? = nil) {
        guard isConnected, connection != nil else {
            emitDiagnostic("warning", "Cannot send message - not connected", context: [
                "messageId": messageId,
                "recipientId": recipientId
            ])
            protocolInstance.reticulumSendFailed(messageId: messageId)
            completion?()
            return
        }

        let content = data.base64EncodedString()

        var reticulumMessage: [String: Any] = [
            "type": "SendMessage",
            "recipient": recipientId,
            "content": content,
            "encoding": "base64"
        ]
        if let replyToMsg = replyToMsg, !replyToMsg.isEmpty {
            reticulumMessage["reply_to_msg"] = replyToMsg
        }

        guard let jsonData = try? JSONSerialization.data(withJSONObject: reticulumMessage),
              let jsonString = String(data: jsonData, encoding: .utf8) else {
            emitDiagnostic("error", "Failed to create Reticulum message")
            protocolInstance.reticulumSendFailed(messageId: messageId)
            completion?()
            return
        }

        sendRaw(jsonString + "\n") { [weak self] error in
            guard let self = self else { return }

            if let error = error {
                self.consecutiveSendFailures += 1
                self.protocolInstance.reticulumSendFailed(messageId: messageId)
                self.emitDiagnostic("error", "Failed to send Reticulum message", context: [
                    "error": error.localizedDescription,
                    "messageId": messageId,
                    "recipientId": recipientId,
                    "consecutiveFailures": self.consecutiveSendFailures
                ])

                if self.consecutiveSendFailures >= self.MAX_CONSECUTIVE_FAILURES {
                    self.emitDiagnostic("warning", "Too many consecutive send failures, triggering reconnect", context: [
                        "failures": self.consecutiveSendFailures
                    ])
                    self.handleConnectionClosed(error: nil)
                }
            } else {
                self.consecutiveSendFailures = 0
                self.protocolInstance.reticulumConfirmSent(messageId: messageId)

                self.emitDiagnostic("debug", "Message sent via Reticulum", context: [
                    "messageId": messageId,
                    "recipientId": recipientId,
                    "contentLength": content.count
                ])
            }

            completion?()
        }
    }

    // MARK: - TCP Send

    private func sendRaw(_ string: String, completion: ((NWError?) -> Void)? = nil) {
        guard let data = string.data(using: .utf8) else {
            completion?(.posix(.EINVAL))
            return
        }
        connection?.send(content: data, completion: .contentProcessed { error in
            completion?(error)
        })
    }

    // MARK: - State Management

    private func updateState(_ newState: TransportState) {
        state = newState
        delegate?.transportManager(self, didChangeState: newState)
    }

    /// Claims `.running` for a connection that has just come up, but only if
    /// the transport has not begun stopping. Returns false when it has, so the
    /// caller can clean up the connection it opened instead of publishing a
    /// state a concurrent `stop()` has already moved past.
    ///
    /// One operation rather than a `guard` followed by an `updateState`,
    /// because those are two separate [stateLock] acquisitions: a `stop()`
    /// landing between them writes `.stopped` and this then writes `.running`
    /// back over it, wedging a torn-down transport at `.running` where the
    /// next `start()` throws `.alreadyRunning` and nothing else ever tears it
    /// down. The delegate notification stays outside the lock, matching
    /// [updateState] — it reaches the bridge module, and calling into that
    /// while holding this manager's lock would put arbitrary downstream work
    /// inside the critical section.
    private func markRunningIfLive() -> Bool {
        stateLock.lock()
        guard _state != .stopping, _state != .stopped else {
            stateLock.unlock()
            return false
        }
        _state = .running
        stateLock.unlock()
        delegate?.transportManager(self, didChangeState: .running)
        return true
    }

    // MARK: - Diagnostics

    private func emitDiagnostic(_ level: String, _ message: String, context: [String: Any] = [:]) {
        delegate?.transportManager(self, didEmitDiagnostic: level, message: message, context: context)
    }
}
