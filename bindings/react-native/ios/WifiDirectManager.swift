//
// WifiDirectManager.swift
// OfflineProtocol
//
// The iOS peer-stream transport: TCP streams over Network framework, found
// through DNS-SD, on the infrastructure network or over AWDL (Apple's
// peer-to-peer Wi-Fi) when there is none. The name is the engine slot's, not
// the radio's: iOS has no Wi-Fi Direct API (ADR 0027).
//

import Foundation
import Network

/// The engine's peer-stream slot on iOS, over `NWListener`, `NWBrowser` and
/// `NWConnection`.
///
/// ## Every peer proves its address before it carries anything
///
/// `docs/spec/stream-framing.md` is the contract, and this manager is a plain
/// implementation of it: a TCP stream to one peer, a `u32` big-endian length
/// before every body, and the identity assertion as the first frame in each
/// direction, checked with `verifyIdentityAssertion`, the one verifier every
/// platform uses. The address it derives is the only id this manager hands the
/// core. The discovery record's `addr` is a hint the preamble must prove,
/// never a name to announce.
///
/// What the chapter asks of a receiver lives in `PeerStreamSession`, which
/// this manager forwards each per-stream event to, after `PeerStreamReader`
/// has cut the stream into whole frames. Both are Foundation-only and
/// unit-tested with fakes. What stays here:
/// - The service is `_offlineprotocol._tcp` with `txtvers=1` first and `addr`,
///   the same record the Python `PeerStreamManager` publishes, so an iPhone
///   and a host on one LAN find each other. An app's `NSBonjourServices` must
///   list `_offlineprotocol._tcp`, or local-network privacy blocks discovery;
///   the browser reports that as a diagnostic instead of failing silently.
/// - Both ends of a pair may dial. Which of two streams for one address is
///   kept is decided by `PeerStreamLinks`, the same way on both ends, so the
///   lower address dials at once and the higher waits
///   `PeerStreamDialPolicy.higherAddressDelay` to see whether the lower one reached it first. Without the wait every
///   first contact would open two streams and close one.
///
/// Everything Network framework calls back with, and every per-stream step,
/// runs on `linkQueue`, one serial queue: the listener, the browser and every
/// connection are started on it. So a stream's first frame cannot overtake its
/// connect, and no body reaches the core after its loss report. The core
/// re-adds a neighbour on any inbound body, so that ordering is what keeps a
/// reported-lost peer lost.
public class WifiDirectManager: NSObject, TransportManager {

    // MARK: - TransportManager Protocol

    public let transportId = "wifi_direct"
    public let transportName = "Peer stream (Network framework)"
    /// Read through [stateLock], because it is touched by three threads: the
    /// lifecycle writes it from the bridge queue, and the send path reads it
    /// from whichever thread the Rust callback arrives on. This is the iOS
    /// half of the `@Volatile` the Android manager's `state` carries.
    public var state: TransportState {
        stateLock.lock(); defer { stateLock.unlock() }
        return _state
    }
    public weak var delegate: TransportManagerDelegate?

    // MARK: - Constants

    /// Fifteen characters, the most a DNS-SD service label allows, and the
    /// type stream-framing.md requires.
    static let SERVICE_TYPE = "_offlineprotocol._tcp"
    /// How long a connected peer may take to prove its address. Local policy,
    /// not wire format (stream-framing.md).
    private let PREAMBLE_TIMEOUT: TimeInterval = 10.0
    /// Open streams, proved or not. The inbound share and the per-host bound
    /// are `PeerStreamDialPolicy.admitsInbound`'s.
    private static let MAX_STREAMS = PeerStreamDialPolicy.maxStreams
    /// A listener or browser that failed is rebuilt after this long.
    private static let REBUILD_DELAY: TimeInterval = 5.0
    /// How long a dial may take to become ready, whatever it waits on
    /// (resolving the record, a path, AWDL coming up, the handshake), before
    /// it is ended and left to the redial ladder. Python's `CONNECT_TIMEOUT`
    /// and Android's connect timeout bound the whole connect the same way.
    private static let DIAL_TIMEOUT: TimeInterval = 10.0
    /// Android's `Limits`: queued bytes toward one peer beyond which a body
    /// is dropped (the core retries it), and only while that peer's oldest
    /// write has been outstanding for `WRITE_STALL_MS`. The core hands a burst
    /// over at once, so a bound applied regardless drops frames for a peer
    /// that is reading.
    private static let MAX_QUEUED_BYTES = 4 * PeerStreamFraming.maxBodyBytes
    private static let WRITE_STALL_MS: Int64 = 2_000
    /// A write outstanding this long ends the stream.
    // ponytail: per write, not per unit of progress as on Android, so a 1 MiB
    // frame needs about 35 KB/s. AWDL and a LAN are far above that; move to
    // chunked progress if a slow link ever trips it.
    private static let WRITE_TIMEOUT_MS: Int64 = 30_000
    /// `kDNSServiceErr_PolicyDenied`: local-network privacy refused us.
    private static let DNS_POLICY_DENIED: Int32 = -65570

