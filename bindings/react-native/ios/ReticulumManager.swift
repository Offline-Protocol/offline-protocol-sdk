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
    // Guarded by [stateLock] — see [startMessagePolling] for why it has to
    // share the flag's lock rather than sit beside it.
    private var _messageTimer: DispatchSourceTimer?
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

    // MARK: - Gateway contract state

    /// Frames submitted to the gateway and not yet answered.
    private let verdicts = GatewayVerdictTracker()

    /// The SDK-owned presence watchlist, rotated the same way the relay
    /// manager rotates its own.
    private let presenceWatch = PresenceWatchPolicy()
    // Guarded by [stateLock], like `_messageTimer` and for the same reason:
    // it is armed from [messageQueue] and torn down from main, the React
    // Native method queue and [connectionQueue], and an unguarded swap lets a
    // start and a stop that interleave orphan a source that ticks for the
    // life of the manager.
    private var _presenceWatchTimer: DispatchSourceTimer?

    /// Fires if the handshake does not finish, so a gateway that accepts the
    /// socket and then says nothing costs one attach timeout rather than the
    /// connection timeout. Guarded by [stateLock] for the reason above.
    private var _attachTimeoutWorkItem: DispatchWorkItem?

    /// Which gateway session the current socket belongs to.
    ///
    /// Every frame handler that acts a queue hop later captures this when the
    /// frame arrives and checks it when the hop runs, because the hop can run
    /// after the socket that carried the frame is gone and its successor is
    /// up: [messageQueue] waits on the global protocol mutex, and a flush or
    /// an MLS operation can hold that for longer than the reconnect delay. A
    /// stale `StatusUpdate(connected)` acted on then would announce the
    /// successor before the gateway bound it, or tear it down for not being
    /// bound yet; a stale `Challenge` would sign the old challenge onto the
    /// new socket and draw a refusal. Bumped on every teardown, under
    /// [stateLock]. The Android manager threads `connectGeneration` the same
    /// way.
    private var _sessionGeneration = 0
    private var sessionGeneration: Int {
        stateLock.lock(); defer { stateLock.unlock() }; return _sessionGeneration
    }

    /// True once the gateway has echoed our own address back.
    ///
    /// This — not the TCP connection — is what makes the carrier usable. A
    /// session the gateway did not bind is verdict-only on the other side: it
    /// may submit and be told `attach_required`, and it is never registered as
    /// a recipient, so nothing addressed to this device would ever arrive.
    /// Offering that to the selector would be offering a transport that can
    /// only refuse.
    private var _isBound = false
    private var isBound: Bool {
        stateLock.lock(); defer { stateLock.unlock() }; return _isBound
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
        //
        // The ordering is what makes this a pause rather than a cancel: any
        // arming that has not yet taken [stateLock] sees the flag and refuses
        // ([startMessagePolling]), and any that already holds it installs a
        // timer the `stop` below then cancels. Neither order leaves a live
        // timer behind.
        isPaused = true
        stopMessagePolling()
        // A backgrounded app must not keep spending battery on presence ticks
        // against a gateway; the watchlist is rebuilt from the core after
        // resume(). Mirrors `InternetManager.pause`.
        stopPresenceWatch()
    }

    public func resume() {
        isPaused = false
        if state == .running && isConnected {
            // Also drains whatever queued during the pause: the poll timer is
            // scheduled at `.now()`, and the core does not re-issue
            // `onMessagesAvailable` for messages it already announced.
            startMessagePolling()
            // Only a bound session has anyone to ask. An unbound one is not
            // announced as a carrier either, so there is nothing waiting on
            // its answers.
            if isBound {
                startPresenceWatch()
            }
        }
    }

    // MARK: - Connection Management

    private func connect() {
        // The attempt takes a fresh session generation as it claims the
        // flags, so a socket never shares a number with the one it replaces
        // even if nothing retired the predecessor in between. Every callback
        // this attempt arms carries it.
        stateLock.lock()
        let skip = _isConnecting || _isConnected
        if !skip {
            _isConnecting = true
            _sessionGeneration += 1
        }
        let generation = _sessionGeneration
        stateLock.unlock()
        guard !skip else { return }

        emitDiagnostic("info", "Connecting to Reticulum daemon", context: [
            "host": daemonHost,
            "port": daemonPort
        ])

        let host = NWEndpoint.Host(daemonHost)
        let port = NWEndpoint.Port(rawValue: daemonPort) ?? NWEndpoint.Port(rawValue: 4242)!

        let conn = NWConnection(host: host, port: port, using: .tcp)

        conn.stateUpdateHandler = { [weak self, weak conn] newState in
            guard let self = self else { return }
            switch newState {
            case .ready:
                // The claim refuses a late open for a session the connection
                // timeout already retired, and a stop() that landed while
                // the open was in flight; either way the socket is stray.
                guard let conn = conn, self.handleConnectionOpened(generation: generation) else {
                    conn?.cancel()
                    return
                }
                self.startReceiving(on: conn, generation: generation)
            case .failed(let error):
                self.emitDiagnostic("error", "Reticulum connection failed", context: [
                    "error": error.localizedDescription
                ])
                self.handleConnectionClosed(error: error, generation: generation)
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
            self.handleConnectionClosed(error: nil, generation: generation)
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
        retireSession(reason: "Disconnected")
        // `receiveBuffer` is not reset here: it belongs to connectionQueue,
        // and this runs from main and the React Native method queue too. The
        // next open resets it there, and a completion for this socket that
        // is still in flight is dropped by its generation before it appends.
    }

    /// Ends the gateway session the current socket carried, whether this side
    /// is closing the socket or the socket has already gone.
    ///
    /// Shared by [disconnect] and [handleConnectionClosed] so that every close
    /// path runs it. Before it was shared, a daemon-side drop ran none of it:
    /// the session stayed bound for the successor to inherit, which defeated
    /// the attach timeout (it checks `isBound`), the presence watch kept
    /// writing to a dead socket every 20s, and the ids in flight were never
    /// failed, so the core waited out its own 120s expiry on each.
    ///
    /// Every frame the connection was carrying is owed an outcome. A frame
    /// nobody reports on waits out that expiry instead of going back on the
    /// retry ladder now, so silence here is two minutes of nothing per
    /// message.
    private func retireSession(reason: String) {
        stateLock.lock()
        _sessionGeneration += 1
        _isBound = false
        stateLock.unlock()
        cancelAttachTimeout()
        stopPresenceWatch()
        failInFlight(reason: reason)
    }

    /// Claims the flags for the socket `generation` names, or refuses when
    /// that session is already over.
    ///
    /// The generation check and the claim are one step under [stateLock],
    /// as Android's `handleConnectionOpened` folds them. Checked and then
    /// claimed as two, a `stop()` landing between them would leave
    /// `isConnected` true on a retired generation with no connection behind
    /// it, and the next `start()`'s `connect()` would skip on that flag with
    /// nothing left to reconnect: a transport wedged at `.running` until
    /// cycled by hand.
    private func handleConnectionOpened(generation: Int) -> Bool {
        stateLock.lock()
        guard _sessionGeneration == generation else {
            stateLock.unlock()
            return false
        }
        _isConnected = true
        _isConnecting = false
        stateLock.unlock()
        connectionTimeoutWorkItem?.cancel()
        connectionTimeoutWorkItem = nil
        consecutiveSendFailures = 0
        receiveBuffer = Data()

        // Not a backoff reset. A TCP open proves only that something is
        // listening; the handshake that follows has four places left to
        // fail, and every one of them reconnects. Reset here, a refusing
        // gateway was retried at the 1s floor forever, with a challenge, a
        // signature and two events spent per turn, and `maxReconnectAttempts`
        // never tripped because the count went back to zero each time. The
        // reset lives in [completeAttach], on the bound session, which is
        // what the relay manager does on `Authenticated`.

        emitDiagnostic("info", "Connected to Reticulum daemon")

        // Identify on messageQueue, not here. `localAddress()` is a UniFFI
        // call that takes the global protocol mutex, and this runs on
        // connectionQueue — the queue every inbound byte arrives on, so
        // blocking it stalls the reads that carry the answer we are about to
        // wait for.
        //
        // `device_id` is this device's address where there is one. The
        // shipped clients sent `config.profile`, a local storage-namespace
        // selector that is not an identity in any namespace the gateway
        // knows; it is logged and never routed on, so it was harmless and
        // useless. Only `DeclareAddress` binds either way.
        messageQueue.async { [weak self] in
            guard let self = self, self.sessionGeneration == generation else { return }
            let identity = self.protocolInstance.localAddress() ?? self.deviceId
            // Checked again after the wait on the global mutex, as the
            // Challenge hop is after its signature: a retired session's
            // Identify on the successor's socket is a second challenge.
            guard self.sessionGeneration == generation else { return }
            if let frame = GatewayAttachPolicy.identifyJson(deviceId: identity) {
                self.sendRaw(frame + "\n")
            }
        }

        armAttachTimeout(generation: generation)

        // Start polling on main thread; notify protocol after state is .running
        // so that any protocol handler querying transport state sees the correct value.
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            // A stop() that retired this session already ran disconnect()
            // for it; acting here would cancel whatever connect() a later
            // start() has since opened. Android gates the same post on its
            // generation first.
            guard self.sessionGeneration == generation else { return }

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
            //
            // Belt to the braces [startMessagePolling] now carries internally.
            // It refuses to arm while paused whatever this reads, which is
            // what makes the refusal proof against a `pause()` landing between
            // this check and the call — this block runs on main while
            // `pause()` runs on the React Native method queue, so nothing
            // orders them.
            if !self.isPaused {
                self.startMessagePolling()
            }
            // No status flip here. The carrier is announced in
            // [completeAttach], on `StatusUpdate(connected)` with a bound
            // session, and that flip is where the check-and-call under
            // [statusFlipLock] lives; see it for why `isConnected` is half
            // of that decision.
        }
        return true
    }

    /// Announces the carrier once the gateway has bound this session.
    ///
    /// Called on `StatusUpdate(connected)`, which the contract puts after
    /// `AddressDeclared` and after `Capabilities` — so by the time the core is
    /// told this transport is available, it has already been told what the
    /// gateway can do, and the flush that the false→true edge triggers sees
    /// them. That ordering is the contract's, and it is pinned by a Rust
    /// source guard because neither platform can test it alone.
    ///
    /// A session the gateway refused to bind never reaches here: the refusal
    /// closes the connection instead. That is the one place this manager
    /// diverges from the relay manager, which reports up and keeps working in
    /// account-name space. A gateway has no such space.
    ///
    /// Decided on [messageQueue], never on the queue the frame arrived on.
    /// `isBound` is set by the `AddressDeclared` hop on that same serial
    /// queue, behind two FFI calls that wait on the global protocol mutex,
    /// and a conforming gateway writes `AddressDeclared`, `Capabilities` and
    /// `StatusUpdate(connected)` in one go: they arrive in one read and are
    /// dispatched before that hop has taken the mutex. Read on the socket
    /// queue, `isBound` was still false here, the timeout was already
    /// cancelled, and the carrier was never announced.
    private func completeAttach(generation: Int) {
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            guard self.sessionGeneration == generation else { return }
            guard self.isBound else {
                // A gateway that announces before it binds is not speaking
                // the contract, and a session it never binds is verdict-only.
                // Closing is the one action that can end somewhere usable.
                // Returning with the timeout cancelled left the transport
                // connected and mute until the daemon happened to drop it.
                self.emitDiagnostic("error", "Gateway reported connected before binding the session")
                self.handleConnectionClosed(error: nil, generation: generation)
                return
            }
            self.statusFlipLock.lock()
            let announced = self.isConnected && self.state != .stopping && self.state != .stopped
            if announced {
                try? self.protocolInstance.reticulumStatusChanged(isConnected: true)
            }
            self.statusFlipLock.unlock()
            guard announced else { return }
            // The gateway bound and announced this session: this, not the
            // TCP open, is what proves the connection good and earns a
            // backoff reset. See [handleConnectionOpened] for what resetting
            // there cost.
            self.reconnectAttempts = 0
            self.currentReconnectDelay = self.RECONNECT_INITIAL_DELAY
            // Skipped while paused, like the poll: `resume()` restarts both.
            guard !self.isPaused else { return }
            self.startPresenceWatch()
            self.pollAndSendMessages()
        }
    }

    /// Bounds the handshake. A gateway that accepts the socket and then says
    /// nothing is not slow, and waiting out the connection timeout for it
    /// means a minute in which the selector has been told nothing at all.
    ///
    /// Disarmed by `StatusUpdate(connected)` and by a teardown, and by
    /// nothing else: it is not gated on the bind, because a gateway that
    /// binds and then never announces is exactly the wedge this exists to
    /// end, and a session that was bound but never announced is one the
    /// core was never told about.
    private func armAttachTimeout(generation: Int) {
        let item = DispatchWorkItem { [weak self] in
            guard let self = self, self.isConnected, self.sessionGeneration == generation else {
                return
            }
            self.emitDiagnostic("error", "Gateway attach timed out before StatusUpdate(connected)")
            self.handleConnectionClosed(error: nil, generation: generation)
        }
        stateLock.lock()
        let previous = _attachTimeoutWorkItem
        _attachTimeoutWorkItem = item
        stateLock.unlock()
        previous?.cancel()
        connectionQueue.asyncAfter(
            deadline: .now() + GatewayAttachPolicy.ATTACH_TIMEOUT, execute: item)
    }

    private func cancelAttachTimeout() {
        stateLock.lock()
        let item = _attachTimeoutWorkItem
        _attachTimeoutWorkItem = nil
        stateLock.unlock()
        item?.cancel()
    }

    /// Reads frames off `conn` for the session `generation` names.
    ///
    /// The generation is the one this socket was armed under, not one read
    /// when a line is dispatched: an inline close in the middle of a segment
    /// (a refusal, a malformed challenge) bumps it, and the lines after the
    /// close in the same read would otherwise capture the successor's number
    /// and act on its session, injecting a refused session's capabilities
    /// after the clear or closing the successor for a `connected` it never
    /// saw. Every completion checks it first, so a socket that has been
    /// retired stops being read the moment its next callback fires.
    private func startReceiving(on conn: NWConnection, generation: Int) {
        conn.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] content, _, isComplete, error in
            guard let self = self, self.sessionGeneration == generation else { return }

            if let data = content {
                self.receiveBuffer.append(data)

                // Process complete lines (newline-delimited JSON)
                let newlineByte = Data([0x0A])
                while let newlineRange = self.receiveBuffer.range(of: newlineByte) {
                    let lineData = self.receiveBuffer.subdata(in: self.receiveBuffer.startIndex..<newlineRange.lowerBound)
                    self.receiveBuffer.removeSubrange(self.receiveBuffer.startIndex..<newlineRange.upperBound)
                    if !lineData.isEmpty {
                        self.processReceivedData(lineData, generation: generation)
                    }
                    // A line that closed the session ends the segment: the
                    // rest belongs to a socket this side has retired.
                    if self.sessionGeneration != generation { return }
                }

                // What remains is one partial line. Past the cap it cannot be
                // resynchronised: its tail would be read as a fresh line, so
                // every frame after it is garbage. The connection goes
                // instead, and the reconnect starts clean. Checked after the
                // split so the cap is on the line, as on Android, and not on
                // however many complete lines shared the read with it.
                if self.receiveBuffer.count > GatewayAttachPolicy.MAX_LINE_BYTES {
                    self.emitDiagnostic("error", "Over-long line from the gateway", context: [
                        "bytes": self.receiveBuffer.count
                    ])
                    self.handleConnectionClosed(error: nil, generation: generation)
                    return
                }
            }

            if isComplete {
                self.handleConnectionClosed(error: nil, generation: generation)
                return
            }

            if let error = error {
                self.handleConnectionClosed(error: error, generation: generation)
                return
            }

            // Continue receiving
            self.startReceiving(on: conn, generation: generation)
        }
    }

    /// Ends the session `generation` names, if it is still the current one.
    ///
    /// Every caller reaches this with the generation of the socket it is
    /// reporting on, and a report for a session that is already over is
    /// dropped before it touches the flags: a stale close would otherwise
    /// clear `isConnected` under a healthy successor, tell the core the
    /// transport is down, and start a reconnect ladder against it. The
    /// Android manager checks `connectGeneration` at the same point.
    private func handleConnectionClosed(error: NWError?, generation: Int) {
        stateLock.lock()
        guard _sessionGeneration == generation else {
            stateLock.unlock()
            return
        }
        let wasConnected = _isConnected
        let wasConnecting = _isConnecting
        _isConnected = false
        _isConnecting = false
        stateLock.unlock()

        // Prevent duplicate disconnect handling
        guard wasConnected || wasConnecting else { return }

        // The socket goes now, not when the reconnect fires. A refused or
        // mismatched session is still open on the gateway's side and it keeps
        // sending on it, including, after its grace, the
        // `StatusUpdate(connected)` of a session it never bound. The session
        // state goes with it, on every close path and not only on `stop()`;
        // see [retireSession] for what a drop that kept it cost.
        connection?.cancel()
        retireSession(reason: "Connection lost")

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
                // No reconnect is coming to run [disconnect] for this
                // socket, so release it here, as `stop()` does.
                self.disconnect()
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

    private func processReceivedData(_ data: Data, generation: Int) {
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

        case "Challenge":
            handleChallenge(json, generation: generation)

        case "AddressDeclared":
            handleAddressDeclared(json, generation: generation)

        case "AddressError":
            handleAddressError(json, generation: generation)

        case "Capabilities":
            let tokens = GatewayAttachPolicy.capabilityTokens(from: json)
            messageQueue.async { [weak self] in
                guard let self = self, self.sessionGeneration == generation else { return }
                // Before the status flip, never after: the flush that flip
                // triggers has to see them.
                try? self.protocolInstance.reticulumGatewayCapabilities(capabilities: tokens)
            }

        case "MessageSent", "DeliveryError":
            handleVerdict(json, type: messageType)

        case "PresenceStatus":
            handlePresence(json)

        case "StatusUpdate":
            let daemonStatus = json["status"] as? String ?? "unknown"
            emitDiagnostic("debug", "Reticulum daemon status update", context: [
                "status": daemonStatus
            ])
            if daemonStatus == "connected" {
                // The gateway's half of the handshake is done, whatever the
                // hop below decides about ours. Disarmed here rather than in
                // the hop, so a hop stalled on the global mutex past the
                // deadline is not closed out from under by its own timeout.
                cancelAttachTimeout()
                completeAttach(generation: generation)
            }

        default:
            emitDiagnostic("debug", "Unknown Reticulum message type", context: [
                "type": messageType
            ])
        }
    }

    // MARK: - The gateway handshake

    /// Signs the gateway's challenge and declares this device's address.
    ///
    /// The signing happens in the core: this hands it the challenge and gets
    /// back the three fields `DeclareAddress` carries. A failure here is not
    /// retried on this connection — the gateway spends a challenge per
    /// connection, and the next reconnect gets a fresh one.
    private func handleChallenge(_ json: [String: Any], generation: Int) {
        switch GatewayAttachPolicy.decodeChallenge(json) {
        case .skip(let reason):
            emitDiagnostic("warning", "Cannot declare an address to the gateway", context: [
                "reason": reason
            ])
            handleConnectionClosed(error: nil, generation: generation)
        case .declare(let challenge):
            messageQueue.async { [weak self] in
                guard let self = self, self.sessionGeneration == generation else { return }
                do {
                    let declaration = try self.protocolInstance.gatewayAddressDeclaration(
                        challenge: [UInt8](challenge))
                    guard let frame = GatewayAttachPolicy.declarationJson(
                        address: declaration.address,
                        publicKey: Data(declaration.publicKey),
                        signature: Data(declaration.signature)
                    ) else {
                        self.emitDiagnostic("error", "Cannot serialize the address declaration")
                        self.handleConnectionClosed(error: nil, generation: generation)
                        return
                    }
                    // Checked again after the signature: it waited on the
                    // global mutex, and a proof over a retired challenge
                    // written to the successor's socket is a refusal.
                    guard self.sessionGeneration == generation else { return }
                    self.sendRaw(frame + "\n") { [weak self] error in
                        // A write that fails leaves no frame for the gateway
                        // to answer, and waiting out the attach timeout to
                        // learn that is ten seconds of a carrier the selector
                        // has been told nothing about. Android closes on the
                        // same failure.
                        guard let self = self, let error = error else { return }
                        self.emitDiagnostic("error", "Failed to write the address declaration", context: [
                            "error": error.localizedDescription
                        ])
                        self.handleConnectionClosed(error: error, generation: generation)
                    }
                } catch {
                    // No identity yet, or a challenge the core refused to
                    // sign. Either way this connection can only ever be
                    // verdict-only, so it is not worth holding open.
                    self.emitDiagnostic("warning", "Cannot build the address declaration", context: [
                        "reason": GatewayAttachPolicy.SkipReason.signingFailed,
                        "error": error.localizedDescription
                    ])
                    self.handleConnectionClosed(error: nil, generation: generation)
                }
            }
        }
    }

    /// Checks what the gateway says it bound against what we hold.
    ///
    /// Both answers go to the core, which owns the security warning; what is
    /// decided here is narrower and is the bridge's own: whether this carrier
    /// can be offered to the selector at all.
    private func handleAddressDeclared(_ json: [String: Any], generation: Int) {
        // Bounded before it reaches the core: the echo is remote-chosen and
        // the line it arrived on may be a mebibyte, and the core logs and
        // attributes the security event to whatever it is handed.
        guard let declared = json["address"] as? String, !declared.isEmpty,
              declared.utf8.count <= GatewayAttachPolicy.MAX_ADDRESS_BYTES else {
            emitDiagnostic("warning", "Invalid AddressDeclared: missing or over-long address")
            return
        }
        messageQueue.async { [weak self] in
            guard let self = self, self.sessionGeneration == generation else { return }
            self.protocolInstance.reticulumAddressDeclared(address: declared)

            let local = self.protocolInstance.localAddress()
            switch GatewayAttachPolicy.bindingOutcome(declared: declared, local: local) {
            case .bound:
                guard self.bindSession(ifCurrent: generation) else { return }
                self.emitDiagnostic("info", "Gateway bound this session to our address")
            case .mismatch, .unknownLocal:
                // The core has already reported this as a security warning.
                // Here it costs the connection: a gateway that bound an
                // address we do not control will attribute our frames to an
                // identity we cannot prove and answer presence about someone
                // else, and reconnecting is the only thing this side can do
                // that might land somewhere honest.
                self.emitDiagnostic("error", "Gateway bound a session to an address we do not hold")
                self.handleConnectionClosed(error: nil, generation: generation)
            }
        }
    }

    /// The gateway refused the declaration, so this session can only be told
    /// verdicts. The carrier is never announced; the connection goes and the
    /// existing backoff decides when to try again.
    private func handleAddressError(_ json: [String: Any], generation: Int) {
        let reason = json["reason"] as? String ?? "unspecified"
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            self.protocolInstance.reticulumAddressDeclarationRefused(reason: reason)
        }
        // Remote-chosen text, bounded before it reaches a diagnostic the
        // way the core bounds it before its own log.
        emitDiagnostic("warning", "Gateway refused the address declaration", context: [
            "reason": String(reason.prefix(256))
        ])
        handleConnectionClosed(error: nil, generation: generation)
    }

    // MARK: - Verdicts

    /// Settles one submitted frame on the gateway's answer.
    ///
    /// This is what the shipped bridges did not do. They called
    /// `reticulumConfirmSent` the moment the socket write returned, so every
    /// frame was "sent" whether the gateway forwarded it, refused it, or
    /// dropped it — and `recipient_unreachable`, the one verdict that parks a
    /// message and offers it to the mesh, never reached the core at all.
    private func handleVerdict(_ json: [String: Any], type: String) {
        guard let verdict = GatewayAttachPolicy.parseVerdict(json, type: type) else {
            emitDiagnostic("warning", "Verdict with no message_id, ignored", context: [
                "type": type
            ])
            return
        }
        guard verdicts.settle(verdict.messageId) else {
            // Already settled: a duplicate, or an answer to a frame this
            // connection timed out on. Reporting it again would settle an id
            // the core has moved past.
            return
        }
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            if verdict.sent {
                self.protocolInstance.reticulumConfirmSent(messageId: verdict.messageId)
            } else {
                // Verbatim. The core classifies on the `recipient_unreachable`
                // prefix and discards the rest at that boundary, so nothing
                // here needs to understand the gateway's wording.
                self.protocolInstance.reticulumSendFailedWithReason(
                    messageId: verdict.messageId, reason: verdict.reason)
                if let recipient = verdict.recipient, !recipient.isEmpty,
                   verdict.reason?.hasPrefix("recipient_unreachable") == true,
                   !self.isSelfPeer(recipient) {
                    // Watch them: the gateway pushes a PresenceStatus when a
                    // watched peer attaches, and that answer is what un-parks
                    // the message this verdict just parked.
                    self.presenceWatch.watch(recipient, nowMs: MonotonicClock.nowMs())
                    // And feed the verdict in as presence, as the relay
                    // manager does on its own DeliveryError. The core parks
                    // on the verdict already; this is what emits the
                    // `presence_updated(offline)` an app renders a header
                    // from, labelled with the carrier that answered. Never
                    // for self: the core drops that too, but a malformed
                    // self-addressed frame should not cost a watch slot.
                    self.protocolInstance.reticulumPeerPresence(
                        peerId: recipient, online: false, lastSeenMs: nil)
                }
            }
            // A verdict frees an in-flight slot, so the next frame goes out
            // on it. Unless paused: with eight in flight, a paused transport
            // would otherwise drain a batch per answer for the whole
            // background stay, which is what `isPaused` exists to stop.
            guard !self.isPaused else { return }
            self.pollAndSendMessages()
        }
    }

    /// Fails every outstanding frame with `reason`, on a connection going away
    /// or on a gateway that answered nothing.
    ///
    /// The hop captures the core handle and never `self`. `deinit` reaches
    /// here through `stop()`, and forming a weak reference to an object whose
    /// deallocation has begun is a hard abort (BRIDGE_MAINTENANCE.md, the
    /// SIGABRT class of #266), while a strong capture would resurrect it. The
    /// handle alone is also what `destroy()` needs: it stops the manager and
    /// releases it in one breath, and a block gated on `self` would find nil
    /// and never report the stranded ids.
    private func failInFlight(reason: String) {
        let stranded = verdicts.drainAll()
        guard !stranded.isEmpty else { return }
        let core = protocolInstance
        messageQueue.async {
            for messageId in stranded {
                core.reticulumSendFailedWithReason(messageId: messageId, reason: reason)
            }
        }
    }

    /// Binds the session, unless the socket that carried the echo is gone.
    ///
    /// One step under [stateLock] with the generation check, because
    /// [retireSession] clears the flag under the same lock: checked and set
    /// separately, a teardown landing between the two leaves the successor
    /// bound on an echo it never received.
    private func bindSession(ifCurrent generation: Int) -> Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard _sessionGeneration == generation else { return false }
        _isBound = true
        return true
    }

    private func isSelfPeer(_ peerId: String) -> Bool {
        if peerId.isEmpty { return false }
        if peerId == deviceId { return true }
        return peerId == protocolInstance.localAddress()
    }

    // MARK: - Presence

    private func handlePresence(_ json: [String: Any]) {
        guard let answer = GatewayAttachPolicy.parsePresence(json) else {
            emitDiagnostic("warning", "Invalid PresenceStatus, ignored")
            return
        }
        if answer.online {
            presenceWatch.unwatch(answer.peer)
        }
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            self.protocolInstance.reticulumPeerPresence(
                peerId: answer.peer, online: answer.online, lastSeenMs: answer.lastSeenMs)
        }
    }

    /// Arms the presence tick, unless the transport is paused.
    ///
    /// Same shape as [startMessagePolling], for the same two reasons: the
    /// pause gate and the timer swap are one [stateLock] critical section, so
    /// a `pause()` on another queue cannot slip between them, and the source
    /// is resumed on this straight-line path because releasing a suspended
    /// one is a hard crash.
    private func startPresenceWatch() {
        stateLock.lock()
        // Bound as well as unpaused: a `stop()` that lands between the
        // status flip and this call has already cleared the bind under this
        // lock, and a timer armed past it would tick on a stopped transport
        // until something else happened to cancel it.
        guard !_isPaused, _isBound else {
            stateLock.unlock()
            return
        }
        let previous = _presenceWatchTimer
        let timer = DispatchSource.makeTimerSource(queue: messageQueue)
        _presenceWatchTimer = timer
        stateLock.unlock()

        previous?.cancel()
        timer.schedule(
            deadline: .now() + PresenceWatchPolicy.defaultTickInterval,
            repeating: PresenceWatchPolicy.defaultTickInterval)
        timer.setEventHandler { [weak self] in
            self?.presenceWatchTick()
        }
        timer.resume()
    }

    private func stopPresenceWatch() {
        stateLock.lock()
        let timer = _presenceWatchTimer
        _presenceWatchTimer = nil
        stateLock.unlock()
        timer?.cancel()
        presenceWatch.clear()
    }

    /// Asks the gateway about the peers the core is waiting to hear about.
    ///
    /// One frame for the whole batch, which is the contract's shape. The core
    /// owns the list — every peer with an undelivered welcome, and every
    /// recipient of a parked message — so the app is not asked to maintain
    /// one.
    private func presenceWatchTick() {
        guard isBound, !isPaused else { return }
        let coreWatchlist = protocolInstance.reticulumPresenceWatchlist()
        let selfAddress = protocolInstance.localAddress()
        let candidates = coreWatchlist.filter { peer in
            !peer.isEmpty && peer != deviceId && peer != selfAddress
        }
        let peers = presenceWatch.peersToQuery(coreWatchlist: candidates, nowMs: MonotonicClock.nowMs())
        guard let frame = GatewayAttachPolicy.checkPresenceJson(peers: peers) else { return }
        sendRaw(frame + "\n")
    }

    /// The verdict tracker's clock, in seconds. Monotonic and sleep-inclusive
    /// like every tracker and watch call in the relay manager: a wall-clock
    /// step of a minute (an NTP correction after airplane mode) must not fail
    /// every frame in flight as unanswered, and one of ten minutes must not
    /// evict the whole watch set.
    private static func nowSeconds() -> TimeInterval {
        TimeInterval(MonotonicClock.nowMs()) / 1000.0
    }

    /// Arms the fallback poll timer, unless the transport is paused.
    ///
    /// The pause gate lives *here*, not at the call sites that arm this, and
    /// it shares one [stateLock] critical section with installing the timer.
    /// Both halves of that are load-bearing.
    ///
    /// **Why here.** A gate at the call site is an invariant every future
    /// caller has to remember; a gate at the one function that can violate it
    /// is an invariant a new caller cannot get wrong.
    ///
    /// **Why under the lock.** `pause()` runs on the React Native method
    /// queue while [handleConnectionOpened] arms this from *main*, so the two
    /// are fully concurrent — wider than the Nostr manager's window, which at
    /// least shares `messageQueue` with its own poll. A caller that read
    /// `isPaused` and then called in could be overtaken by the whole of
    /// `pause()` in between — flag set, timer cancelled — and arm a fresh 5s
    /// timer against a transport the app just paused, which then polls for the
    /// rest of the background stay. That is the durable symptom `isPaused`
    /// exists to remove, so the check and the install are one decision. The
    /// Android manager gets this for free: its `pause()` and its reconnect
    /// edge are the same thread.
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
            // batch leaks past every `pause()`. The Android polling runnable
            // carries the identical check.
            guard let self = self, !self.isPaused else { return }
            self.pollAndSendMessages()
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

    private func pollAndSendMessages() {
        guard isBound else { return }
        sweepExpiredVerdicts()
        sendNextMessage(sent: 0, maxBatchSize: 10)
    }

    /// Fails frames the gateway never answered.
    ///
    /// The contract says a gateway MUST answer every submission, and silence
    /// is the one failure the core cannot see: it holds the frame in
    /// `pending_confirmation` until its own 120s expiry and counts it a
    /// failure then. Failing it here, at 60s, puts it back on the retry
    /// ladder while the core still considers it live — which is why this
    /// timeout has to stay the shorter of the two.
    private func sweepExpiredVerdicts() {
        let stale = verdicts.expired(
            now: Self.nowSeconds(), timeout: GatewayAttachPolicy.VERDICT_TIMEOUT)
        guard !stale.isEmpty else { return }
        emitDiagnostic("warning", "Gateway did not answer for submitted frames", context: [
            "count": stale.count
        ])
        for messageId in stale {
            protocolInstance.reticulumSendFailedWithReason(
                messageId: messageId, reason: "gateway_silent: no verdict within 60s")
        }
    }

    /// Sends messages one at a time, chaining the next send from each completion
    /// handler so that NWConnection writes are serialized (no concurrent sends).
    private func sendNextMessage(sent: Int, maxBatchSize: Int) {
        // Bounded by what is unanswered, not only by the batch: a gateway
        // that is slow to answer must not have the whole outbox handed to it,
        // because every id in flight is one the core cannot retry until it is
        // settled.
        guard verdicts.count < GatewayAttachPolicy.MAX_IN_FLIGHT else { return }
        guard sent < maxBatchSize, isBound else {
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

        // The core re-queues an unconfirmed frame under the same id after its
        // own acknowledgement timeout, and a verdict can honestly take longer
        // than that over a radio backbone. Sending it again would forward the
        // frame twice and, when this copy timed out, fail an id the gateway
        // had already confirmed. Popping it was enough: the core's pending
        // entry is refreshed by the pop.
        guard verdicts.begin(message.messageId, now: Self.nowSeconds()) else {
            messageQueue.async { [weak self] in
                self?.sendNextMessage(sent: sent, maxBatchSize: maxBatchSize)
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

    /// Writes one frame. **The write is not the outcome.**
    ///
    /// A successful write means the gateway has the bytes, which says nothing
    /// about whether it could forward them; the answer arrives later as a
    /// `MessageSent` or a `DeliveryError` and is settled in [handleVerdict].
    /// Confirming here is what the shipped bridge did, and it is why a
    /// `recipient_unreachable` verdict — the one that parks a message and
    /// offers it to the mesh — could never reach the core.
    ///
    /// A *failed* write is settled here, because there is no frame on the wire
    /// for the gateway to answer about.
    private func sendMessage(messageId: String, recipientId: String, data: Data, replyToMsg: String? = nil, completion: (() -> Void)? = nil) {
        guard isBound, connection != nil else {
            emitDiagnostic("warning", "Cannot send message - not attached", context: [
                "messageId": messageId,
                "recipientId": recipientId
            ])
            settleLocally(messageId: messageId, reason: "Not attached to a gateway")
            completion?()
            return
        }

        let content = data.base64EncodedString()

        // Sanitised the way the gateway sanitises it: an id it would refuse
        // is replaced *there* by one it mints, and the verdict then comes back
        // under a name nothing here is waiting on. Message ids are UUIDs, so
        // this passes; it is what keeps that from being an assumption.
        guard let wireId = GatewayAttachPolicy.sanitizeMessageId(messageId),
              let jsonString = GatewayAttachPolicy.sendMessageJson(
                messageId: wireId,
                recipient: recipientId,
                content: content,
                replyToMsg: replyToMsg
              ) else {
            emitDiagnostic("error", "Failed to create Reticulum message")
            settleLocally(messageId: messageId, reason: "Unserializable frame")
            completion?()
            return
        }

        // Sampled before the write, so the failure it may report later
        // names the session this write belonged to.
        let generation = sessionGeneration
        sendRaw(jsonString + "\n") { [weak self] error in
            guard let self = self else { return }

            if let error = error {
                self.consecutiveSendFailures += 1
                self.settleLocally(messageId: messageId, reason: "Write failed")
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
                    self.handleConnectionClosed(error: nil, generation: generation)
                }
            } else {
                self.consecutiveSendFailures = 0
                self.emitDiagnostic("debug", "Message submitted to the gateway", context: [
                    "messageId": messageId,
                    "recipientId": recipientId,
                    "contentLength": content.count
                ])
            }

            completion?()
        }
    }

    /// Settles a frame that never reached the gateway, so no verdict is
    /// coming for it. Only reports to the core if this call is the one that
    /// took the id out of flight.
    ///
    /// The decision is taken here and the report is hopped, because the two
    /// callers are on different queues: the pre-flight guard runs on
    /// [messageQueue] and the write completion runs on [connectionQueue],
    /// which is the queue every inbound byte arrives on. An FFI call there
    /// waits on the global protocol mutex and stalls the reads — including the
    /// verdicts this manager is waiting for.
    private func settleLocally(messageId: String, reason: String) {
        guard verdicts.settle(messageId) else { return }
        messageQueue.async { [weak self] in
            self?.protocolInstance.reticulumSendFailedWithReason(
                messageId: messageId, reason: reason)
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
