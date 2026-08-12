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

    // Relay connections: URL string -> URLSessionWebSocketTask
    private var relayUrls: [String] = []
    private var relayConnections: [String: URLSessionWebSocketTask] = [:]
    private var relayConnected: [String: Bool] = [:]
    private var subscriptionIds: [String: String] = [:]
    private var urlSession: URLSession?

    // Nostr public key (obtained from Rust core)
    private var publicKeyHex: String = ""

    // Message polling. Both timers are guarded by [stateLock] — see
    // [startMessagePolling] for why they have to share the flag's lock rather
    // than sit beside it.
    private var _messageTimer: DispatchSourceTimer?
    private var _pingTimer: DispatchSourceTimer?
    private let messageQueue = DispatchQueue(label: "com.offlineprotocol.nostr.messages")
    private let connectionQueue = DispatchQueue(label: "com.offlineprotocol.nostr.connection")

    // Lock protecting mutable state accessed from multiple queues
    private let stateLock = NSLock()

    /// Orders the two `nostrStatusChanged` call sites that nothing else
    /// orders: the connected edge, which runs on [messageQueue], and `stop()`,
    /// which runs inline on whatever thread tore the transport down.
    ///
    /// Same reasoning as `ReticulumManager.statusFlipLock`, which carries the
    /// full argument — these two managers are mirrors of each other and the
    /// window is the same one. It is narrower here, because this edge's check
    /// and its flip sit in one [messageQueue] block rather than a hop apart,
    /// but "narrower" is not "closed": the check and the call are still two
    /// steps, and `stop()` runs on another thread between them for free.
    /// `stop()` publishes `.stopping` before contending for this lock, which
    /// is what makes taking it sufficient.
    private let statusFlipLock = NSLock()

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

    // Subscription ids belonging to in-flight key-package resolution queries,
    // as opposed to the standing message subscription. Events arriving under
    // one of these are records fetched on the transport's behalf, not inbound
    // messages, and go to a different entry point. Guarded by stateLock.
    private var _activeQueryIds: Set<String> = []

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

    /// True between `pause()` and `resume()`. Mirrors `InternetManager`'s flag
    /// of the same name, and exists for the same reason: stopping the poll and
    /// ping timers is not the same as pausing the transport.
    ///
    /// Two paths re-arm the send loop behind a pause without it. The reconnect
    /// edge is the durable one — a relay that drops and reconnects while the
    /// app is backgrounded runs `updateConnectionStatus`'s connected branch,
    /// which restarted a 100ms poll *and* the 30s ping for the whole background
    /// stay. The other is `onMessagesAvailable`, the *primary* send path: the
    /// timer this manager's pause stops is only the fallback, so a core
    /// callback still drained a batch of ten straight through a paused
    /// transport. The Android manager carries the identical pair.
    private var _isPaused = false
    private var isPaused: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isPaused }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isPaused = newValue }
    }

    // Failure tracking for DORS
    private var _consecutiveSendFailures: Int = 0
    private var consecutiveSendFailures: Int {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _consecutiveSendFailures }
        set { stateLock.lock(); defer { stateLock.unlock() }; _consecutiveSendFailures = newValue }
    }

    // MARK: - Initialization

    /// The profile is deliberately not taken here.
    ///
    /// This manager is built at configure time, before the protocol has an
    /// identity, so any id passed in could only be the app-chosen profile —
    /// and the one thing this transport must never carry is a value a
    /// username can be recomputed from. The address is read from the protocol
    /// at `start()`, which is the first moment it is both known and needed.
    public init(protocol protocolInstance: OfflineProtocol) {
        self.protocolInstance = protocolInstance
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

        // Refused without an identity, and refused *here* rather than at the
        // one call site that enables the transport: `start()` is the single
        // point that opens a socket, and the first thing a socket does is
        // publish this device's routing tag to a third-party relay.
        //
        // That tag is derived from the address. With no identity there is no
        // address — the core installs no Nostr transport at all — so starting
        // anyway would open relay connections that can never subscribe, which
        // is indistinguishable from "nobody is talking to us".
        guard let address = protocolInstance.localAddress(), !address.isEmpty else {
            throw TransportError.notAvailable(
                "Nostr requires the protocol identity. Initialize MLS (or leave encryption enabled) before enabling Nostr."
            )
        }

        // An identity is necessary but not sufficient. The core registers the
        // Nostr transport during the identity rebuild and only when Nostr was
        // enabled in the config `create()` received, so `enableTransport("nostr")`
        // against a config that had it off arrives here with an address and no
        // transport behind it.
        //
        // Probed with the subscription filter because that is the exact call the
        // socket-open callback makes, and a nil answer *there* is not a safe
        // failure: it is logged and returned from with no retry, so the socket
        // stays connected to a relay it never subscribes on for the rest of its
        // life. Refusing before any socket exists is the recoverable shape.
        guard protocolInstance.nostrGetSubscriptionFilter(subscriptionId: "startup-probe") != nil else {
            throw TransportError.notAvailable(
                "No Nostr transport is registered. Enable Nostr in the protocol configuration before starting it."
            )
        }

        emitDiagnostic("info", "Starting Nostr transport", context: [
            "address": address,
            "relayCount": relayUrls.count,
            "publicKey": publicKeyHex
        ])

        // An explicit start() means "run": a pause() from a previous session
        // must not leave this fresh transport connected-but-mute. Mirrors
        // `InternetManager.start()`.
        isPaused = false

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

        // Notify protocol. Under [statusFlipLock], and after `.stopping` is
        // already published above — see `ReticulumManager.stop()` for the
        // ordering argument. Inline rather than hopped onto messageQueue:
        // `deinit` reaches here, and a `sync` hop would self-deadlock when the
        // last reference is released on that queue.
        statusFlipLock.lock()
        try? protocolInstance.nostrStatusChanged(isConnected: false)
        statusFlipLock.unlock()

        updateState(.stopped)
        emitDiagnostic("info", "Nostr transport stopped")
    }

    public func pause() {
        // Set before the timers are cancelled, and read by both paths a
        // cancellation was never going to reach — `onMessagesAvailable` and
        // the reconnect edge in `updateConnectionStatus`. See `isPaused`.
        //
        // The ordering is what makes this a pause rather than a cancel: any
        // arming that has not yet taken [stateLock] sees the flag and refuses
        // (`startMessagePolling`), and any that already holds it installs a
        // timer the `stop` below then cancels. Neither order leaves a live
        // timer behind.
        isPaused = true
        stopMessagePolling()
        stopPingTimer()
    }

    public func resume() {
        isPaused = false
        if state == .running && isConnected {
            // Also drains whatever queued during the pause: the poll timer is
            // scheduled at `.now()`, and the core does not re-issue
            // `onMessagesAvailable` for messages it already announced.
            startMessagePolling()
            startPingTimer()
        }
    }

    // MARK: - Event-Driven Sending

    /// Called by the Rust transport callback when new outgoing messages are
    /// available.
    ///
    /// This is the *primary* send path — the timer `pause()` cancels is the
    /// 100ms fallback — so it carries the pause check itself. Without it a
    /// paused transport still drained a batch of ten per callback, each one
    /// taking the core's global protocol mutex, for as long as the core kept
    /// announcing. The messages are not lost: they stay queued in the core and
    /// `resume()` drains them.
    public func onMessagesAvailable() {
        guard !isPaused else { return }
        messageQueue.async { [weak self] in
            guard let self = self, !self.isPaused else { return }
            self.pollAndSendMessages()
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
        // Sampled and published as one step, inside the queue that owns the
        // map. Read and swapped separately — as these were, across three
        // separate critical sections — two relays transitioning at once can
        // interleave so that the *later* swap publishes the *earlier* reader's
        // answer. A relay connects and this reads `anyConnected = true`, then
        // is descheduled; the same relay drops, and that call reads false,
        // swaps false→false and sees `wasConnected = false`, so neither edge
        // fires; the first resumes and swaps false→true with `wasConnected =
        // false`, firing the connected edge against a relay set that is empty.
        //
        // `stateLock` nests inside `connectionQueue` here and only here. That
        // is safe because the ordering is one-directional: every `stateLock`
        // holder in this file releases it across a single field access and
        // never hops a queue, so nothing ever waits on `connectionQueue` while
        // holding it.
        let (anyConnected, wasConnected): (Bool, Bool) = connectionQueue.sync {
            let any = relayConnected.values.contains(true)
            stateLock.lock()
            let was = _isConnected
            _isConnected = any
            stateLock.unlock()
            return (any, was)
        }

        if anyConnected && !wasConnected {
            messageQueue.async { [weak self] in
                guard let self = self else { return }

                // A stop() that landed while this relay's handshake was still
                // in flight has already told the core we are down and moved to
                // .stopped. Announcing the connection now would put the state
                // back to .running and the core back to connected, against a
                // transport nothing will ever tear down again — and the next
                // start() would throw .alreadyRunning off it. The relay socket
                // is stray, so close it here. The Android manager gates the
                // same edge the same way; both are pinned in the uniffi source
                // guards.
                //
                // The claim covers the state write; the flip below carries its
                // own check under [statusFlipLock], because `stop()` runs its
                // own flip on another thread and nothing else orders the two.
                guard self.markRunningIfLive() else {
                    self.disconnectAll(fromDeinit: false)
                    return
                }

                // `isConnected` alongside the state, mirroring
                // `ReticulumManager` — see the long note there for why a state
                // check alone is the wrong question.
                //
                // It answers a narrower question here than it does there, and
                // it is worth being exact about which. It does *not* rescue the
                // check-then-act in [updateConnectionStatus]: this reads the
                // very value that call published, so a torn swap would be read
                // back as true and announced anyway. That hole is closed at the
                // swap itself, which is now one step under `connectionQueue`.
                // What this term covers is the interval *after* a sound swap —
                // the last relay dropping between the edge being enqueued and
                // this block reaching the front of `messageQueue`. Suppressing
                // the stale true costs nothing: the disconnect that cleared the
                // flag enqueues its own false behind this, and a reconnect
                // announces itself.
                self.statusFlipLock.lock()
                if self.isConnected && self.state != .stopping && self.state != .stopped {
                    try? self.protocolInstance.nostrStatusChanged(isConnected: true)
                }
                self.statusFlipLock.unlock()
                self.consecutiveSendFailures = 0
                // The status flip above stands even while paused — the relay
                // really is up, and DORS needs to know — but the timers do
                // not. This is the durable half of what `isPaused` closes: a
                // relay that drops and reconnects during a background stay
                // reaches here, and restarting the poll from it re-armed the
                // 100ms timer for the rest of the stay. Mirrors
                // `InternetManager.handleAuthenticated`'s `if !isPaused`,
                // ping included — the earlier ping exception here is gone,
                // see `startPingTimer`.
                //
                // Belt to the braces `startMessagePolling`/`startPingTimer`
                // now carry internally: those two refuse to arm while paused
                // whatever this reads, which is what makes the refusal proof
                // against a `pause()` landing between this guard and the
                // calls below. What the guard still earns on its own is
                // `pollAndSendMessages`, which is not a timer and has no
                // internal gate to fall back on.
                guard !self.isPaused else { return }
                self.startMessagePolling()
                self.startPingTimer()
                self.pollAndSendMessages()
            }
        } else if !anyConnected && wasConnected {
            messageQueue.async { [weak self] in
                guard let self = self else { return }
                self.stopMessagePolling()
                self.stopPingTimer()
                self.releaseActiveQueries()
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

            // Route key-package resolution answers away from the message path
            // *before* anything else looks at them. They are not messages: the
            // content is sealed to a different key, and the self-event filter
            // below would be meaningless for a record we deliberately fetched.
            if let subId = json[1] as? String, isActiveQuery(subId) {
                handleQueryEvent(subscriptionId: subId, event: event)
                return
            }

            // Skip events we published ourselves.
            //
            // This only ever catches the LEGACY unsealed form. Sealed frames
            // (NIP-59 gift wraps, kind 1059) are signed by a fresh single-use
            // key per event — that unlinkability is the point — so `pubkey`
            // never equals ours and this guard cannot fire for them. Do not
            // "fix" that by comparing something else: there is nothing on a
            // gift wrap that identifies its author, by design.
            //
            // Nothing is lost. The subscription filters on `#p` = our own
            // routing tag, so our outbound events (addressed to a *peer's*
            // tag) are not delivered here in the first place; the only way to
            // receive our own gift wrap is to message ourselves, and the
            // engine's message-id deduplication and self-suppression handle
            // that case on the Rust side.
            guard senderPubkey != publicKeyHex else { return }

            // NIP-01 requires `created_at`; a missing or malformed one reaches
            // Rust as 0, which is ignored for the watermark rather than
            // treated as receive progress.
            let createdAt = (event["created_at"] as? NSNumber)?.int64Value ?? 0

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
                    // the real protocol-level sender from Message.sender.
                    // `createdAt` advances the persisted receive watermark,
                    // which becomes the `since` on the next subscription — the
                    // bound that stops a relay replaying its whole retention
                    // window on every reconnect.
                    let bytes = [UInt8](messageData)
                    try self.protocolInstance.nostrMessageReceivedAt(
                        senderId: senderPubkey,
                        data: bytes,
                        createdAt: createdAt
                    )

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
            // End of stored events. For the standing message subscription this
            // just means "live from here"; for a resolution query it means the
            // relay has given us everything it holds, so the query is done and
            // its subscription should not stay open.
            if json.count >= 2, let subId = json[1] as? String, isActiveQuery(subId) {
                finishQuery(subscriptionId: subId)
            } else {
                emitDiagnostic("debug", "End of stored events received")
            }

        case "NOTICE":
            if json.count >= 2, let message = json[1] as? String {
                emitDiagnostic("warning", "Relay notice", context: ["message": message])
            }

        default:
            emitDiagnostic("debug", "Unknown Nostr message type", context: ["type": messageType])
        }
    }

    // MARK: - Key-Package Resolution Queries

    private func isActiveQuery(_ subscriptionId: String) -> Bool {
        stateLock.lock(); defer { stateLock.unlock() }
        return _activeQueryIds.contains(subscriptionId)
    }

    /// Drains queries the transport wants issued and sends each REQ to every
    /// connected relay.
    ///
    /// Broadcast rather than primary-only, unlike an outgoing event: a peer's
    /// published records may sit on relays we share with them but not with the
    /// first one in our list, and there is no acknowledgement that would tell
    /// us we asked the wrong one. Duplicate answers are free — the transport
    /// opens each independently and the engine deduplicates the key package.
    private func pollAndSendQueries() {
        guard isConnected else { return }

        var issued = 0
        let maxBatchSize = 5

        while issued < maxBatchSize {
            guard let query = protocolInstance.nostrGetNextQuery() else { break }
            issued += 1

            let relays: [String] = connectionQueue.sync {
                relayConnections.compactMap { (url, _) in
                    relayConnected[url] == true ? url : nil
                }
            }

            guard !relays.isEmpty else {
                // Nothing to ask. Release it rather than leaving the transport
                // holding an entry no answer will ever arrive for; the next
                // send to that peer re-queues it once the rate limit lapses.
                protocolInstance.nostrQueryCompleted(queryId: query.queryId)
                break
            }

            stateLock.lock()
            _activeQueryIds.insert(query.queryId)
            stateLock.unlock()

            for relayUrl in relays {
                sendToRelay(relayUrl, message: query.reqJson)
            }

            emitDiagnostic("debug", "Issued Nostr key-package query", context: [
                "queryId": query.queryId,
                "relays": relays.count
            ])
        }
    }

    private func handleQueryEvent(subscriptionId: String, event: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: event),
              let eventJson = String(data: data, encoding: .utf8) else {
            return
        }

        messageQueue.async { [weak self] in
            guard let self = self else { return }
            do {
                try self.protocolInstance.nostrQueryEventReceived(
                    queryId: subscriptionId,
                    eventJson: eventJson
                )
            } catch {
                self.emitDiagnostic("error", "Error processing Nostr key-package record", context: [
                    "error": error.localizedDescription
                ])
            }
        }
    }

    /// Closes a finished query on every relay and releases it in the transport.
    ///
    /// A query is broadcast, so more than one relay answers it and each sends
    /// its own EOSE. The first one closes it: leaving the subscription open for
    /// the stragglers would keep a live filter on a peer's routing tag for the
    /// life of the connection, which is precisely the standing signal this
    /// design avoids elsewhere. A later relay's records are simply missed, and
    /// the next send to that peer re-queues the lookup.
    ///
    /// Releasing the query in the transport is deferred onto `messageQueue`,
    /// and that is load-bearing rather than tidiness. A relay sends its stored
    /// events immediately before EOSE, and `handleQueryEvent` hands each to
    /// `messageQueue` asynchronously — so releasing here, synchronously on the
    /// WebSocket receive path, can drop the `activeQueries` entry while this
    /// query's own records are still queued behind it. Those records then find
    /// an unknown query id and are discarded: cold contact silently fails to
    /// upgrade, and the peer waits out the resolution rate limit before another
    /// attempt. `messageQueue` is serial, so hopping onto it puts the release
    /// strictly after every event already enqueued. The `_activeQueryIds`
    /// removal and the CLOSE stay synchronous — they must beat the *next*
    /// relay's EOSE, and they touch nothing the engine owns.
    /// Releases every in-flight resolution query after the relays drop.
    ///
    /// A query whose relays went away before EOSE never finishes: nothing will
    /// ever answer it, so without this the bridge holds its subscription id for
    /// the life of the process and the transport holds the entry until its own
    /// cap evicts something — possibly a live query. Letting them go costs
    /// nothing, since the next send to those peers re-queues the lookup once
    /// the resolution rate limit lapses.
    ///
    /// Called on `messageQueue` like the rest of the release path, so it lands
    /// after any events already enqueued for these queries.
    private func releaseActiveQueries() {
        stateLock.lock()
        let queryIds = _activeQueryIds
        _activeQueryIds.removeAll()
        stateLock.unlock()

        guard !queryIds.isEmpty else { return }

        for queryId in queryIds {
            protocolInstance.nostrQueryCompleted(queryId: queryId)
        }

        emitDiagnostic("debug", "Released in-flight Nostr key-package queries", context: [
            "count": queryIds.count
        ])
    }

    private func finishQuery(subscriptionId: String) {
        stateLock.lock()
        let wasActive = _activeQueryIds.remove(subscriptionId) != nil
        stateLock.unlock()
        guard wasActive else { return }

        let closeMessage = "[\"CLOSE\",\"\(subscriptionId)\"]"
        let relays: [String] = connectionQueue.sync {
            relayConnections.compactMap { (url, _) in
                relayConnected[url] == true ? url : nil
            }
        }
        for relayUrl in relays {
            sendToRelay(relayUrl, message: closeMessage)
        }

        messageQueue.async { [weak self] in
            self?.protocolInstance.nostrQueryCompleted(queryId: subscriptionId)
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
                    self.processNostrMessage(text)
                case .data(let data):
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

    /// Arms the fallback poll timer, unless the transport is paused.
    ///
    /// The pause gate lives *here*, not at the four call sites that arm this,
    /// and it shares one [stateLock] critical section with installing the
    /// timer. Both halves of that are load-bearing.
    ///
    /// **Why here.** A gate at the call site is an invariant every future
    /// caller has to remember; a gate at the one function that can violate it
    /// is an invariant a new caller cannot get wrong. This is the same move
    /// `react_native_transports_do_not_run_on_the_main_looper` makes by
    /// deriving its manager set instead of listing it.
    ///
    /// **Why under the lock.** `pause()` runs on the React Native method
    /// queue, while the reconnect edge that arms this runs on `messageQueue`
    /// (Reticulum's, on main). They are not ordered against each other, so a
    /// caller that read `isPaused` and *then* called in could be overtaken by
    /// the whole of `pause()` in between — flag set, timer cancelled — and
    /// arm a fresh 100ms timer against a transport the app just paused, which
    /// then polls for the rest of the background stay. That is precisely the
    /// durable symptom `isPaused` exists to remove, so the check and the
    /// install have to be one decision. The Android managers get this for
    /// free: their `pause()` and their reconnect edge are the same thread.
    ///
    /// The source is created inside the critical section and resumed outside
    /// it, which is safe in both directions: a `pause()` that lands in the gap
    /// cancels the timer through [stopMessagePolling] before it ever fires,
    /// and the `resume()` below is still required — releasing a suspended
    /// `DispatchSource` is a hard crash, so the paused branch must return
    /// *before* a source exists rather than cancel one it never resumed.
    ///
    /// The same crash is why the gap is safe against a *second* arming, which
    /// is less obvious. Two callers can overlap so that the second one reads
    /// the first's source as its `previous` and cancels it while the first has
    /// not resumed it yet. Cancelling a suspended source is fine; *releasing*
    /// one is not, and the release cannot happen there — the first caller
    /// still holds its own strong reference and drops it only after
    /// `timer.resume()` returns. Anything that moves the `resume()` off this
    /// straight-line path, or hands the source somewhere it can outlive this
    /// frame while suspended, reintroduces the crash.
    private func startMessagePolling() {
        stateLock.lock()
        guard !_isPaused else {
            stateLock.unlock()
            return
        }
        let previous = _messageTimer
        let timer = DispatchSource.makeTimerSource(queue: messageQueue)
        _messageTimer = timer
        stateLock.unlock()

        previous?.cancel()
        timer.schedule(deadline: .now(), repeating: MESSAGE_POLL_INTERVAL)
        timer.setEventHandler { [weak self] in
            // Re-read per tick: `cancel()` cannot reach a handler already
            // dispatched onto `messageQueue`, so without this one full poll
            // batch — messages *and* queries — leaks past every `pause()`.
            // The Android polling runnables carry the identical check.
            guard let self = self, !self.isPaused else { return }
            self.pollAndSendMessages()
            self.pollAndSendQueries()
        }
        timer.resume()
    }

    private func stopMessagePolling() {
        stateLock.lock()
        let timer = _messageTimer
        _messageTimer = nil
        stateLock.unlock()
        timer?.cancel()
    }

    /// The keepalive ping, armed under the same rule as the poll above.
    ///
    /// Gating it here is what lets the reconnect edge call this
    /// unconditionally and still honour a pause — and it is why `pause()`
    /// stopping the ping is now the whole story rather than half of it. An
    /// earlier revision left the reconnect edge restarting the ping on the
    /// grounds that an unpinged paused socket is an undetectable zombie; that
    /// argument applies at least as strongly to a socket already connected
    /// when the pause landed, whose ping `pause()` stops and never restarts,
    /// so it bought nothing and only diverged from `InternetManager`, which
    /// gates both timers together.
    private func startPingTimer() {
        stateLock.lock()
        guard !_isPaused else {
            stateLock.unlock()
            return
        }
        let previous = _pingTimer
        let timer = DispatchSource.makeTimerSource(queue: connectionQueue)
        _pingTimer = timer
        stateLock.unlock()

        previous?.cancel()
        timer.schedule(deadline: .now() + PING_INTERVAL, repeating: PING_INTERVAL)
        timer.setEventHandler { [weak self] in
            guard let self = self, !self.isPaused else { return }
            self.sendPings()
        }
        timer.resume()
    }

    private func stopPingTimer() {
        stateLock.lock()
        let timer = _pingTimer
        _pingTimer = nil
        stateLock.unlock()
        timer?.cancel()
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

    /// Claims `.running` for a relay that has just come up, but only if the
    /// transport has not begun stopping. Returns false when it has, so the
    /// caller can close the connection it opened instead of publishing a state
    /// a concurrent `stop()` has already moved past.
    ///
    /// One operation rather than a `guard` followed by an `updateState`, for
    /// the reason spelled out on `ReticulumManager.markRunningIfLive`: those
    /// are two separate [stateLock] acquisitions, and a `stop()` landing
    /// between them leaves a torn-down transport wedged at `.running`.
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