    // MARK: - Properties

    private let protocolInstance: OfflineProtocol
    private let deviceId: String

    // Message sending (event-driven, no polling)
    private let messageQueue = DispatchQueue(label: "com.offlineprotocol.wifidirect.messages")

    /// Every Network framework callback and every per-stream step runs here.
    /// See the type's documentation for why one serial queue is the invariant.
    private let linkQueue = DispatchQueue(label: "com.offlineprotocol.wifidirect.links")
    private static let linkQueueKey = DispatchSpecificKey<Bool>()

    /// Each stream from its first frame to its end: the preamble, the
    /// framing, one announced stream per address. Set once in `init`, before
    /// any callback can arrive, and never reassigned.
    private var peers: PeerStreamSession<Stream>!

    /// One TCP stream. A new one per connection and never reused, so no state
    /// from an earlier stream can be mistaken for this one's.
    private final class Stream: Hashable {
        let connection: NWConnection
        /// Whether this device opened it, which decides which of two streams
        /// for one address is kept.
        let outbound: Bool
        /// The address an outbound stream was dialed toward, for the redial.
        let dialed: String?
        let reader = PeerStreamReader()
        let writes = WriteStallWatchdog(timeoutMs: WifiDirectManager.WRITE_STALL_MS)
        var queuedBytes = 0
        /// Whether the peer sent a whole frame, and whether a dialed stream
        /// proved the address it was dialed toward. Heard and never proved is
        /// a record that does not hold the address it advertises; a stream cut
        /// mid-preamble has not been heard.
        var heard = false
        var proved = false

        init(connection: NWConnection, outbound: Bool, dialed: String?) {
            self.connection = connection
            self.outbound = outbound
            self.dialed = dialed
        }

        static func == (lhs: Stream, rhs: Stream) -> Bool { lhs === rhs }
        func hash(into hasher: inout Hasher) { hasher.combine(ObjectIdentifier(self)) }
    }

    // linkQueue only, all of it.
    private var listener: NWListener?
    private var browser: NWBrowser?
    private var streams = Set<Stream>()
    /// The endpoint each advertised address is reachable at, while it is.
    private var adverts: [String: NWEndpoint] = [:]
    /// Records whose dial was answered by a peer that did not prove the
    /// address they advertise. Left out of `adverts` until the browser reports
    /// the record again, so another record for the address is dialed, or none.
    private var unprovable = Set<NWEndpoint>()
    /// When to dial each advertised address, and when to dial it again.
    private var dialPolicy = PeerStreamDialPolicy()
    /// Bumped by every start() and stop(), so a timer armed for one run finds
    /// nothing to act on in the next.
    private var generation = 0

    /// Guards [_state] and [_isPaused], which the lifecycle writes from the
    /// bridge queue and the send path reads from any thread. Everything
    /// Network framework touches lives on `linkQueue` instead, and the proved
    /// peers in `PeerStreamLinks`, under its own lock and the same rule.
    ///
    /// Held across one field read or write and nothing else: never across a
    /// UniFFI call, a send, or a delegate callback. That is also why it can be
    /// a plain [NSLock]: no accessor calls out, so nothing can re-enter.
    private let stateLock = NSLock()
    private var _state: TransportState = .unavailable

    /// True between `pause()` and `resume()`. Mirrors `InternetManager`'s flag
    /// of the same name: pausing stops discovery and the send path, which the
    /// drain loop re-reads every iteration.
    private var _isPaused = false

    private func setState(_ newState: TransportState) {
        stateLock.lock(); defer { stateLock.unlock() }
        _state = newState
    }

