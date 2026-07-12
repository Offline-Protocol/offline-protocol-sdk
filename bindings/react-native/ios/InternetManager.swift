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
    // Lock-guarded (stateLock) like the Kotlin bridge's @Volatile state:
    // written on main (updateState) but read from messageQueue, the
    // URLSession delegate queue, and RN threads (the module reads it in
    // enableTransport).
    private var _state: TransportState = .unavailable
    public private(set) var state: TransportState {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _state }
        set { stateLock.lock(); defer { stateLock.unlock() }; _state = newValue }
    }
    public weak var delegate: TransportManagerDelegate?
    
    // MARK: - Constants
    
    private let MESSAGE_POLL_INTERVAL: TimeInterval = 0.1 // 100ms
    private let RECONNECT_INITIAL_DELAY: TimeInterval = 1.0
    private let RECONNECT_MAX_DELAY: TimeInterval = 30.0
    private let RECONNECT_BACKOFF_MULTIPLIER: Double = 2.0
    private let PING_INTERVAL: TimeInterval = 10.0  // Reduced from 30s for faster failure detection
    private let CONNECTION_TIMEOUT: TimeInterval = 10.0
    private let AUTH_RESPONSE_TIMEOUT: TimeInterval = 10.0
    // Tracker ids for app-authored raw SendMessage frames (sendRawCommand):
    // recorded to keep the per-recipient FIFO honest, never reported to the
    // core. Mirrors InternetManager.kt — keep in sync.
    private static let rawSendSentinelPrefix = "raw:"
    
    // MARK: - Properties
    
    private let protocolInstance: OfflineProtocol
    private let deviceId: String

    // Connection/configuration state. Kotlin marks the equivalents
    // AtomicBoolean/@Volatile; Swift has no volatile, and an unsynchronized
    // cross-thread read of even a trivial var is a data race — so each field
    // is a lock-guarded accessor over a private stored var (same pattern as
    // webSocketTask below). Writes stay on main (the lifecycle entry points
    // and the close funnel); reads are safe from messageQueue, the URLSession
    // delegate queue, and RN threads. stateLock is never held across a call
    // out (delegate, protocol, socket), only around the raw load/store.
    private let stateLock = NSLock()

    private var _authToken: String? = nil
    private var authToken: String? {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _authToken }
        set { stateLock.lock(); defer { stateLock.unlock() }; _authToken = newValue }
    }
    private var _serverUrl: URL?
    private var serverUrl: URL? {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _serverUrl }
        set { stateLock.lock(); defer { stateLock.unlock() }; _serverUrl = newValue }
    }
    
    // WebSocket components.
    // Written ONLY on main (connect/disconnect via runOnMainSync, plus
    // teardownSocket and the terminal close callbacks, which all hop to
    // main); read from the URLSession delegate queue, messageQueue, and RN
    // threads (sendRawCommand, checkPresence). The single-writer rule is
    // what makes the compare-then-detach in teardownSocket and the close
    // funnel race-free. Unlike the Kotlin bridge's `@Volatile webSocket`,
    // Swift has no volatile: an unsynchronized read of a strong var races
    // the writer's release (load + retain of a freed object), so every
    // access goes through the lock — reads stay cheap, the single-writer
    // rule is unchanged.
    private let socketLock = NSLock()
    private var _webSocketTask: URLSessionWebSocketTask?
    private var webSocketTask: URLSessionWebSocketTask? {
        get { socketLock.lock(); defer { socketLock.unlock() }; return _webSocketTask }
        set { socketLock.lock(); defer { socketLock.unlock() }; _webSocketTask = newValue }
    }
    private var _urlSession: URLSession?
    private var urlSession: URLSession? {
        get { socketLock.lock(); defer { socketLock.unlock() }; return _urlSession }
        set { socketLock.lock(); defer { socketLock.unlock() }; _urlSession = newValue }
    }

    // Message polling
    private var messageTimer: DispatchSourceTimer?
    private var pingTimer: DispatchSourceTimer?
    private let messageQueue = DispatchQueue(label: "com.offlineprotocol.internet.messages")
    
    // Reconnection. reconnectAttempts is lock-guarded (stateLock): written
    // on main, read by getMetrics() from the caller's thread — mirrors the
    // Kotlin bridge's AtomicInteger. currentReconnectDelay stays a plain var:
    // unlike Kotlin (whose handleAuthenticated runs on the reader thread),
    // every touch here is on main.
    private var _reconnectAttempts: Int = 0
    private var reconnectAttempts: Int {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _reconnectAttempts }
        set { stateLock.lock(); defer { stateLock.unlock() }; _reconnectAttempts = newValue }
    }
    private var currentReconnectDelay: TimeInterval = 1.0
    private var reconnectWorkItem: DispatchWorkItem?
    // Auth watchdog (mirrors the Kotlin bridge's authTimeoutRunnable):
    // main-owned like reconnectWorkItem; armed on socket open, cancelled on
    // Authenticated and by every close/teardown path.
    private var authTimeoutWorkItem: DispatchWorkItem?
    private var maxReconnectAttempts: Int = 0 // 0 = infinite
    private var autoReconnect: Bool = true

    // State tracking. Lock-guarded (stateLock) like the Kotlin bridge's
    // AtomicBoolean/@Volatile fields: written on main (open/close/lifecycle),
    // read from messageQueue (poll ticks, drains), the URLSession delegate
    // queue (send completions), and RN threads (sendRawCommand,
    // checkPresence, getMetrics).
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
    private var _isAuthenticated = false
    private var isAuthenticated: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isAuthenticated }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isAuthenticated = newValue }
    }
    // True between pause() and resume(): a background reconnect must not
    // restart the poll/ping/presence timers the app paused.
    private var _isPaused = false
    private var isPaused: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isPaused }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isPaused = newValue }
    }
    private var transportStartAt: Date?
    
    // Failure tracking for DORS. Atomic (lock-guarded): send/ping completions
    // mutate on the URLSession delegate queue while handleConnectionOpened
    // resets on main — mirrors the Kotlin bridge's AtomicInteger.
    private let consecutiveSendFailures = AtomicCounter()
    private let consecutivePingFailures = AtomicCounter()
    private let MAX_CONSECUTIVE_FAILURES: Int64 = 2  // Trigger disconnect after 2 consecutive failures

    // Correlates the relay's recipient-keyed failure signals (DeliveryError /
    // ConnectionRequestError carry no message_id) back to in-flight sends.
    private let inFlightTracker = RecipientInFlightTracker()

    // Which peers to query via CheckPresence, and how many per tick.
    private let presenceWatch = PresenceWatchPolicy()
    private var presenceWatchTimer: DispatchSourceTimer?

    // Timer lifecycles are invoked from several contexts (the URLSession
    // delegate queue via handleAuthenticated, RN threads via
    // stop/pause/resume, main via handleConnectionClosed); the lock makes
    // each swap atomic so two concurrent starts can't leak a live timer.
    // One lock covers all three timers.
    private let timerLock = NSLock()

    // Translates core-tagged server-plane control frames (controlOp on
    // InternetMessage) into relay-native ops. `let`, not `lazy var`: first
    // touch could otherwise race between messageQueue (translate) and the
    // URLSession delegate queue (onGroupError) — lazy init is unsynchronized.
    private let controlOpTranslator: RelayControlOpTranslator

    // Client-side mirror of the relay's token bucket: every relay-bound
    // frame takes a token before the socket write (a server-side drop after
    // a "successful" local write is invisible to the sender).
    private let rateLimiter = RelayRateLimiter()

    /// Cached mach timebase for monotonicNowMs (constant for the process).
    private static let machTimebase: mach_timebase_info_data_t = {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        return info
    }()

    /// Time source for the rate limiter, the in-flight tracker, and the
    /// presence watch policy: monotonic AND sleep-inclusive
    /// (mach_continuous_time — the true analogue of the Kotlin bridge's
    /// SystemClock.elapsedRealtime), so a wall-clock step (NTP correction,
    /// manual change) can never freeze or over-mint token refill,
    /// mass-expire in-flight sends, or evict the whole watch set, and a
    /// device-sleep interval still refills the bucket instead of pausing it
    /// (mach_absolute_time-based clocks stop ticking during sleep). Every
    /// call into those three must use this — mixing time sources per call
    /// site would look like clock jumps to their TTLs.
    private func monotonicNowMs() -> Int64 {
        let timebase = Self.machTimebase
        let ticks = mach_continuous_time()
        // Split multiply-then-divide so ticks * numer can't overflow UInt64;
        // the sub-tick truncation is nanoseconds-scale, irrelevant at ms.
        let numer = UInt64(timebase.numer)
        let denom = UInt64(timebase.denom)
        let nanos = (ticks / denom) * numer + (ticks % denom) * numer / denom
        return Int64(nanos / 1_000_000)
    }

    /// Control-op frames deferred by the rate limiter, drained (oldest
    /// first) at the start of each poll tick. A translation's commit closure
    /// runs only after its LAST frame's send completion succeeded (iOS
    /// completions are async — the drain sends one frame at a time).
    /// messageQueue-confined; cleared on disconnect/stop/RateLimited — the
    /// frames are per-connection and their commits are generation-guarded
    /// anyway.
    private final class PendingControlFrames {
        let controlOp: String
        var frames: [[String: Any]]
        let commit: (() -> Void)?

        init(controlOp: String, frames: [[String: Any]], commit: (() -> Void)?) {
            self.controlOp = controlOp
            self.frames = frames
            self.commit = commit
        }
    }

    private var pendingControlFrames: [PendingControlFrames] = []

    /// True while a deferred frame's send completion is outstanding — the
    /// poll loop must not pull new messages mid-chain. messageQueue-confined.
    private var isDrainingControlFrames = false

    /// Count of `.replace` primary sends whose completion is outstanding.
    /// Their delta frames and commit are handed off only from the SUCCESS
    /// completion (Kotlin's synchronous send gets that ordering for free),
    /// so until the completion lands the poll loop must not pull more
    /// messages: a same-group re-register would translate against an
    /// uncommitted diff base. A counter, not a flag: the disconnect paths
    /// never touch it, and URLSession always fires the completion (even for
    /// a cancelled task), so it can never wedge — a stale completion cannot
    /// falsely release a newer primary's hold. messageQueue-confined.
    private var inFlightControlPrimaries = 0

    /// Receives raw relay frames that are app/server concerns rather than SDK
    /// concerns (invite links, role changes, rate limiting, unknown types) —
    /// the module forwards them to JS as the `internet_server_message` event.
    public var serverMessageEmitter: ((String) -> Void)?
    
    // Metrics. Atomic (lock-guarded): send completions mutate on the
    // URLSession delegate queue, receive paths too, and getMetrics() reads
    // from the caller's thread — mirrors the Kotlin bridge's AtomicLong
    // metrics.
    private let bytesSent = AtomicCounter()
    private let bytesReceived = AtomicCounter()
    private let messagesSent = AtomicCounter()
    private let messagesReceived = AtomicCounter()
    
    // MARK: - Initialization
    
    public init(protocol protocolInstance: OfflineProtocol, deviceId: String, serverUrl: String? = nil) {
        self.protocolInstance = protocolInstance
        self.deviceId = deviceId
        self.controlOpTranslator = RelayControlOpTranslator(selfId: deviceId)
        if let urlString = serverUrl, let url = URL(string: urlString) {
            // Stored var, not the guarded accessor: computed setters cannot
            // run before super.init(), and no other thread can see self yet.
            self._serverUrl = url
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
        
        // autoReconnect / maxReconnectAttempts are main-owned plain vars
        // (scheduleReconnect and handleConnectionClosed read them on main);
        // configure() arrives on an RN thread, so hop like the lifecycle
        // entry points instead of racing those readers.
        runOnMainSync {
            self.serverUrl = url
            self.autoReconnect = autoReconnect
            self.maxReconnectAttempts = maxReconnectAttempts
        }

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
    
    // The four lifecycle entry points run their whole bodies on main
    // (runOnMainSync, matching the Kotlin bridge): they mutate state that
    // scheduleReconnect / handleConnectionClosed / the close funnel also
    // mutate on main (reconnectWorkItem, urlSession, isPaused, state), and
    // an RN-thread writer would race them.
    public func start() throws {
        var thrown: Error?
        runOnMainSync {
            do {
                try startOnMain()
            } catch {
                thrown = error
            }
        }
        if let error = thrown {
            throw error
        }
    }

    private func startOnMain() throws {
        // .starting rejects too: a second start mid-connect would otherwise
        // replace urlSession below and orphan the connecting session (which
        // strongly retains self as its delegate) until process exit.
        guard state != .running && state != .starting else {
            throw TransportError.alreadyRunning
        }
        guard state != .stopping else {
            throw TransportError.invalidState("Transport is stopping")
        }

        // Self-stop paths (max-reconnect-attempts, autoReconnect=false close)
        // set .stopped WITHOUT releasing the session — only stop() does — and
        // a push-triggered re-enable skips stop() for a non-running manager.
        // Always release the previous session before minting a new one, or
        // the orphan retains this manager until process exit.
        urlSession?.invalidateAndCancel()
        urlSession = nil

        guard let url = serverUrl else {
            throw TransportError.notAvailable("Server URL not configured. Call configure(serverUrl:) first.")
        }

        emitDiagnostic("info", "Starting Internet transport", context: [
            "deviceId": deviceId,
            "serverUrl": url.absoluteString
        ])

        updateState(.starting)
        transportStartAt = Date()

        // An explicit start() means "run": a pause() from a previous session
        // must not leave this fresh transport authenticated-but-mute (e.g.
        // pause → stop → push-triggered enableTransport, which would
        // otherwise skip the poll/ping/presence timers on Authenticated).
        // The reconnect backoff is likewise per-session state: a stale 30s
        // delay must not slow the first retry of a brand-new start.
        isPaused = false
        reconnectAttempts = 0
        currentReconnectDelay = RECONNECT_INITIAL_DELAY

        // Create URL session
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = CONNECTION_TIMEOUT
        config.waitsForConnectivity = true
        urlSession = URLSession(configuration: config, delegate: self, delegateQueue: nil)

        // Connect to WebSocket
        connect()
    }

    public func stop() {
        runOnMainSync {
            stopOnMain()
        }
    }

    private func stopOnMain() {
        // Even a transport that already stopped itself (e.g. after
        // max-reconnect-attempts set .stopped) still holds a URLSession that
        // retains its delegate (self) plus per-connection state; stop() must
        // always release those instead of early-returning and leaking the
        // session until process exit.
        let wasActive = state == .running || state == .starting

        if wasActive {
            updateState(.stopping)
        }

        // Cancel reconnect attempts
        reconnectWorkItem?.cancel()
        reconnectWorkItem = nil

        // Stop timers
        stopMessagePolling()
        stopPingTimer()
        // The presence watch is otherwise only stopped by
        // handleConnectionClosed; if the close callback never fires (task
        // already nil, cancel racing the pending receive) the timer would
        // tick forever.
        stopPresenceWatch()

        // Close WebSocket
        disconnect()

        // Per-connection state must not survive a stop()/start() cycle:
        // disconnect() detaches the task before cancelling it, so the close
        // callbacks are suppressed as stale and handleConnectionClosed's
        // clear/reset never runs for this path.
        inFlightTracker.clear()
        controlOpTranslator.reset()
        messageQueue.async { [weak self] in
            self?.pendingControlFrames.removeAll()
        }
        // The watch set survives *reconnects* on purpose (pending traffic is
        // still pending), but an explicit stop() ends the session: without
        // this, a stop/start cycle spends up to the idle TTL of CheckPresence
        // tokens on the previous session's peers.
        presenceWatch.clear()

        if wasActive {
            // Notify protocol
            try? protocolInstance.internetStatusChanged(isConnected: false)
        }

        // URLSession retains its delegate (self) until invalidated; without
        // this, deinit is unreachable and every start() after stop() leaks a
        // session.
        urlSession?.invalidateAndCancel()
        urlSession = nil

        if wasActive {
            updateState(.stopped)
        }
        emitDiagnostic("info", "Internet transport stopped")
    }

    public func pause() {
        runOnMainSync {
            // The flag makes the pause durable: a background network blip
            // reconnects and re-authenticates, and handleAuthenticated must
            // not restart the timers the app paused.
            isPaused = true
            stopMessagePolling()
            stopPingTimer()
            // A backgrounded app must not keep spending battery and relay
            // rate-limit budget on CheckPresence ticks; parked welcomes
            // re-arm from the watch loop after resume().
            stopPresenceWatch()
            // Final drain: flush messages already queued in the Rust queue
            // (still marked Available to DORS) instead of leaving them
            // stranded until resume(). The module pauses the core right
            // after the transports, so the remaining window — a send racing
            // pause() itself — is bounded to sends already in flight.
            // pollAndSendMessages and the control-frame drain state are
            // messageQueue-confined, and a cancelled poll timer's in-flight
            // tick may still be running there — hop onto messageQueue like
            // stop() does. pause() leaves the socket up, so the drain
            // landing after this block returns is safe (and it re-checks
            // isConnected itself).
            if state == .running && isConnected {
                messageQueue.async { [weak self] in
                    self?.pollAndSendMessages()
                }
            }
        }
    }

    public func resume() {
        runOnMainSync {
            isPaused = false
            if state == .running && isConnected {
                startMessagePolling()
                startPingTimer()
                startPresenceWatch()
            }
        }
    }

    public func getMetrics() -> [String: Any] {
        return [
            "bytes_sent": bytesSent.get(),
            "bytes_received": bytesReceived.get(),
            "messages_sent": messagesSent.get(),
            "messages_received": messagesReceived.get(),
            "is_connected": isConnected,
            "is_authenticated": isAuthenticated,
            "reconnect_attempts": reconnectAttempts
        ]
    }
    
    // MARK: - Connection Management

    /// Runs `action` on main, synchronously. webSocketTask is written only
    /// on main; lifecycle entry points (start/stop via RN threads) hop here
    /// so every write honors the single-writer rule — and stop() must
    /// observe the detach before invalidating the session. Mirrors the
    /// Kotlin bridge's runOnMainSync.
    private func runOnMainSync(_ action: () -> Void) {
        if Thread.isMainThread {
            action()
        } else {
            DispatchQueue.main.sync(execute: action)
        }
    }

    private func connect() {
        runOnMainSync {
            guard let url = serverUrl else { return }
            guard !isConnecting && !isConnected else { return }
            // stop() may have run between a reconnect being scheduled and
            // firing. The session null-check must precede the isConnecting
            // latch: with no session there is no callback to ever clear the
            // flag, and every future connect() would early-return — a wedged
            // transport.
            guard let session = urlSession else { return }

            isConnecting = true

            // Create WebSocket task
            var request = URLRequest(url: url)
            request.timeoutInterval = CONNECTION_TIMEOUT
            // Add device ID header for identification
            request.setValue(deviceId, forHTTPHeaderField: "X-Device-ID")

            let task = session.webSocketTask(with: request)
            webSocketTask = task
            task.resume()

            emitDiagnostic("info", "Connecting to WebSocket", context: [
                "url": url.absoluteString
            ])

            // Start receiving messages
            receiveMessage()
        }
    }

    private func disconnect() {
        runOnMainSync {
            // Detach before cancel so the cancel-triggered delegate/receive
            // callbacks see a stale task and no-op (see isStale).
            let task = webSocketTask
            webSocketTask = nil
            task?.cancel(with: .goingAway, reason: nil)
            cancelAuthTimeout()
            isConnected = false
            isConnecting = false
            isAuthenticated = false
        }
    }

    /// True when the callback belongs to a socket task that is no longer the
    /// manager's current one (replaced by a reconnect or detached by a
    /// teardown/close). A stale task's terminal callbacks must not clear the
    /// in-flight tracker, reset the translator, or report the transport down
    /// while a newer, healthy connection is live; a stale task's send/ping
    /// outcomes must not touch failure counters or trigger teardown.
    /// webSocketTask is written only on main (single-writer rule); reads
    /// from other queues are the same best-effort compare the Kotlin
    /// listener's stale-socket guard does.
    private func isStale(_ task: URLSessionTask) -> Bool {
        return task !== webSocketTask
    }

    /// Cancels and detaches `task` IF it is still the current socket, then
    /// runs the closed handler exactly once for it. Detaching before cancel
    /// makes the cancel-triggered delegate/receive callbacks no-ops (the
    /// stale-task guard), so a dead socket can never tear down the
    /// connection rebuilt after it. Scoping the teardown to the task that
    /// observed the failure — and running it on main, the only thread that
    /// writes webSocketTask — closes the reverse race too: a stale path
    /// (late AuthError, queued send-failure teardown) can never cancel a
    /// newer, healthy socket. Mirrors the Kotlin bridge's teardownSocket.
    private func teardownSocket(ifCurrent task: URLSessionWebSocketTask, reason: String) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self, task === self.webSocketTask else { return }
            self.webSocketTask = nil
            task.cancel(with: .goingAway, reason: nil)
            self.handleConnectionClosed(error: NSError(
                domain: "OfflineProtocol.InternetManager",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: reason]
            ))
        }
    }

    private func handleConnectionOpened(task: URLSessionWebSocketTask) {
        isConnected = true
        isConnecting = false
        isAuthenticated = false
        // Backoff deliberately NOT reset here: only a full authenticate
        // proves the connection good (handleAuthenticated). Resetting on
        // TCP open would let a persistently bad token cycle
        // connect → AuthError → teardown at the initial 1s delay forever,
        // hammering the relay.
        consecutiveSendFailures.set(0)
        consecutivePingFailures.set(0)

        emitDiagnostic("info", "WebSocket connected, authenticating...", context: [
            "serverUrl": serverUrl?.absoluteString ?? "unknown"
        ])

        // A relay that opens the socket but never answers must not wedge
        // the transport (isConnected=true short-circuits connect() and the
        // timers only start on Authenticated). URLSession's request timeout
        // is no substitute: any inbound frame resets it, so a half-alive
        // relay that pings but never authenticates slips past it.
        scheduleAuthTimeout(for: task)

        // Authenticate with the relay server using deviceId as the user ID
        sendAuthentication()
    }

    /// Arms the auth watchdog for `task` (mirrors the Kotlin bridge's
    /// scheduleAuthTimeout). Main-thread only. Fires through the close
    /// funnel via teardownSocket; cancelled on Authenticated and by every
    /// close/teardown path.
    private func scheduleAuthTimeout(for task: URLSessionWebSocketTask) {
        cancelAuthTimeout()
        let workItem = DispatchWorkItem { [weak self] in
            guard let self = self else { return }
            self.authTimeoutWorkItem = nil
            guard task === self.webSocketTask, !self.isAuthenticated else { return }
            self.emitDiagnostic("error", "No auth response from relay within timeout", context: [
                "timeoutMs": Int(self.AUTH_RESPONSE_TIMEOUT * 1000)
            ])
            self.teardownSocket(ifCurrent: task, reason: "Auth response timeout")
        }
        authTimeoutWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + AUTH_RESPONSE_TIMEOUT, execute: workItem)
    }

    /// Main-thread only.
    private func cancelAuthTimeout() {
        authTimeoutWorkItem?.cancel()
        authTimeoutWorkItem = nil
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
    
    /// Hops to main before mutating: this is called from the URLSession
    /// delegate queue (receive loop → dispatchRelayFrame), but
    /// reconnectAttempts / currentReconnectDelay / isPaused / state are
    /// main-owned (scheduleReconnect, the lifecycle entry points) — the
    /// Kotlin bridge makes this exact pair atomic; Swift's equivalent is
    /// single-queue confinement. The task-identity guard drops a late
    /// Authenticated from a socket that was already replaced or torn down —
    /// it must not mark the transport running or start timers for a
    /// connection that no longer exists.
    private func handleAuthenticated(userId: String, username: String, task: URLSessionWebSocketTask) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self, !self.isStale(task) else { return }
            self.handleAuthenticatedOnMain(userId: userId, username: username)
        }
    }

    private func handleAuthenticatedOnMain(userId: String, username: String) {
        isAuthenticated = true
        cancelAuthTimeout()
        // The relay accepted us — this, not the TCP open, is what proves the
        // connection good and earns a backoff reset.
        reconnectAttempts = 0
        currentReconnectDelay = RECONNECT_INITIAL_DELAY

        updateState(.running)

        // Notify protocol - this will trigger outbox flush for pending messages
        try? protocolInstance.internetStatusChanged(isConnected: true)

        // Start polling, pinging, and the presence watch — unless the app
        // paused the transport; a background reconnect must stay quiet and
        // resume() restarts the timers.
        if !isPaused {
            startMessagePolling()
            startPingTimer()
            startPresenceWatch()

            // Immediately poll for messages to flush outbox after reconnection
            // This ensures messages queued during disconnection are sent promptly
            messageQueue.async { [weak self] in
                self?.pollAndSendMessages()
            }
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

        // The dead socket's auth watchdog must not fire into (or outlive)
        // whatever connection replaces it.
        cancelAuthTimeout()

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
        // Deferred frames belong to the dead connection; their commits are
        // generation-dead after the reset above.
        messageQueue.async { [weak self] in
            self?.pendingControlFrames.removeAll()
        }

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
        // A close can race stop(): its scheduled reconnect must not revive a
        // transport the app already stopped (the delayed connect() would
        // find urlSession nilled and leave the transport wedged).
        guard state != .stopping && state != .stopped else { return }
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
        guard let task = webSocketTask else { return }
        receiveLoop(task)
    }

    private func receiveLoop(_ task: URLSessionWebSocketTask) {
        task.receive { [weak self] result in
            guard let self = self else { return }
            // A stale task's completion must neither tear down the
            // connection rebuilt after it nor re-arm a second receive loop
            // onto the new task (see isStale).
            guard !self.isStale(task) else { return }

            switch result {
            case .success(let message):
                self.handleReceivedMessage(message, task: task)
                // Continue receiving — on the CAPTURED task, never the
                // current one: a completion that passed the staleness guard
                // just before a stop/start swap must die with its own task
                // (the guard above catches it next round), not migrate its
                // loop onto the replacement socket alongside connect()'s
                // own receiveMessage() — two loops on one task.
                self.receiveLoop(task)

            case .failure(let error):
                self.handleSocketTerminated(task, error: error)
            }
        }
    }

    /// The single task-scoped close funnel. URLSession fires 2-3 terminal
    /// signals per disconnect (receive-loop failure, didCloseWith,
    /// didCompleteWithError); each hops to main, checks the task is still
    /// current, detaches FIRST, then runs handleConnectionClosed. After the
    /// first entry detaches, the later duplicates fail the identity check
    /// and become no-ops — otherwise every disconnect would inflate
    /// reconnectAttempts (and the backoff delay) 2-3x.
    private func handleSocketTerminated(_ task: URLSessionTask, error: Error?) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self, task === self.webSocketTask else { return }
            self.webSocketTask = nil
            self.handleConnectionClosed(error: error)
        }
    }

    private func handleReceivedMessage(_ message: URLSessionWebSocketTask.Message, task: URLSessionWebSocketTask) {
        switch message {
        case .data(let data):
            processReceivedData(data, task: task)
        case .string(let text):
            if let data = text.data(using: .utf8) {
                processReceivedData(data, task: task)
            }
        @unknown default:
            emitDiagnostic("warning", "Unknown WebSocket message type")
        }
    }

    private func processReceivedData(_ data: Data, task: URLSessionWebSocketTask) {
        bytesReceived.add(Int64(data.count))

        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let messageType = json["type"] as? String,
              let rawText = String(data: data, encoding: .utf8) else {
            emitDiagnostic("warning", "Received non-JSON or invalid message", context: [
                "size": data.count
            ])
            return
        }

        // One malformed frame must degrade to a diagnostic, never propagate.
        // The Kotlin bridge wraps this dispatch in a catch; Swift dictionary
        // casts return nil instead of throwing, so the per-field guards (and
        // per-entry compactMaps) inside the dispatch are the equivalent
        // containment.
        dispatchRelayFrame(json, messageType: messageType, task: task, rawText: rawText)
    }

    /// `rawText` is the frame exactly as it arrived; it — not a
    /// re-serialized `json` — is what `internet_server_message` forwards,
    /// per the TS contract ("the verbatim relay frame"). JSONSerialization
    /// reorders keys and canonicalizes numbers (25.0 -> 25), the same
    /// reason sendRawCommand refuses to re-serialize outbound frames.
    private func dispatchRelayFrame(_ json: [String: Any], messageType: String, task: URLSessionWebSocketTask, rawText: String) {
        switch messageType {
        case "Authenticated":
            // Handle authentication success
            let userId = json["user_id"] as? String ?? deviceId
            let username = json["username"] as? String ?? deviceId
            handleAuthenticated(userId: userId, username: username, task: task)
            
        case "AuthError":
            // Handle authentication error
            let reason = json["reason"] as? String ?? "Unknown error"
            emitDiagnostic("error", "Authentication failed", context: [
                "reason": reason
            ])
            // The auth-failed socket must actually be closed — left open,
            // its eventual close callback would race the reconnect's fresh
            // connection (the teardown detaches it first, so the
            // cancel-triggered callbacks are ignored as stale).
            teardownSocket(ifCurrent: task, reason: reason)
            
        case "MessageSent":
            // Handle MessageSent event from WebSocket server
            // This contains the server-generated message_id that we should use
            let sentRecipient = json["recipient"] as? String ?? ""
            let sentMessageId = json["message_id"] as? String

            // The relay accepted this frame (forwarded, or FCM-poked an
            // offline recipient) — either way it is no longer in flight and
            // must not be swept into a later recipient-keyed DeliveryError.
            // NOT a delivery signal (the poke case), so the recipient is
            // deliberately not unwatched here.
            if !sentRecipient.isEmpty {
                inFlightTracker.resolveOnRelayAccepted(
                    recipient: sentRecipient,
                    messageId: (sentMessageId?.isEmpty == false) ? sentMessageId : nil,
                    nowMs: monotonicNowMs()
                )
            }

            if let messageId = sentMessageId, !messageId.isEmpty {
                let timestamp = json["timestamp"] as? String ?? ""

                // The server has confirmed the message was sent with this message_id
                // We need to notify the protocol so it can update the message ID
                // The protocol will emit a message_sent event with this server-generated ID
                emitDiagnostic("debug", "MessageSent from relay server", context: [
                    "messageId": messageId,
                    "recipient": sentRecipient,
                    "timestamp": timestamp
                ])
                // Note: The protocol SDK will handle the message_sent event internally
                // The frontend will receive it via the normal event stream
            }
            
        case "MessageReceived":
            // Handle incoming direct message. Only the sender is required;
            // a missing content degrades to "" (Kotlin parity) instead of
            // dropping the frame.
            guard let senderId = json["sender"] as? String, !senderId.isEmpty else {
                emitDiagnostic("warning", "Invalid MessageReceived format")
                return
            }
            let content = json["content"] as? String ?? ""

            let replyToMsg = json["reply_to_msg"] as? String
            let messageId = json["message_id"] as? String
            let timestampStr = json["timestamp"] as? String ?? ""

            messagesReceived.increment()
            
            messageQueue.async { [weak self] in
                guard let self = self else { return }
                
                do {
                    // The protocol expects the full serialized Message JSON bytes
                    // The WebSocket server sends the message content, which should be the full Message JSON
                    // But we need to ensure reply_to_msg and message_id are included if present
                    var messageData: Data
                    var isFullMessage = false
                    // The SDK-level content inside the Message, for the
                    // server-plane firewall below.
                    var innerContent = content

                    // Try to parse content as JSON to see if it's already a full Message
                    if let contentData = content.data(using: .utf8),
                       let contentJson = try? JSONSerialization.jsonObject(with: contentData) as? [String: Any],
                       contentJson["sender"] != nil && contentJson["recipient"] != nil {
                        // It's already a full Message JSON
                        isFullMessage = true
                        innerContent = contentJson["content"] as? String ?? ""
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
                        // (legacy JS-relay senders). LegacyRelayMessage carries
                        // every field the Rust Message deserializer requires —
                        // a missing id/timestamp or non-lowercase priority
                        // makes the transport silently drop the frame.
                        let messageDict = LegacyRelayMessage.buildDict(
                            senderId: senderId,
                            recipientId: self.deviceId, // Will be corrected by protocol
                            content: content,
                            timestampMs: self.parseTimestampToMs(timestampStr),
                            messageId: messageId,
                            replyToMsg: replyToMsg
                        )
                        messageData = try JSONSerialization.data(withJSONObject: messageDict)
                    }
                    
                    // Server-plane firewall: peers must never originate
                    // relay-answer frames (__GROUP_CREATED__ & co.). The
                    // relay forwards content verbatim, and the core trusts
                    // these answers from the internet path — one forged
                    // GroupCreated could mark a group relay-synced against a
                    // relay that never registered it. Legitimate answers
                    // enter via injectGroupInternalMessage, not this path.
                    if RelayControlOpTranslator.isForgedServerPlaneAnswer(innerContent) {
                        self.emitDiagnostic("warning", "Dropped peer frame impersonating a relay server answer", context: [
                            "senderId": senderId,
                            "prefix": String(innerContent.prefix(32))
                        ])
                        return
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
            handleRecipientUnreachable(
                recipient: recipient,
                reason: reason,
                plane: .data,
                source: "DeliveryError"
            )

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
                    return RelayTimestamps.parseToMsOrNull(str)
                }
                if let num = json["last_seen"] as? NSNumber,
                   CFGetTypeID(num) != CFBooleanGetTypeID() {
                    return RelayTimestamps.normalizeEpochToMs(num.int64Value)
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
            // A typing peer is provably online — same inbound-proof rule as
            // MessageReceived: stop spending CheckPresence slots on it.
            presenceWatch.unwatch(typingUserId)
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
                    let messageData = try self.buildInternalMessageData(senderId: senderId, content: content)
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
                    let messageData = try self.buildInternalMessageData(senderId: acceptedBy, content: content)
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
                    let messageData = try self.buildInternalMessageData(senderId: rejectedBy, content: content)
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
            handleRecipientUnreachable(
                recipient: recipient,
                reason: reason,
                plane: .connReq,
                source: "ConnectionRequestError"
            )
            
        case "GroupCreated":
            guard let groupId = json["group_id"] as? String, !groupId.isEmpty else { return }
            let name = json["name"] as? String ?? ""
            // A success answer on the group channel closes the translator's
            // admin-denial correlation window — without this only errors
            // close it, and it would stay armed for the rest of the
            // connection after a successful register.
            controlOpTranslator.onGroupAnswered(groupId: groupId)
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
            controlOpTranslator.onGroupAnswered(groupId: groupId)
            let userId = json["user_id"] as? String ?? ""
            let addedBy = json["added_by"] as? String ?? ""
            injectGroupInternalMessage(senderId: addedBy.isEmpty ? "relay" : addedBy, prefix: "__GROUP_MEMBER_ADDED__", payload: ["group_id": groupId, "user_id": userId, "added_by": addedBy])
            
        case "GroupMemberRemoved":
            guard let groupId = json["group_id"] as? String, !groupId.isEmpty else { return }
            controlOpTranslator.onGroupAnswered(groupId: groupId)
            let userId = json["user_id"] as? String ?? ""
            let removedBy = json["removed_by"] as? String ?? ""
            injectGroupInternalMessage(senderId: removedBy.isEmpty ? "relay" : removedBy, prefix: "__GROUP_MEMBER_REMOVED__", payload: ["group_id": groupId, "user_id": userId, "removed_by": removedBy])
            
        case "GroupInfo":
            guard let groupId = json["group_id"] as? String, !groupId.isEmpty else { return }
            let name = json["name"] as? String ?? ""
            let createdBy = json["created_by"] as? String ?? ""
            let createdAt = json["created_at"] as? String ?? ""
            let membersRaw = json["members"] as? [Any] ?? []
            // Per-entry tolerance (parity with the Kotlin bridge): one
            // non-object element or an entry without a user_id is skipped,
            // never allowed to throw away the whole member list — and apps
            // must not see phantom members.
            let members: [[String: String]] = membersRaw.compactMap { raw in
                guard let m = raw as? [String: Any],
                      let memberId = m["user_id"] as? String, !memberId.isEmpty else { return nil }
                return ["user_id": memberId, "role": m["role"] as? String ?? "member", "joined_at": m["joined_at"] as? String ?? ""]
            }
            injectGroupInternalMessage(senderId: "relay", prefix: "__GROUP_INFO__", payload: ["group_id": groupId, "name": name, "created_by": createdBy, "created_at": createdAt, "members": members])
            
        case "UserGroups":
            guard let groupsRaw = json["groups"] as? [Any] else { return }
            // Per-entry tolerance (parity with the Kotlin bridge): a
            // malformed element or one without a group_id is skipped, never
            // allowed to nuke the whole groups array.
            let groups: [[String: String]] = groupsRaw.compactMap { raw in
                guard let g = raw as? [String: Any],
                      let gId = g["group_id"] as? String, !gId.isEmpty else { return nil }
                return ["group_id": gId, "name": g["name"] as? String ?? "", "created_at": g["created_at"] as? String ?? ""]
            }
            injectGroupInternalMessage(senderId: "relay", prefix: "__USER_GROUPS__", payload: ["groups": groups])
            
        case "GroupError":
            let reason = json["reason"] as? String ?? "Unknown error"
            let groupId = json["group_id"] as? String ?? ""
            // Admin-denied registration must stop member-delta attempts —
            // but only when the error is ours: the request_id (echoed for
            // app raw-channel ops, never tagged by the translator) lets the
            // translator disown errors that answer someone else's frame.
            controlOpTranslator.onGroupError(
                groupId: groupId,
                reason: reason,
                requestId: json["request_id"] as? String
            )
            var payload: [String: Any] = ["reason": reason]
            // group_id lets the core revoke relay_synced so group sends fall
            // back to per-member delivery.
            if !groupId.isEmpty {
                payload["group_id"] = groupId
            }
            injectGroupInternalMessage(senderId: "relay", prefix: "__GROUP_ERROR__", payload: payload)
            // Dual-emit: apps correlating request_id-carrying errors
            // (invite-link ops ride the raw channel) need the full frame.
            emitServerMessage(rawText)

        case "RateLimited":
            // The relay dropped whatever exceeded the bucket — possibly a
            // member delta whose membership snapshot a commit is about to
            // record. Reset so in-flight commits die (generation guard) and
            // the next register re-derives deltas from scratch; the worst
            // case is an idempotent re-registration. Drain the local bucket
            // too: it was clearly too optimistic.
            //
            // The whole reaction runs as ONE messageQueue block so it
            // serializes with poll ticks: resetting the translator here on
            // the delegate queue while the clear is still queued would let
            // an interleaved tick translate a post-reset register whose
            // delta frames the clear then wipes (their commits never run,
            // and the group's relay membership stays stale until the next
            // register trigger).
            messageQueue.async { [weak self] in
                guard let self = self else { return }
                self.controlOpTranslator.reset()
                self.rateLimiter.drain(nowMs: self.monotonicNowMs())
                self.pendingControlFrames.removeAll()
            }
            emitServerMessage(rawText)
            emitDiagnostic("warning", "Relay rate limit hit — translator state reset")

        case "GroupRoleChanged":
            // A promotion of this device to admin re-enables member deltas
            // an earlier denial suppressed.
            controlOpTranslator.onRoleChanged(
                groupId: json["group_id"] as? String ?? "",
                userId: json["user_id"] as? String ?? "",
                newRole: json["new_role"] as? String ?? json["role"] as? String ?? ""
            )
            emitServerMessage(rawText)
            emitDiagnostic("debug", "Relay server message forwarded", context: [
                "type": messageType
            ])

        // Server-plane frames that are app concerns, not SDK concerns —
        // forwarded verbatim as the internet_server_message event so the
        // invite-link lifecycle and misc server events can ride the SDK's
        // socket without a second WebSocket in the app.
        case "GroupInviteLinkCreated", "GroupInviteLinkRevoked", "GroupJoinedViaInvite",
             "GroupInviteJoinPending", "GroupDeleted":
            emitServerMessage(rawText)
            emitDiagnostic("debug", "Relay server message forwarded", context: [
                "type": messageType
            ])

        default:
            // Unknown types are forwarded too — future relay additions
            // surface to the app instead of being silently dropped.
            emitServerMessage(rawText)
            emitDiagnostic("debug", "Received relay message", context: [
                "type": messageType
            ])
        }
    }

    private func emitServerMessage(_ rawText: String) {
        serverMessageEmitter?(rawText)
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
    
    /// Build serialized Message JSON data for an internal (relay) message,
    /// same shape as MessageReceived. Throws instead of force-trying: the
    /// inputs are plist-safe today, but a crash in the receive path on every
    /// relay group frame is one refactor away — callers' do/catch degrades
    /// it to a diagnostic instead (the Kotlin bridge cannot crash here).
    private func buildInternalMessageData(senderId: String, content: String) throws -> Data {
        let messageDict = LegacyRelayMessage.buildDict(
            senderId: senderId,
            recipientId: deviceId,
            content: content,
            timestampMs: Int64(Date().timeIntervalSince1970 * 1000)
        )
        return try JSONSerialization.data(withJSONObject: messageDict)
    }
    
    /// Inject a group (relay) internal message into the protocol so it emits the corresponding event.
    private func injectGroupInternalMessage(senderId: String, prefix: String, payload: [String: Any]) {
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            do {
                let payloadData = try JSONSerialization.data(withJSONObject: payload)
                guard let payloadStr = String(data: payloadData, encoding: .utf8) else { return }
                let content = prefix + payloadStr
                let messageData = try self.buildInternalMessageData(senderId: senderId, content: content)
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
    
    // Same lock-guarded swap as the presence watch timer (see timerLock):
    // two concurrent starts must not leak a live timer.
    private func startMessagePolling() {
        let timer = DispatchSource.makeTimerSource(queue: messageQueue)
        timer.schedule(deadline: .now(), repeating: MESSAGE_POLL_INTERVAL)
        timer.setEventHandler { [weak self] in
            self?.pollAndSendMessages()
        }
        timerLock.lock()
        let previous = messageTimer
        messageTimer = timer
        timerLock.unlock()
        previous?.cancel()
        timer.resume()
    }

    private func stopMessagePolling() {
        timerLock.lock()
        let timer = messageTimer
        messageTimer = nil
        timerLock.unlock()
        timer?.cancel()
    }
    
    private func pollAndSendMessages() {
        // Double-check connection state to handle race conditions
        // This prevents sending messages right after transport disconnect
        guard isConnected, isAuthenticated else {
            return
        }

        // Monotonic like every tracker/watch call (mirrors the Kotlin
        // bridge's contract): a wall-clock step must never mass-expire
        // in-flight sends or evict the watch set.
        inFlightTracker.prune(nowMs: monotonicNowMs())

        // Deferred control frames first: they are older than anything the
        // queue will hand us and their commits are still pending.
        drainPendingControlFrames()
        // The drain settles asynchronously (iOS send completions); until the
        // spill queue is empty AND the in-flight frame resolved AND no
        // primary's outcome is pending, pulling more messages could
        // interleave frames into a register's delta chain — or translate a
        // same-group re-register against an uncommitted diff base and
        // permanently miss a RemoveGroupMember. The next 100ms tick retries.
        // (The Kotlin bridge gets this ordering for free from OkHttp's
        // synchronous enqueue.)
        guard pendingControlFrames.isEmpty, !isDrainingControlFrames,
              inFlightControlPrimaries == 0 else { return }

        // Timer already runs on messageQueue, no need for extra dispatch
        // Poll for next message from protocol - batch send up to 10 messages per poll
        // to efficiently flush the outbox after reconnection
        // Batch counter — deliberately NOT the messagesSent metric, which the
        // send completions own.
        var batchSent = 0
        let maxBatchSize = 10

        while batchSent < maxBatchSize {
            // Re-check connection state between messages to handle mid-batch disconnects
            guard isConnected, isAuthenticated else {
                emitDiagnostic("warning", "Connection lost mid-batch, stopping message send", context: [
                    "messagesSent": batchSent
                ])
                break
            }

            // Every relay-bound frame takes a token (see RelayRateLimiter):
            // the poll cadence alone could burst 100 frames/s at the relay's
            // 10/s bucket, and over-budget frames are dropped server-side
            // AFTER the local write "succeeded". Out of tokens: leave the
            // rest queued in the core.
            guard rateLimiter.tryAcquire(nowMs: monotonicNowMs()) else {
                break
            }

            guard let message = self.protocolInstance.internetGetNextMessage() else {
                rateLimiter.refund()
                break
            }

            if let controlOp = message.controlOp {
                self.sendControlOp(
                    messageId: message.messageId,
                    recipientId: message.recipientId,
                    controlOp: controlOp,
                    controlPayload: message.controlPayload ?? "",
                    data: Data(message.data),
                    replyToMsg: message.replyToMsg
                )
            } else {
                self.sendMessage(
                    messageId: message.messageId,
                    recipientId: message.recipientId,
                    data: Data(message.data),
                    replyToMsg: message.replyToMsg
                )
            }
            batchSent += 1

            // A control op may have spilled extra frames into the pending
            // queue or have a primary outcome outstanding; stop pulling
            // until its chain settles (same reasoning as the guard above
            // the batch).
            if !pendingControlFrames.isEmpty || isDrainingControlFrames
                || inFlightControlPrimaries > 0 {
                break
            }
        }

        if batchSent > 1 {
            emitDiagnostic("debug", "Batch sent messages", context: [
                "count": batchSent
            ])
        }
    }
    
    private func sendMessage(
        messageId: String,
        recipientId: String,
        data: Data,
        replyToMsg: String? = nil
    ) {
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
            // The poll loop acquired a token for this frame; nothing was
            // written, so return it ("false means defer, never drop").
            rateLimiter.refund()
            // Report failure so DORS metrics stay accurate
            protocolInstance.internetSendFailed(messageId: messageId)
            return
        }

        // Convert data to string content for the relay protocol
        let content = String(data: data, encoding: .utf8) ?? data.base64EncodedString()

        // Wrap in relay server protocol format
        // reply_to_msg is provided directly from the Rust SDK via
        // InternetMessage — the only source, matching the Kotlin bridge.
        var relayMessage: [String: Any] = [
            "type": "SendMessage",
            "recipient": recipientId,
            "content": content
        ]
        if let replyToMsg = replyToMsg, !replyToMsg.isEmpty {
            relayMessage["reply_to_msg"] = replyToMsg
        }

        guard let jsonData = try? JSONSerialization.data(withJSONObject: relayMessage),
              let jsonString = String(data: jsonData, encoding: .utf8) else {
            emitDiagnostic("error", "Failed to create relay message")
            rateLimiter.refund()
            protocolInstance.internetSendFailed(messageId: messageId)
            return
        }

        // Track for recipient-keyed failure correlation BEFORE the write: a
        // fast relay DeliveryError can be processed on the delegate queue
        // before this send's completion runs, and it must find the entry —
        // otherwise the message silently degrades to the slow core
        // confirm-timeout. (Kotlin records synchronously with the write.)
        // The failure completion un-records this exact entry.
        inFlightTracker.recordSent(
            recipient: recipientId,
            messageId: messageId,
            plane: .data,
            nowMs: monotonicNowMs()
        )

        task.send(.string(jsonString)) { [weak self] error in
            guard let self = self else { return }
            // A stale task's send outcome must not increment the failure
            // counter, report internetSendFailed for the message (the
            // disconnect path's fail_all_pending already owned it), or
            // trigger a teardown of the connection rebuilt after it
            // (see isStale). Its optimistic tracker entry died with the
            // connection (handleConnectionClosed/stop clear the tracker).
            guard !self.isStale(task) else { return }

            if let error = error {
                // Failed completion: the frame never hit the wire — return
                // its token (matches the presence/raw/drain paths) and take
                // back the optimistic in-flight entry (this path owns the
                // failure outcome).
                self.rateLimiter.refund()
                self.inFlightTracker.unrecord(recipient: recipientId, messageId: messageId)
                let failures = self.consecutiveSendFailures.increment()
                self.protocolInstance.internetSendFailed(messageId: messageId)
                self.emitDiagnostic("error", "Failed to send WebSocket message", context: [
                    "error": error.localizedDescription,
                    "messageId": messageId,
                    "recipientId": recipientId,
                    "consecutiveFailures": failures
                ])

                // If too many consecutive send failures, the connection is likely dead
                // Trigger disconnect so DORS can switch to another transport
                if failures >= self.MAX_CONSECUTIVE_FAILURES {
                    self.emitDiagnostic("warning", "Too many consecutive send failures, triggering reconnect for DORS", context: [
                        "failures": failures
                    ])
                    self.teardownSocket(ifCurrent: task, reason: "Send failures exceeded threshold")
                }
            } else {
                // Reset failure counter on successful send
                self.consecutiveSendFailures.set(0)
                self.bytesSent.add(Int64(jsonData.count))
                self.messagesSent.increment()
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
    /// primary frame's socket-write success, failed otherwise. Extra frames
    /// (member deltas, LeaveGroup taps) go through the token-gated pending
    /// queue; the translator's commit runs only after the last of them was
    /// written. Runs on messageQueue.
    private func sendControlOp(
        messageId: String,
        recipientId: String,
        controlOp: String,
        controlPayload: String,
        data: Data,
        replyToMsg: String?
    ) {
        let translation = controlOpTranslator.translate(
            controlOp: controlOp,
            controlPayload: controlPayload,
            recipientId: recipientId
        )
        switch translation {
        case .passThrough:
            // Every op the core emits should translate to .replace or .tap;
            // .passThrough here means an unknown op (translator behind the
            // Rust registry — see test_internet_control_op_registry_is_closed)
            // or a malformed payload. The frame still ships verbatim as
            // SendMessage (the relay echoes/forwards it without acting), but
            // make the degradation observable instead of silent.
            emitDiagnostic("warning", "Unhandled control op sent verbatim as SendMessage", context: [
                "controlOp": controlOp,
                "recipientId": recipientId
            ])
            sendMessage(messageId: messageId, recipientId: recipientId, data: data, replyToMsg: replyToMsg)

        case .tap(let frames, let commit):
            // Verbatim delivery owns the message id outcome; the extra
            // relay-native frames are best-effort. The translator's state
            // commits only once every extra frame was written — a dropped
            // frame must be re-sent by a later translation, not assumed
            // applied. Frames the rate limiter defers spill to the next poll
            // tick with the commit still attached.
            sendMessage(messageId: messageId, recipientId: recipientId, data: data, replyToMsg: replyToMsg)
            enqueueControlFrames(controlOp: controlOp, frames: frames, commit: commit)

        case .replace(let frames, let commit):
            guard isConnected, isAuthenticated, let task = webSocketTask else {
                rateLimiter.refund()
                protocolInstance.internetSendFailed(messageId: messageId)
                return
            }
            guard let primary = frames.first else {
                // Nothing to send (fully deduped) — the intent is already
                // reflected server-side; confirm so the core moves on.
                // No frame was written: return the poll loop's token.
                rateLimiter.refund()
                commit?()
                protocolInstance.internetConfirmSent(messageId: messageId)
                return
            }
            guard let primaryData = try? JSONSerialization.data(withJSONObject: primary),
                  let primaryJson = String(data: primaryData, encoding: .utf8) else {
                // A non-empty frame that cannot serialize is a failure, not
                // a dedup: never confirm a message nothing was written for.
                rateLimiter.refund()
                protocolInstance.internetSendFailed(messageId: messageId)
                emitDiagnostic("error", "Unserializable relay-native control op", context: [
                    "controlOp": controlOp,
                    "messageId": messageId
                ])
                return
            }
            // Plane tag: connection-request primaries are answered by the
            // relay's recipient-keyed ConnectionRequestError, so they are
            // tracked (CONN_REQ, recorded BEFORE the write — same fast-answer
            // race as sendMessage). Group-scoped primaries (CreateGroup /
            // SendGroupMessage) answer on the group-scoped GroupError channel
            // and are never recorded here.
            let isConnectionRequestOp: Bool
            switch controlOp {
            case "conn_req", "conn_acc", "conn_rej", "conn_can":
                isConnectionRequestOp = true
            default:
                isConnectionRequestOp = false
            }
            if isConnectionRequestOp {
                inFlightTracker.recordSent(
                    recipient: recipientId,
                    messageId: messageId,
                    plane: .connReq,
                    nowMs: monotonicNowMs()
                )
            }
            // Hold the poll loop until the primary's outcome lands: the
            // extra frames and the commit are handed off only from the
            // SUCCESS completion below.
            inFlightControlPrimaries += 1
            task.send(.string(primaryJson)) { [weak self] error in
                guard let self = self else { return }
                // Hop to messageQueue: the primary counter, the spill queue,
                // and enqueueControlFrames are messageQueue-confined.
                self.messageQueue.async {
                    self.inFlightControlPrimaries -= 1
                    // A stale task's send outcome must not touch failure
                    // counters, fail the message id (fail_all_pending owned
                    // it), or trigger teardown (see isStale); its deltas and
                    // commit die with the connection (the disconnect reset
                    // generation-kills the commit).
                    guard !self.isStale(task) else { return }
                    if let error = error {
                        // Never hit the wire — return the token, take back
                        // the optimistic in-flight entry. No deltas and no
                        // commit either (Kotlin only enqueues inside
                        // `if (sent)`): a failed CreateGroup followed by
                        // succeeding AddGroupMember deltas would send
                        // out-of-order registration state, and committing
                        // would record membership the relay never saw.
                        self.rateLimiter.refund()
                        if isConnectionRequestOp {
                            self.inFlightTracker.unrecord(recipient: recipientId, messageId: messageId)
                        }
                        let failures = self.consecutiveSendFailures.increment()
                        self.protocolInstance.internetSendFailed(messageId: messageId)
                        self.emitDiagnostic("error", "Failed to send relay-native control op", context: [
                            "controlOp": controlOp,
                            "messageId": messageId,
                            "error": error.localizedDescription,
                            "consecutiveFailures": failures
                        ])
                        if failures >= self.MAX_CONSECUTIVE_FAILURES {
                            self.teardownSocket(ifCurrent: task, reason: "Send failures exceeded threshold")
                        }
                    } else {
                        self.consecutiveSendFailures.set(0)
                        self.bytesSent.add(Int64(primaryData.count))
                        self.messagesSent.increment()
                        self.protocolInstance.internetConfirmSent(messageId: messageId)
                        // Only now, with the primary provably written, may
                        // the deltas chase it and the commit arm — keeping
                        // the wire order primary-then-deltas and the
                        // translator's diff base honest.
                        self.enqueueControlFrames(
                            controlOp: controlOp,
                            frames: Array(frames.dropFirst()),
                            commit: commit
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
    }

    /// Queues a translation's extra frames for token-gated delivery and
    /// starts draining immediately (the common small case goes out in the
    /// same tick). The commit runs only after the translation's last frame
    /// was written. messageQueue only.
    private func enqueueControlFrames(
        controlOp: String,
        frames: [[String: Any]],
        commit: (() -> Void)?
    ) {
        if frames.isEmpty {
            commit?()
            return
        }
        pendingControlFrames.append(
            PendingControlFrames(controlOp: controlOp, frames: frames, commit: commit)
        )
        drainPendingControlFrames()
    }

    /// Sends deferred control frames, oldest first, as tokens allow — one
    /// frame at a time: the next frame is submitted only from the previous
    /// frame's send completion (hopped back to messageQueue), so "written to
    /// the socket" means the completion succeeded and a translation's commit
    /// truly runs after its LAST frame's write outcome. A write failure
    /// drops everything pending: the frames are per-connection, their
    /// commits stay uninvoked (and are generation-dead after the disconnect
    /// reset), and the reconnect's re-register re-derives them.
    /// messageQueue only.
    private func drainPendingControlFrames() {
        guard !isDrainingControlFrames else { return }
        guard let pending = pendingControlFrames.first, let frame = pending.frames.first else { return }
        guard isConnected, isAuthenticated, let task = webSocketTask else { return }
        // Presence/data frames all meter through the same bucket; a deferred
        // frame just waits for the next tick's tokens.
        guard rateLimiter.tryAcquire(nowMs: monotonicNowMs()) else { return }
        guard let frameData = try? JSONSerialization.data(withJSONObject: frame),
              let frameJson = String(data: frameData, encoding: .utf8) else {
            rateLimiter.refund()
            // Unreachable today (frames are translator-built from parsed
            // JSON) — but if it fires, it is a serialization bug, not socket
            // health, and only the OWNING translation is unsendable: drop
            // it (commit stays uninvoked; the next register re-derives) and
            // keep draining the rest instead of wiping every pending chain.
            emitDiagnostic("error", "Unserializable deferred control frame — dropping its translation", context: [
                "controlOp": pending.controlOp,
                "frameType": frame["type"] as? String ?? "unknown"
            ])
            pendingControlFrames.removeFirst()
            drainPendingControlFrames()
            return
        }
        isDrainingControlFrames = true
        task.send(.string(frameJson)) { [weak self] error in
            guard let self = self else { return }
            self.messageQueue.async {
                self.isDrainingControlFrames = false
                // Stale task: the disconnect path already cleared the queue
                // and generation-killed the commits.
                guard !self.isStale(task) else { return }
                if let error = error {
                    self.rateLimiter.refund()
                    self.emitDiagnostic("warning", "Relay control frame dropped by socket", context: [
                        "controlOp": pending.controlOp,
                        "frameType": frame["type"] as? String ?? "unknown",
                        "error": error.localizedDescription
                    ])
                    self.pendingControlFrames.removeAll()
                    return
                }
                self.bytesSent.add(Int64(frameData.count))
                // The queue may have been cleared (RateLimited) — and even
                // repopulated by a newer translation — while the write was in
                // flight. Only the translation that owns the in-flight frame
                // may be popped; a positional pop would silently drop a frame
                // that was never sent.
                guard self.pendingControlFrames.first === pending else { return }
                pending.frames.removeFirst()
                if pending.frames.isEmpty {
                    self.pendingControlFrames.removeFirst()
                    pending.commit?()
                }
                self.drainPendingControlFrames()
            }
        }
    }

    /// Sends a raw, caller-built relay command verbatim (RN
    /// `internetSendRawCommand`). The JSON must parse; returns false when
    /// invalid, not connected+authenticated, or deferred by the client-side
    /// rate limiter (the caller may retry). Responses the SDK doesn't
    /// consume arrive as `internet_server_message` events.
    public func sendRawCommand(json: String, completion: @escaping (Bool) -> Void) {
        guard isConnected, isAuthenticated, let task = webSocketTask else {
            completion(false)
            return
        }
        guard let data = json.data(using: .utf8),
              let parsed = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
            emitDiagnostic("warning", "Rejected invalid raw server command", context: [:])
            completion(false)
            return
        }
        guard rateLimiter.tryAcquire(nowMs: monotonicNowMs()) else {
            completion(false)
            return
        }
        // An app-authored SendMessage joins the same per-recipient FIFO the
        // relay answers in order: without a tracker entry its MessageSent
        // would consume the oldest SDK entry via the oldest-first fallback,
        // costing that message its DeliveryError fail-fast. Sentinel-id
        // entries resolve/drain like any other but are never reported to
        // the core (the app owns raw-frame outcomes) — see
        // handleRecipientUnreachable. Mirrors InternetManager.kt.
        var sentinel: (recipient: String, id: String)? = nil
        if parsed["type"] as? String == "SendMessage",
           let recipient = parsed["recipient"] as? String, !recipient.isEmpty {
            let id = Self.rawSendSentinelPrefix + UUID().uuidString
            inFlightTracker.recordSent(
                recipient: recipient,
                messageId: id,
                plane: .data,
                nowMs: monotonicNowMs()
            )
            sentinel = (recipient, id)
        }
        // The ORIGINAL string is what goes out (re-serializing would alter
        // app-authored frames). Resolve from the send completion so `true`
        // means written to the socket, matching the documented TS contract
        // (OkHttp on Android is enqueue-confirmed — the closest its API
        // offers); a failed write returns its token.
        task.send(.string(json)) { [weak self] error in
            if error != nil {
                self?.rateLimiter.refund()
                // Never written: no relay outcome will ever correlate.
                if let sentinel = sentinel {
                    self?.inFlightTracker.unrecord(recipient: sentinel.recipient, messageId: sentinel.id)
                }
            }
            completion(error == nil)
        }
    }

    // MARK: - Presence Watch

    /// Fail-fast handler for the relay's recipient-keyed offline signals
    /// (DeliveryError / ConnectionRequestError). Fails every live in-flight
    /// message to the recipient ON THE SIGNAL'S PLANE (DeliveryError answers
    /// data frames, ConnectionRequestError answers conn ops — neither may
    /// sweep the other's entries) with the recipient_unreachable reason (the
    /// core classifies it as per-peer no-carrier and parks welcomes without
    /// burning budget), ingests an authoritative offline presence, and adds
    /// the recipient to the presence watch set.
    private func handleRecipientUnreachable(
        recipient: String,
        reason: String,
        plane: RecipientInFlightTracker.Plane,
        source: String
    ) {
        guard !recipient.isEmpty else {
            emitDiagnostic("warning", "Recipient-unreachable signal without recipient", context: [
                "source": source,
                "reason": reason
            ])
            return
        }
        let now = monotonicNowMs()
        let failedIds = inFlightTracker.drainRecipient(recipient, plane: plane, nowMs: now)
        for id in failedIds {
            // Sentinel entries track app-authored raw SendMessage frames
            // only to keep the per-recipient FIFO honest for MessageSent
            // resolution; their outcomes belong to the app, not the core.
            if id.hasPrefix(Self.rawSendSentinelPrefix) { continue }
            protocolInstance.internetSendFailedWithReason(
                messageId: id,
                reason: "recipient_unreachable: \(reason)"
            )
        }
        // Never watch self, and never feed "self is offline" into the core:
        // a malformed self-addressed frame's DeliveryError would otherwise
        // occupy a rotation slot until the idle TTL and could surface a
        // presence_updated(self, offline) to the app. (The core drops self
        // presence too — this just keeps the bridge honest at the source.)
        if recipient != deviceId {
            presenceWatch.watch(recipient, nowMs: now)
            protocolInstance.internetPeerPresence(peerId: recipient, online: false, lastSeenMs: nil)
        }
        emitDiagnostic("warning", "Recipient unreachable", context: [
            "recipient": recipient,
            "reason": reason,
            "source": source,
            "failedInFlight": failedIds.count
        ])
    }

    private func startPresenceWatch() {
        let timer = DispatchSource.makeTimerSource(queue: messageQueue)
        timer.schedule(
            deadline: .now() + PresenceWatchPolicy.defaultTickInterval,
            repeating: PresenceWatchPolicy.defaultTickInterval
        )
        timer.setEventHandler { [weak self] in
            self?.presenceWatchTick()
        }
        timerLock.lock()
        let previous = presenceWatchTimer
        presenceWatchTimer = timer
        timerLock.unlock()
        previous?.cancel()
        timer.resume()
    }

    private func stopPresenceWatch() {
        timerLock.lock()
        let timer = presenceWatchTimer
        presenceWatchTimer = nil
        timerLock.unlock()
        timer?.cancel()
    }

    private func presenceWatchTick() {
        guard isConnected, isAuthenticated, let task = webSocketTask else { return }

        let coreWatchlist = protocolInstance.internetPresenceWatchlist()
        // Monotonic like every tracker/watch call (mirrors the Kotlin
        // bridge's contract): a wall-clock step must never evict the whole
        // watch set through the idle TTL.
        let now = monotonicNowMs()
        // Self is filtered BEFORE the merge so it can never enter the watch
        // set and pin a rotation slot until the idle TTL.
        let peers = presenceWatch.peersToQuery(
            coreWatchlist: coreWatchlist.filter { $0 != deviceId },
            nowMs: now
        )
        var queried = 0
        for peer in peers {
            // Presence queries yield to data traffic under rate pressure;
            // skipped peers come around on a later rotation.
            guard rateLimiter.tryAcquire(nowMs: monotonicNowMs()) else { break }
            let checkMessage: [String: Any] = [
                "type": "CheckPresence",
                "username": peer
            ]
            guard let jsonData = try? JSONSerialization.data(withJSONObject: checkMessage),
                  let jsonString = String(data: jsonData, encoding: .utf8) else {
                rateLimiter.refund()
                continue
            }
            task.send(.string(jsonString)) { [weak self] error in
                // A failed completion means the frame never hit the wire —
                // return its token (Kotlin refunds on a false send()).
                if error != nil { self?.rateLimiter.refund() }
            }
            queried += 1
        }
        if queried > 0 {
            emitDiagnostic("debug", "Presence watch tick", context: [
                "queried": queried,
                "watched": presenceWatch.watchedPeers().count
            ])
        }
    }

    /// App-facing one-shot presence query (RN `checkInternetPresence`). The
    /// answer arrives as the SDK's `presence_updated` event — fire-and-event,
    /// matching relay semantics. Completes true only when the query was
    /// written to the socket (the send completion succeeded); false when not
    /// connected+authenticated or deferred by the client-side rate limiter
    /// (the caller may retry).
    public func checkPresence(userId: String, completion: @escaping (Bool) -> Void) {
        guard !userId.isEmpty, isConnected, isAuthenticated, let task = webSocketTask else {
            completion(false)
            return
        }
        let checkMessage: [String: Any] = [
            "type": "CheckPresence",
            "username": userId
        ]
        guard let jsonData = try? JSONSerialization.data(withJSONObject: checkMessage),
              let jsonString = String(data: jsonData, encoding: .utf8) else {
            completion(false)
            return
        }
        guard rateLimiter.tryAcquire(nowMs: monotonicNowMs()) else {
            completion(false)
            return
        }
        task.send(.string(jsonString)) { [weak self] error in
            if error != nil { self?.rateLimiter.refund() }
            completion(error == nil)
        }
    }

    // MARK: - Ping/Pong

    // Same lock-guarded swap as the presence watch timer (see timerLock).
    private func startPingTimer() {
        let timer = DispatchSource.makeTimerSource(queue: messageQueue)
        timer.schedule(deadline: .now() + PING_INTERVAL, repeating: PING_INTERVAL)
        timer.setEventHandler { [weak self] in
            self?.sendPing()
        }
        timerLock.lock()
        let previous = pingTimer
        pingTimer = timer
        timerLock.unlock()
        previous?.cancel()
        timer.resume()
    }

    private func stopPingTimer() {
        timerLock.lock()
        let timer = pingTimer
        pingTimer = nil
        timerLock.unlock()
        timer?.cancel()
    }
    
    private func sendPing() {
        guard let task = webSocketTask else { return }
        task.sendPing { [weak self] error in
            guard let self = self else { return }
            // A stale task's ping outcome says nothing about the current
            // connection (see isStale).
            guard !self.isStale(task) else { return }

            if let error = error {
                let failures = self.consecutivePingFailures.increment()
                self.emitDiagnostic("warning", "Ping failed", context: [
                    "error": error.localizedDescription,
                    "consecutiveFailures": failures
                ])

                // If ping fails, the connection is likely dead
                // Trigger disconnect so DORS can switch to another transport
                if failures >= self.MAX_CONSECUTIVE_FAILURES {
                    self.emitDiagnostic("warning", "Too many consecutive ping failures, triggering reconnect for DORS", context: [
                        "failures": failures
                    ])
                    self.teardownSocket(ifCurrent: task, reason: "Ping failures exceeded threshold")
                }
            } else {
                // Reset failure counter on successful ping
                self.consecutivePingFailures.set(0)
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
            guard let self = self, !self.isStale(webSocketTask) else { return }
            self.handleConnectionOpened(task: webSocketTask)
        }
    }

    public func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask, didCloseWith closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?) {
        // One of the 2-3 terminal signals URLSession fires per disconnect;
        // funnel into the single task-scoped close: main hop, identity
        // check, detach FIRST, then handleConnectionClosed — whichever
        // signal lands first wins and the rest become stale no-ops.
        DispatchQueue.main.async { [weak self] in
            guard let self = self, webSocketTask === self.webSocketTask else { return }
            self.webSocketTask = nil
            let reasonString = reason.flatMap { String(data: $0, encoding: .utf8) }
            self.emitDiagnostic("info", "WebSocket closed", context: [
                "closeCode": closeCode.rawValue,
                "reason": reasonString ?? "none"
            ])
            self.handleConnectionClosed(error: nil)
        }
    }

    public func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        if let error = error {
            // Same close funnel as didCloseWith / the receive-loop failure.
            handleSocketTerminated(task, error: error)
        }
    }
}

extension InternetManager: @unchecked Sendable {}

/// Lock-guarded counter mirroring the Kotlin bridge's atomic metrics and
/// failure counters: send/ping completions mutate on the URLSession delegate
/// queue, the poll loop on messageQueue, and getMetrics() reads from the
/// caller's thread.
final class AtomicCounter {
    private var value: Int64 = 0
    private let lock = NSLock()

    /// Adds one and returns the new value.
    @discardableResult
    func increment() -> Int64 {
        lock.lock()
        defer { lock.unlock() }
        value += 1
        return value
    }

    func add(_ delta: Int64) {
        lock.lock()
        defer { lock.unlock() }
        value += delta
    }

    func set(_ newValue: Int64) {
        lock.lock()
        defer { lock.unlock() }
        value = newValue
    }

    func get() -> Int64 {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