    private var isPaused: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isPaused }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isPaused = newValue }
    }

    /// Whether any peer has proved an address. Reads `peers`, not
    /// `stateLock`: the proved peers are the only ones there is anything to
    /// send to.
    private var hasConnectedPeers: Bool {
        return !peers.isEmpty
    }

    // MARK: - Initialization

    public init(protocol protocolInstance: OfflineProtocol, deviceId: String) {
        self.protocolInstance = protocolInstance
        self.deviceId = deviceId
        super.init()
        linkQueue.setSpecific(key: Self.linkQueueKey, value: true)
        peers = PeerStreamSession<Stream>(
            host: self,
            preambleTimeout: PREAMBLE_TIMEOUT,
            send: { [weak self] frame, stream in
                guard let self = self, self.write(frame, to: stream) else {
                    throw TransportError.notRunning
                }
            },
            disconnect: { stream in
                // Its `.cancelled` then ends it, and finds nothing to report.
                stream.connection.cancel()
            },
            schedule: { [weak self] delay, block in
                self?.linkQueue.asyncAfter(deadline: .now() + delay, execute: block)
            },
            isOutbound: { $0.outbound }
        )
    }

    deinit {
        stop()
    }

    /// TCP with keepalive, over AWDL as well as the infrastructure network.
    /// The one place a carrier is chosen: Wi-Fi Aware, when it comes, starts
    /// here (ADR 0027). Keepalive is the Python manager's: a stream whose peer
    /// died idle ends within about thirty seconds, which is what lets the
    /// losing kind of reconnect through once the stale winner is gone.
    private static func makeParameters() -> NWParameters {
        let tcp = NWProtocolTCP.Options()
        tcp.enableKeepalive = true
        tcp.keepaliveIdle = 15
        tcp.keepaliveInterval = 5
        tcp.keepaliveCount = 3
        tcp.connectionTimeout = 10
        let parameters = NWParameters(tls: nil, tcp: tcp)
        parameters.includePeerToPeer = true
        return parameters
    }

    // MARK: - TransportManager Implementation

    public func isAvailable() -> Bool {
        return true
    }

    public func start() throws {
        guard state != .running else {
            throw TransportError.alreadyRunning
        }

        emitDiagnostic("info", "Starting peer-stream transport", context: [
            "deviceId": deviceId
        ])

        // An explicit start() means "run": a pause() from a previous session
        // must not leave this fresh transport connected-but-mute. Mirrors
        // `InternetManager.start()`.
        isPaused = false
        updateState(.starting)

        var failure: Error?
        onLinkQueueSync {
            generation += 1
            do {
                try startListening()
                startBrowsing()
            } catch {
                failure = error
            }
        }
        if let failure = failure {
            updateState(.stopped)
            throw TransportError.startFailed(failure.localizedDescription)
        }

        updateState(.running)
        try? protocolInstance.wifiDirectStatusChanged(isConnected: true)
        emitDiagnostic("info", "Peer-stream transport started")
    }

    public func stop() {
        guard state == .running || state == .starting else {
            return
        }

        updateState(.stopping)

        onLinkQueueSync {
            // Everything is forgotten first. Each callback checks that its
            // listener, browser or stream is still this manager's, so from
            // here one already queued finds nothing to attach to; forgotten
            // after endAll(), a stream that became ready in between would be
            // announced into the just-emptied table.
            generation += 1
            let old = (listener: listener, browser: browser, streams: streams)
            listener = nil
            browser = nil
            streams = []
            adverts = [:]
            unprovable = []
            dialPolicy.reset()

            // Report every proved peer lost while the core still holds its
            // link, before the layer goes down, on linkQueue so no delivery
            // lands after a loss report.
            peers.endAll()

            old.listener?.cancel()
            old.browser?.cancel()
            old.streams.forEach { $0.connection.cancel() }
        }

        try? protocolInstance.wifiDirectStatusChanged(isConnected: false)

        updateState(.stopped)
        emitDiagnostic("info", "Peer-stream transport stopped")
    }

    public func pause() {
        // Set before discovery stops. This is what actually pauses the send
        // path, see `isPaused`; stopping the browser only stops finding new
        // peers. The listener stays up, so a peer can still reach us.
        isPaused = true
        onLinkQueueSync { stopBrowsing() }
    }

    public func resume() {
        isPaused = false
        if state == .running {
            onLinkQueueSync { startBrowsing() }
            // Drain any messages that accumulated while paused. Required
            // rather than tidy: the core does not re-issue
            // `onMessagesAvailable` for messages it already announced, and
            // this manager has no fallback timer to pick them up.
            drainAndSendMessages()
        }
    }

    // MARK: - Message Handling (Event-Driven)

    /// Called by the Rust transport callback when new outgoing messages are
    /// available. Goes straight to the drain: `state` and the proved peers are
    /// both readable from any thread.
    public func onMessagesAvailable() {
        drainAndSendMessages()
    }

    /// Drains the Rust message queue, framing each body and handing it to
    /// `linkQueue` to write.
    ///
    /// Unbounded, where the Android manager's mirror of this spends a batch
    /// budget and reposts. The budget there exists because that looper is
    /// shared with the framework callbacks; `messageQueue` is this manager's
    /// alone, and no lifecycle path waits on it. Unbounded is not
    /// unconditional, though: the state and `isPaused` are re-read inside the
    /// loop, because a `stop()` or `pause()` landing after the guard would
    /// otherwise leave every remaining iteration taking the core's global
    /// mutex for a message that is then dropped.
    private func drainAndSendMessages() {
        guard !isPaused, state == .running, hasConnectedPeers else { return }

        messageQueue.async { [weak self] in
            guard let self = self else { return }
            while !self.isPaused, self.state == .running,
                  let message = self.protocolInstance.wifiDirectGetNextMessage() {
                self.sendMessage(recipientId: message.recipientId, data: Data(message.data))
            }
        }
    }

    /// Frames one body and writes it to the stream that proved `recipientId`.
    ///
    /// Never broadcasts. The core returns a body only for an address a stream
    /// proved, so a recipient with no stream here means it ended between the
    /// core's answer and this lookup; the body is dropped and the core's
    /// acknowledgement and retry cover it.
    private func sendMessage(recipientId: String, data: Data) {
        guard let frame = PeerStreamFraming.frame(data) else {
            emitDiagnostic("error", "Peer-stream body dropped: outside the frame bounds", context: [
                "recipientId": recipientId,
                "dataSize": data.count
            ])
            return
        }
        linkQueue.async { [weak self] in
            guard let self = self else { return }
            guard let stream = self.peers.handle(for: recipientId), self.streams.contains(stream) else {
                self.emitDiagnostic("warning", "Peer-stream body dropped: no stream holds the recipient", context: [
                    "recipientId": recipientId,
                    "dataSize": data.count
                ])
                return
            }
            if !self.write(frame, to: stream) {
                self.emitDiagnostic("warning", "Peer-stream body dropped: the peer is not reading", context: [
                    "recipientId": recipientId,
                    "dataSize": data.count
                ])
            }
        }
    }

    /// Writes one frame, or returns false when the peer is stalled and its
    /// queue is over the bound. `linkQueue` only; the completion and the
    /// deadline run there too, since the connection was started on it.
    private func write(_ frame: Data, to stream: Stream) -> Bool {
        let now = MonotonicClock.nowMs()
        if stream.queuedBytes + frame.count > Self.MAX_QUEUED_BYTES,
           stream.writes.stalledAgeMs(nowMs: now) != nil {
            return false
        }
        stream.queuedBytes += frame.count
        let token = stream.writes.arm(nowMs: now)
        stream.connection.send(content: frame, completion: .contentProcessed { [weak stream] error in
            guard let stream = stream else { return }
            stream.queuedBytes -= frame.count
            stream.writes.disarm(token)
            // A failed write ends the stream; its `.failed` or `.cancelled`
            // does the bookkeeping.
            if error != nil { stream.connection.cancel() }
        })
        linkQueue.asyncAfter(deadline: .now() + .milliseconds(Int(Self.WRITE_TIMEOUT_MS))) { [weak self, weak stream] in
            guard let self = self, let stream = stream, self.streams.contains(stream),
                  let age = stream.writes.stalledAgeMs(nowMs: MonotonicClock.nowMs()),
                  age >= Self.WRITE_TIMEOUT_MS else { return }
            self.emitDiagnostic("warning", "Peer stream closed: a write made no progress", context: [
                "ageMs": age
            ])
            stream.connection.cancel()
        }
        return true
    }

    // MARK: - Listening and discovery (linkQueue only)

    private func startListening() throws {
        let listener = try NWListener(using: Self.makeParameters())
        let address = protocolInstance.localAddress()
        listener.service = NWListener.Service(
            name: PeerStreamFraming.instanceName(address: address),
            type: Self.SERVICE_TYPE,
            domain: nil,
            txtRecord: PeerStreamFraming.txtRecord(address: address)
        )
        listener.stateUpdateHandler = { [weak self, weak listener] newState in
            guard let self = self, let listener = listener, listener === self.listener else { return }
            self.listenerChanged(newState)
        }
        listener.newConnectionHandler = { [weak self, weak listener] connection in
            guard let self = self, let listener = listener, listener === self.listener,
                  PeerStreamDialPolicy.admitsInbound(
                      from: Self.host(of: connection),
                      inboundFrom: self.streams.filter { !$0.outbound }.map { Self.host(of: $0.connection) },
                      open: self.streams.count) else {
                connection.cancel()
                return
            }
            self.open(Stream(connection: connection, outbound: false, dialed: nil))
        }
        self.listener = listener
        listener.start(queue: linkQueue)
    }

    /// The remote host an accepted connection came from, nil if unknown
    /// (unknowns share one bound).
    private static func host(of connection: NWConnection) -> String? {
        guard case .hostPort(let host, _) = connection.endpoint else { return nil }
        return "\(host)"
    }

    private func listenerChanged(_ newState: NWListener.State) {
        switch newState {
        case .waiting(let error):
            reportNetworkError("Peer-stream listener waiting", error)
        case .failed(let error):
            // Suspension in the background ends a listener this way.
            reportNetworkError("Peer-stream listener failed", error)
            listener?.cancel()
            listener = nil
            let expected = generation
            linkQueue.asyncAfter(deadline: .now() + Self.REBUILD_DELAY) { [weak self] in
                guard let self = self, self.generation == expected, self.listener == nil else { return }
                do {
                    try self.startListening()
                } catch {
                    self.emitDiagnostic("error", "Peer-stream listener could not restart", context: [
                        "error": error.localizedDescription
                    ])
                }
            }
        default:
            break
        }
    }

    private func startBrowsing() {
        guard browser == nil, !isPaused else { return }
        let browser = NWBrowser(
            for: .bonjourWithTXTRecord(type: Self.SERVICE_TYPE, domain: nil),
            using: Self.makeParameters()
        )
        browser.stateUpdateHandler = { [weak self, weak browser] newState in
            guard let self = self, let browser = browser, browser === self.browser else { return }
            self.browserChanged(newState)
        }
        browser.browseResultsChangedHandler = { [weak self, weak browser] results, changes in
            guard let self = self, let browser = browser, browser === self.browser else { return }
            self.advertsChanged(results, changes)
        }
        self.browser = browser
        browser.start(queue: linkQueue)
    }

    private func stopBrowsing() {
        browser?.cancel()
        browser = nil
        adverts = [:]
        unprovable = []
    }

    private func browserChanged(_ newState: NWBrowser.State) {
        switch newState {
        case .waiting(let error):
            reportNetworkError("Peer-stream discovery waiting", error)
        case .failed(let error):
            reportNetworkError("Peer-stream discovery failed", error)
            stopBrowsing()
            let expected = generation
            linkQueue.asyncAfter(deadline: .now() + Self.REBUILD_DELAY) { [weak self] in
                guard let self = self, self.generation == expected else { return }
                self.startBrowsing()
            }
        default:
            break
        }
    }

    private func reportNetworkError(_ message: String, _ error: NWError) {
        if case .dns(let code) = error, code == Self.DNS_POLICY_DENIED {
            emitDiagnostic("error", "Local network access denied: list _offlineprotocol._tcp in NSBonjourServices and set NSLocalNetworkUsageDescription", context: [
                "error": error.localizedDescription
            ])
        } else {
            emitDiagnostic("warning", message, context: ["error": error.localizedDescription])
        }
    }

    /// Records each advertised address from every record the browser holds,
    /// and dials the ones a change added or changed. An advert with no `addr`
    /// is a device with no identity yet, which has no preamble to send, and
    /// ours is skipped by its address.
    private func advertsChanged(_ results: Set<NWBrowser.Result>, _ changes: Set<NWBrowser.Result.Change>) {
        guard let local = protocolInstance.localAddress() else { return }
        var fresh = Set<NWEndpoint>()
        for change in changes {
            if case .added(let result) = change { fresh.insert(result.endpoint) }
            if case .changed(_, let result, _) = change { fresh.insert(result.endpoint) }
        }
        // A record reported again may now hold what it advertises.
        unprovable.subtract(fresh)
        recordAdverts(results, fresh: fresh, local: local)
        for (address, endpoint) in adverts where fresh.contains(endpoint) {
            // The lower address's stream is the one both ends keep, so the
            // lower one dials at once and the higher gives it time.
            let weAreLower = PeerStreamLinks<Stream>.newStreamWins(
                outbound: true, localAddress: local, peer: address)
            if let delay = dialPolicy.discovered(
                address, weAreLower: weAreLower, held: peers.handle(for: address) != nil) {
                scheduleDial(address, after: delay)
            }
        }
    }

    private func recordAdverts(_ results: Set<NWBrowser.Result>, fresh: Set<NWEndpoint>, local: String) {
        var records: [(address: String, endpoint: NWEndpoint)] = []
        for result in results {
            guard let address = Self.advertisedAddress(result), address != local else { continue }
            records.append((address, result.endpoint))
        }
        unprovable.formIntersection(records.map { $0.endpoint })
        adverts = PeerStreamDialPolicy.adverts(
            records, fresh: fresh, current: adverts, unprovable: unprovable)
    }

    /// The peer address a record advertises, or nil for a record that is not
    /// a peer hint. A record carrying `sid` is a service instance under the
    /// DNS-SD mapping chapter: it names the same host and address as the peer
    /// record, so taking it as a peer would open a second stream to one host
    /// per service published there. The stream chapter says a peer browser
    /// MUST ignore it, as the Python manager does.
    private static func advertisedAddress(_ result: NWBrowser.Result) -> String? {
        guard case .bonjour(let txt) = result.metadata else { return nil }
        if txt.dictionary.keys.contains("sid") { return nil }
        guard let address = txt["addr"], !address.isEmpty else { return nil }
        return address
    }

    /// Arms a dial `dialPolicy` decided on.
    private func scheduleDial(_ address: String, after delay: TimeInterval) {
        let expected = generation
        linkQueue.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let self = self, self.generation == expected else { return }
            self.dial(address)
        }
    }

    /// Opens a stream toward `address`, unless it went away or is already
    /// held (an inbound stream got here first), or later when no slot is free.
    private func dial(_ address: String) {
        guard !isPaused, let endpoint = adverts[address], peers.handle(for: address) == nil else {
            dialPolicy.abandoned(address)
            return
        }
        guard streams.count < Self.MAX_STREAMS else {
            if let delay = dialPolicy.noSlot(address) { scheduleDial(address, after: delay) }
            return
        }
        let stream = Stream(
            connection: NWConnection(to: endpoint, using: Self.makeParameters()),
            outbound: true,
            dialed: address
        )
        peers.claim(address, for: stream)
        open(stream)
    }

    // MARK: - Streams (linkQueue only)

    private func open(_ stream: Stream) {
        streams.insert(stream)
        stream.connection.stateUpdateHandler = { [weak self, weak stream] newState in
            guard let self = self, let stream = stream, self.streams.contains(stream) else { return }
            switch newState {
            case .ready:
                self.peers.connected(stream)
                self.receive(on: stream)
            case .failed, .cancelled:
                self.end(stream)
            default:
                break
            }
        }
        stream.connection.start(queue: linkQueue)
        guard stream.outbound else { return }
        // The TCP connect timeout does not cover resolving the record, and a
        // dial toward a host that left without a goodbye can sit there with no
        // state change, holding a slot and its address's one dial.
        linkQueue.asyncAfter(deadline: .now() + Self.DIAL_TIMEOUT) { [weak stream] in
            guard let stream = stream else { return }
            if case .ready = stream.connection.state { return }
            stream.connection.cancel()
        }
    }

    /// Reads the stream and hands each whole frame to the session, which owns
    /// the preamble, the frame rules and every call to the core. A reader
    /// refusal ends the stream, and the end reports the loss if the peer was
    /// announced.
    private func receive(on stream: Stream) {
        stream.connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self, weak stream] content, _, isComplete, error in
            guard let self = self, let stream = stream, self.streams.contains(stream) else { return }
            if let content = content, !content.isEmpty {
                for result in stream.reader.append(content) {
                    switch result {
                    case .success(let frame):
                        stream.heard = true
                        self.peers.received(frame, from: stream)
                        if !stream.proved, let address = stream.dialed,
                           self.peers.provedAddress(of: stream) == address {
                            stream.proved = true
                            self.dialPolicy.proved(address)
                        }
                    case .failure(let refusal):
                        self.emitDiagnostic("warning", "Peer stream refused", context: ["reason": refusal.reason])
                        stream.connection.cancel()
                        return
                    }
                }
            }
            if isComplete || error != nil {
                stream.connection.cancel()
                return
            }
            self.receive(on: stream)
        }
    }

    /// The stream is over. Reported lost if it held an address, and an
    /// outbound one is dialed again, later each time, while its advert stays.
    /// A stream refused because the other one for its address won finds the
    /// address held and does not redial.
    private func end(_ stream: Stream) {
        guard streams.remove(stream) != nil else { return }
        peers.ended(stream)
        stream.writes.reset()
        stream.connection.cancel()
        // Answered by a peer that never proved the address its record
        // advertises: a device on the LAN claiming another's address, or one
        // that moved. Redialing it would fail the same way on the ladder for
        // as long as the record lives, and keep the real record for the
        // address undialed. A dial that lost the tie-break did prove it.
        if stream.dialed != nil, stream.heard, !stream.proved,
           let local = protocolInstance.localAddress() {
            unprovable.insert(stream.connection.endpoint)
            recordAdverts(browser?.browseResults ?? [], fresh: [], local: local)
        }
        guard let address = stream.dialed,
              let delay = dialPolicy.ended(
                address, advertised: adverts[address] != nil,
                held: peers.handle(for: address) != nil) else { return }
        scheduleDial(address, after: delay)
    }

    /// Runs `body` on `linkQueue` and waits, or inline when already there.
    /// Inline matters: `deinit` calls `stop()`, and the last reference can be
    /// released inside a `linkQueue` block, where a `sync` onto the same
    /// queue would trap.
    private func onLinkQueueSync(_ body: () -> Void) {
        if DispatchQueue.getSpecific(key: Self.linkQueueKey) == true {
            body()
        } else {
            linkQueue.sync(execute: body)
        }
    }

    // MARK: - State Management

    private func updateState(_ newState: TransportState) {
        setState(newState)
        // Deliberately outside the lock: the delegate is the bridge module,
        // and calling into it while holding [stateLock] would put arbitrary
        // downstream work, including UniFFI calls, inside this manager's
        // critical section.
        delegate?.transportManager(self, didChangeState: newState)
    }

    // MARK: - Diagnostics

    private func emitDiagnostic(_ level: String, _ message: String, context: [String: Any] = [:]) {
        delegate?.transportManager(self, didEmitDiagnostic: level, message: message, context: context)
    }
}

// MARK: - PeerStreamHost

/// The core, as `PeerStreamSession` sees it. These are the only places this
/// manager hands the core a peer id, and each is the address a preamble
/// proved. Called on `linkQueue`.
extension WifiDirectManager: PeerStreamHost {
    func peerStreamIdentityAssertion() throws -> Data {
        return Data(try protocolInstance.identityAssertion(signedData: []))
    }

    func peerStreamLocalAddress() -> String? {
        return protocolInstance.localAddress()
    }

    func peerStreamVerify(_ assertion: Data) throws -> String {
        return try verifyIdentityAssertion(assertion: [UInt8](assertion))
    }

    func peerStreamConnected(_ address: String) {
        do {
            try protocolInstance.wifiDirectPeerConnected(peerId: address)
        } catch {
            emitDiagnostic("error", "Error announcing peer-stream peer", context: [
                "error": error.localizedDescription
            ])
        }
    }

    func peerStreamReceived(_ address: String, _ body: Data) {
        do {
            try protocolInstance.wifiDirectMessageReceived(senderId: address, data: [UInt8](body))
        } catch {
            // The core refused this body. That is about the message, not the
            // peer, so the peer stays.
            emitDiagnostic("warning", "Peer-stream body refused by the core", context: [
                "address": address,
                "error": error.localizedDescription
            ])
        }
    }

    func peerStreamLost(_ address: String) {
        do {
            try protocolInstance.wifiDirectPeerDisconnected(peerId: address)
        } catch {
            emitDiagnostic("error", "Error reporting peer-stream peer lost", context: [
                "error": error.localizedDescription
            ])
        }
        emitDiagnostic("info", "Peer-stream peer disconnected", context: ["address": address])
    }

    func peerStreamDiagnostic(_ level: String, _ message: String, _ context: [String: Any]) {
        emitDiagnostic(level, message, context: context)
    }
}

extension WifiDirectManager: @unchecked Sendable {}
