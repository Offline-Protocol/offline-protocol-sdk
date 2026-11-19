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
import UIKit
// Scoped: NetworkExtension also exports a legacy `NWEndpoint`, which would make
// every use of Network's ambiguous in this file.
import class NetworkExtension.NEHotspotConfiguration
import class NetworkExtension.NEHotspotConfigurationManager
import enum NetworkExtension.NEHotspotConfigurationError
import let NetworkExtension.NEHotspotConfigurationErrorDomain

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
///
/// A third source of streams is a page that dialled this device's WebRTC
/// Direct listener (`openWebrtcDirect`, ADR 0039): `WebrtcDirectListener`
/// runs its socket and the engine's listener on `linkQueue` too, and each
/// stream it proves is a row in the same table, under the same caps and the
/// same one-stream-per-address rule (docs/bridges/swift.md S10).
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
    /// How long a dialed stream must hold its address after its proof to
    /// count as having carried its peer with no body: the keepalive window.
    /// Android's `Limits.carriedAfterMs`.
    private static let CARRIED_AFTER: TimeInterval = 30.0
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
    private var peers: PeerStreamSession<Held>!

    /// One row in the stream table, of either kind: a TCP stream, or a stream
    /// a page dialled to the WebRTC Direct listener. A new object per stream,
    /// compared by identity and never by a value a later stream could share,
    /// so no state from an earlier stream can be mistaken for this one's.
    private class Held: Hashable {
        /// Whether this device opened it, which decides which of two streams
        /// for one address is kept.
        let outbound: Bool

        init(outbound: Bool) {
            self.outbound = outbound
        }

        /// The remote host an inbound stream came from, for the per-host
        /// bound; nil if unknown (unknowns share one bound).
        var remoteHost: String? { nil }

        /// A stream a page dialled to the listener, which `PeerStreamLinks`
        /// orders apart.
        var dialled: Bool { false }

        /// Ends the transport under the row. The end's bookkeeping is the
        /// owner's (`end(_:)`, `endDialled(_:)`).
        func abort() {}

        static func == (lhs: Held, rhs: Held) -> Bool { lhs === rhs }
        func hash(into hasher: inout Hasher) { hasher.combine(ObjectIdentifier(self)) }
    }

    /// One TCP stream.
    private final class Stream: Held {
        let connection: NWConnection
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
        /// Whether a body followed the preamble, and when a dialed stream
        /// proved its address: together they say whether the stream carried
        /// the peer, which is what starts the redial ladder over (`end(_:)`).
        var carried = false
        var provedAt: TimeInterval?

        init(connection: NWConnection, outbound: Bool, dialed: String?) {
            self.connection = connection
            self.dialed = dialed
            super.init(outbound: outbound)
        }

        override var remoteHost: String? { WifiDirectManager.host(of: connection) }

        /// Its `.cancelled` then ends it.
        override func abort() {
            connection.cancel()
        }
    }

    /// A stream a page dialled to the WebRTC Direct listener (ADR 0039). The
    /// listener owns the channel, its framing and its writer, so the row
    /// holds only the listener's id for it. Inbound, from the address the
    /// page's first check came from.
    private final class DialledStream: Held {
        let id: UInt64
        let remoteIp: String
        private weak var listener: WebrtcDirectListener?
        /// The listener ended it, or was told to: it is never told twice.
        private var ended = false

        init(id: UInt64, remoteIp: String, listener: WebrtcDirectListener) {
            self.id = id
            self.remoteIp = remoteIp
            self.listener = listener
            super.init(outbound: false)
        }

        override var remoteHost: String? { remoteIp }
        override var dialled: Bool { true }

        func endedByListener() {
            ended = true
        }

        /// One body; the listener frames it. Throws when it is refused.
        func send(_ body: Data) throws {
            guard !ended, let listener = listener else { throw TransportError.notRunning }
            try listener.send(stream: id, body: body)
        }

        /// Its `closed` then ends it, and finds nothing to report.
        override func abort() {
            guard !ended else { return }
            ended = true
            listener?.closeStream(id)
        }
    }

    // linkQueue only, all of it.
    private var listener: NWListener?
    private var browser: NWBrowser?
    /// Every open row, of both kinds: what the caps count.
    private var streams = Set<Held>()
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
    /// The fast link this device is joining or has joined, if any.
    private var fastLink: FastLinkJoin?
    /// `wifiDirect.fastLink` (ADR 0038). Set before `start()`, which
    /// declares it with the roles.
    var fastLinkMode: FastLinkMode = .any
    /// The WebRTC Direct listener a page dials, between `openWebrtcDirect`
    /// and its close, what closes it when the Wi-Fi network or the
    /// foreground goes, and its held streams by the listener's id.
    private var webrtc: WebrtcDirectListener?
    private var webrtcPath: NWPathMonitor?
    private var webrtcBackground: NSObjectProtocol?
    private var dialled: [UInt64: DialledStream] = [:]

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
        peers = PeerStreamSession<Held>(
            host: self,
            preambleTimeout: PREAMBLE_TIMEOUT,
            send: { [weak self] frame, held in
                // Only a TCP stream sends a preamble through here: the
                // listener sends ours on a dialled one.
                guard let self = self, let stream = held as? Stream,
                      self.write(frame, to: stream) else {
                    throw TransportError.notRunning
                }
            },
            disconnect: { held in
                // Its end then finds nothing to report.
                held.abort()
            },
            schedule: { [weak self] delay, block in
                self?.linkQueue.asyncAfter(deadline: .now() + delay, execute: block)
            },
            isOutbound: { $0.outbound },
            isDialled: { $0.dialled }
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
        // An iPhone joins a fast link and cannot host one (ADR 0038), as far
        // as the application's `wifiDirect.fastLink` allows.
        try? protocolInstance.wifiDirectSetFastLinkRoles(
            canHost: false,
            canJoin: fastLinkMode != .off,
            acceptUnrequested: fastLinkMode == .any
        )
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

            // The fast link ends as this device's own end, which never counts
            // against the peer, before any stream is reported lost: after
            // endAll() the engine has already seen the link's stream drop and
            // ended it `lost`, a miss charged to a peer that offered it.
            try? protocolInstance.wifiDirectSetFastLinkRoles(canHost: false, canJoin: false, acceptUnrequested: false)
            endFastLink(reason: "closed")

            // Report every proved peer lost while the core still holds its
            // link, before the layer goes down, on linkQueue so no delivery
            // lands after a loss report.
            peers.endAll()

            old.listener?.cancel()
            old.browser?.cancel()
            old.streams.forEach { $0.abort() }
            // Its streams were forgotten and reported lost above, so the
            // `closed` it reports for each finds nothing.
            closeWebrtcDirectOnLinkQueue(reason: WebrtcDirectListener.closedStopped)
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
        onLinkQueueSync {
            stopBrowsing()
            // iOS drops a joined network 15 s after the app leaves the
            // foreground and deletes its passphrase. Leaving now, and saying
            // so, lets the host end its half at once rather than when
            // keepalive notices, about 40 s later.
            endFastLink(reason: "background")
            // A WebRTC Direct listener lives only in the foreground (ADR 0039
            // decision 8): suspended, its socket answers no page, and its
            // streams would wait out their consent instead of ending now.
            closeWebrtcDirectOnLinkQueue(reason: WebrtcDirectListener.closedPaused)
        }
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
        // The same wake carries fast-link directives (ADR 0038). Async: the
        // core calls this holding its mutex, and draining a directive takes it.
        linkQueue.async { [weak self] in self?.drainFastLinkDirectives() }
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
        guard !data.isEmpty, data.count <= PeerStreamFraming.maxBodyBytes else {
            emitDiagnostic("error", "Peer-stream body dropped: outside the frame bounds", context: [
                "recipientId": recipientId,
                "dataSize": data.count
            ])
            return
        }
        linkQueue.async { [weak self] in
            guard let self = self else { return }
            guard let held = self.peers.handle(for: recipientId), self.streams.contains(held) else {
                self.emitDiagnostic("warning", "Peer-stream body dropped: no stream holds the recipient", context: [
                    "recipientId": recipientId,
                    "dataSize": data.count
                ])
                return
            }
            if let row = held as? DialledStream {
                // The listener frames it. A refusal is a stream that ended or
                // has more than its bound queued, which also closes it: the
                // body is dropped and the core retries it.
                do {
                    try row.send(data)
                } catch {
                    self.emitDiagnostic("warning", "Peer-stream body dropped: the dialled stream refused it", context: [
                        "recipientId": recipientId,
                        "dataSize": data.count
                    ])
                }
                return
            }
            guard let stream = held as? Stream, let frame = PeerStreamFraming.frame(data) else { return }
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
                      inboundFrom: self.streams.filter { !$0.outbound }.map { $0.remoteHost },
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
                        if !stream.carried, stream.dialed != nil,
                           self.peers.provedAddress(of: stream) == stream.dialed {
                            if stream.proved {
                                stream.carried = true
                            } else {
                                stream.proved = true
                                stream.provedAt = ProcessInfo.processInfo.systemUptime
                            }
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
        // The ladder starts over only after a stream that carried its peer: a
        // body after the preamble, or the address held through the keepalive
        // window. A proof alone is not enough: a stream the peer refuses for
        // its own held one (a stale stream there) proves the address too, and
        // resetting on it redialed every second until that stream died. A body
        // alone is too narrow: two peers with a session may reconnect and send
        // nothing. Android's `Ran.carried` is the same rule.
        if let address = stream.dialed,
           stream.carried || stream.provedAt.map({
               ProcessInfo.processInfo.systemUptime - $0 >= Self.CARRIED_AFTER
           }) == true {
            dialPolicy.proved(address)
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
        // A stream that is not the fast link's own reached the link's peer
        // first (both phones back on one home network): the link is ended
        // `superseded` before the stream is announced. Announced first, the
        // engine took it for the link's and marked the link up, the join
        // never dialled the network the user had just joined, and leaving
        // the home network for it then lost both. Only a first announcement
        // for the address reaches here, so an up link, whose own stream
        // holds the address, never does.
        if let pending = fastLink, pending.peer == address,
           let held = peers.handle(for: address), held !== (pending.stream as? Stream) {
            if pending.joined && !Self.runsOffTheGroup(held, host: pending.host) {
                // Over the group itself, from the general Bonjour dial: an
                // Android below 13 advertises on the group's interface too,
                // and that dial needs no DHCP address, so it can prove the
                // host before the link's own, over a link-local address with
                // no gateway on its path yet. It is the link's stream, and
                // the link's own dial, if any, is let go.
                if let own = pending.stream as? Stream { own.connection.cancel() }
                pending.stream = held
            } else {
                emitDiagnostic("info", "Fast link: another stream reached the peer first", context: [
                    "linkId": pending.linkId,
                ])
                endFastLink(reason: "superseded")
            }
        }
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

// MARK: - WebRTC Direct (ADR 0039)

/// A page on the same Wi-Fi network dials this device from a code it shows,
/// with no internet and no server (ADR 0039, docs/spec/stream-framing.md,
/// "Dialling a listener from a browser"). The listener itself (admission by
/// authenticated check only, DTLS, the channel, the preamble and the framing)
/// is the engine's `webrtc_direct_*`; `WebrtcDirectListener` moves datagrams
/// and ticks for it on `linkQueue`, and this extension decides what the core
/// hears: each stream it proves is a row in the one stream table
/// (docs/bridges/swift.md S10).
extension WifiDirectManager: WebrtcDirectSink {
    /// How long `openWebrtcDirect` waits for Network framework to name the
    /// Wi-Fi interface. Its first path update is immediate on a device.
    private static let WEBRTC_DIRECT_PATH_WAIT: TimeInterval = 2.0

    /// The engine's `webrtc_direct_*` methods, adapted to the listener's
    /// types. The generated `WebrtcDirectCode` prints every field, the
    /// password-carrying fragment among them, so it goes no further than
    /// this adapter.
    private struct WebrtcDirectFfi: WebrtcDirectEngine {
        let engine: OfflineProtocol

        func open(localIp: String, localPort: UInt16) throws -> WebrtcDirectDialCode {
            let code = try engine.webrtcDirectOpen(localIp: localIp, localPort: localPort)
            return WebrtcDirectDialCode(
                fragment: code.fragment, host: code.host, port: code.port, address: code.address)
        }

        func close() {
            engine.webrtcDirectClose()
        }

        func datagramReceived(sourceIp: String, sourcePort: UInt16, data: Data) throws {
            try engine.webrtcDirectDatagramReceived(
                sourceIp: sourceIp, sourcePort: sourcePort, data: [UInt8](data))
        }

        func nextDatagram() -> WebrtcDirectListener.Datagram? {
            guard let datagram = engine.webrtcDirectNextDatagram() else { return nil }
            return WebrtcDirectListener.Datagram(
                destinationIp: datagram.destinationIp,
                destinationPort: datagram.destinationPort,
                data: Data(datagram.data))
        }

        func nextEvent() -> WebrtcDirectListener.Event? {
            guard let event = engine.webrtcDirectNextEvent() else { return nil }
            switch event.kind {
            case .proved:
                // The address the listener's verifier derived, never a
                // claimed one.
                return .proved(
                    stream: event.stream, address: event.address ?? "",
                    remoteIp: event.remoteIp ?? "", remotePort: event.remotePort ?? 0)
            case .body:
                return .body(stream: event.stream, data: Data(event.data ?? []))
            case .closed:
                return .closed(stream: event.stream)
            }
        }

        func send(stream: UInt64, body: Data) throws {
            try engine.webrtcDirectSend(stream: stream, body: [UInt8](body))
        }

        func closeStream(_ stream: UInt64) {
            engine.webrtcDirectCloseStream(stream: stream)
        }

        func tick() -> UInt32? {
            engine.webrtcDirectTick()
        }
    }

    /// Opens a WebRTC Direct listener on this device's private IPv4 address
    /// on the Wi-Fi network, and returns its dial code. A page on that
    /// network dials the code, and each dial that proves an address is a
    /// peer stream in this transport's one table. A listener already open is
    /// closed first, its streams reported lost and its code dead: the socket,
    /// certificate and credentials are fresh on every open.
    ///
    /// The socket is bound to the Wi-Fi interface Network framework reports
    /// and to its one address, never a wildcard, so nothing it sends follows
    /// the default route onto cellular. The listener lives only in the
    /// foreground (ADR 0039 decision 8): it closes when the application
    /// leaves it, when the transport pauses or stops, and when the Wi-Fi
    /// network goes or loses the bound address, since its code then names an
    /// address no page can reach. Each of those closes, and a socket failure,
    /// is reported to `onClosed` once with its reason
    /// (`WebrtcDirectListener.closedNetwork` and the others); a close of the
    /// application's own (`closeWebrtcDirect`) or by the next open is not.
    ///
    /// Blocks for the open (a certificate is made), so never on the main
    /// thread. Throws when the transport is not running or paused, the device
    /// is on no Wi-Fi network with a private address, or the engine refuses
    /// (no identity yet, or a library built without `webrtc-direct`). The
    /// code's fragment carries the ICE password: the caller shows it, and
    /// nothing here logs it.
    public func openWebrtcDirect(
        onClosed: @escaping (_ reason: String) -> Void = { _ in }
    ) throws -> WebrtcDirectDialCode {
        guard state == .running else { throw TransportError.notRunning }
        guard !isPaused else {
            throw TransportError.invalidState("a WebRTC Direct listener opens only in the foreground")
        }
        guard let wifi = Self.wifiInterfaceName() else {
            throw TransportError.notAvailable("not on a Wi-Fi network: a WebRTC Direct listener binds the network's own address")
        }
        guard let ip = WebrtcDirectListener.listenAddress(
            WebrtcDirectListener.ipv4Addresses(ofInterface: wifi)) else {
            throw TransportError.notAvailable("the Wi-Fi network gave this device no private IPv4 address")
        }
        var result: Result<WebrtcDirectDialCode, Error> = .failure(TransportError.notRunning)
        onLinkQueueSync {
            closeWebrtcDirectOnLinkQueue()
            // Stopped or paused while the interface was looked up.
            guard state == .running, !isPaused else { return }
            do {
                let socket = try WebrtcDirectListener.bindSocket(ip: ip, interface: wifi)
                let listener = WebrtcDirectListener(
                    engine: WebrtcDirectFfi(engine: protocolInstance),
                    sink: self,
                    queue: linkQueue,
                    diagnostic: { [weak self] level, message, context in
                        self?.emitDiagnostic(level, message, context: context)
                    },
                    onClosed: onClosed,
                    onSocketLost: { [weak self] lost in
                        // Its streams end now rather than at their consent
                        // timeout.
                        self?.linkQueue.async { [weak self] in
                            guard let self = self, self.webrtc === lost else { return }
                            self.closeWebrtcDirectOnLinkQueue(reason: WebrtcDirectListener.closedSocket)
                        }
                    })
                let code = try listener.open(socket: socket)
                webrtc = listener
                watchWebrtcDirect(listener, interface: wifi, ip: ip)
                result = .success(code)
            } catch {
                result = .failure(error)
            }
        }
        return try result.get()
    }

    /// Closes the WebRTC Direct listener, if one is open. Every dialled
    /// stream ends and each that held an address is reported lost once; the
    /// code admits nothing again. False when none was open.
    @discardableResult
    public func closeWebrtcDirect() -> Bool {
        var closed = false
        onLinkQueueSync { closed = closeWebrtcDirectOnLinkQueue() }
        return closed
    }

    /// Closes the listener, if one is open. `reason` is nil for a close the
    /// application asked for (its own, or an open replacing the listener),
    /// and otherwise the token the listener reports to the application once.
    /// linkQueue only.
    @discardableResult
    fileprivate func closeWebrtcDirectOnLinkQueue(reason: String? = nil) -> Bool {
        webrtcPath?.cancel()
        webrtcPath = nil
        if let observer = webrtcBackground {
            NotificationCenter.default.removeObserver(observer)
            webrtcBackground = nil
        }
        guard let listener = webrtc else { return false }
        webrtc = nil
        // The listener reports every proved stream closed, and each ends
        // through the table as it is; any it did not report end here.
        listener.close(reason: reason)
        for row in dialled.values { endDialled(row) }
        return true
    }

    /// The name of the interface Network framework reports as Wi-Fi, or nil.
    /// Waits on a queue of its own, so never call it on `linkQueue`'s.
    private static func wifiInterfaceName() -> String? {
        let monitor = NWPathMonitor(requiredInterfaceType: .wifi)
        let queue = DispatchQueue(label: "com.offlineprotocol.wifidirect.webrtc-path")
        let looked = DispatchSemaphore(value: 0)
        var name: String?
        monitor.pathUpdateHandler = { path in
            name = path.availableInterfaces.first { $0.type == .wifi }?.name
            looked.signal()
        }
        monitor.start(queue: queue)
        _ = looked.wait(timeout: .now() + WEBRTC_DIRECT_PATH_WAIT)
        monitor.cancel()
        return queue.sync { name }
    }

    /// Closes `listener` when the Wi-Fi interface goes or loses the bound
    /// address, and when the application leaves the foreground. linkQueue
    /// only.
    private func watchWebrtcDirect(_ listener: WebrtcDirectListener, interface: String, ip: String) {
        let path = NWPathMonitor(requiredInterfaceType: .wifi)
        path.pathUpdateHandler = { [weak self, weak listener] update in
            guard let self = self, let listener = listener, self.webrtc === listener else { return }
            let held = update.availableInterfaces.contains { $0.type == .wifi && $0.name == interface }
                && WebrtcDirectListener.ipv4Addresses(ofInterface: interface).contains(ip)
            guard !held else { return }
            self.emitDiagnostic("info", "WebRTC Direct listener closed: the Wi-Fi network went", context: [:])
            self.closeWebrtcDirectOnLinkQueue(reason: WebrtcDirectListener.closedNetwork)
        }
        path.start(queue: linkQueue)
        webrtcPath = path
        webrtcBackground = NotificationCenter.default.addObserver(
            forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: nil
        ) { [weak self, weak listener] _ in
            self?.linkQueue.async { [weak self, weak listener] in
                guard let self = self, let listener = listener, self.webrtc === listener else { return }
                self.emitDiagnostic("info", "WebRTC Direct listener closed: the application left the foreground", context: [:])
                self.closeWebrtcDirectOnLinkQueue(reason: WebrtcDirectListener.closedBackground)
            }
        }
    }

    /// Ends a dialled row: out of the table, reported lost if and only if it
    /// still held its address (one refused or superseded is already out),
    /// and the listener told unless it reported the end. linkQueue only.
    private func endDialled(_ row: DialledStream) {
        guard dialled[row.id] === row else { return }
        dialled.removeValue(forKey: row.id)
        streams.remove(row)
        peers.ended(row)
        row.abort()
    }

    // MARK: WebrtcDirectSink (linkQueue, inside the listener's pump)

    /// A page's preamble verified on the listener's `stream`: the caps, then
    /// the one-stream-per-address rule, then the announcement, as for a TCP
    /// stream (`PeerStreamSession.dialProved`). A refused stream is closed
    /// through the listener, nothing is announced, and neither this table nor
    /// the session keeps it, so its later `Closed` finds nothing to end.
    func webrtcDirectProved(
        _ listener: WebrtcDirectListener, stream id: UInt64, address: String, remoteIp: String
    ) {
        let row = DialledStream(id: id, remoteIp: remoteIp, listener: listener)
        let refusal: String?
        if webrtc !== listener || state != .running {
            refusal = "not running"
        } else if address.isEmpty {
            refusal = "no address"
        } else {
            refusal = nil
        }
        if let refusal = refusal {
            emitDiagnostic("warning", "Dialled stream refused", context: [
                "stream": id,
                "reason": refusal,
            ])
            row.abort()
            return
        }
        let held = peers.dialProved(row, address: address) { replacing in
            PeerStreamDialPolicy.admitsDialled(
                from: remoteIp,
                inboundFrom: streams.filter { !$0.outbound }.map { $0.remoteHost },
                open: streams.count,
                replacing: replacing?.remoteHost)
        }
        guard held else { return }
        streams.insert(row)
        dialled[id] = row
    }

    func webrtcDirectBody(_ listener: WebrtcDirectListener, stream id: UInt64, data: Data) {
        guard let row = dialled[id] else { return }
        peers.dialReceived(data, from: row)
    }

    func webrtcDirectClosed(_ listener: WebrtcDirectListener, stream id: UInt64) {
        guard let row = dialled[id] else { return }
        row.endedByListener()
        endDialled(row)
    }
}

// MARK: - Fast link (ADR 0038)

/// A network this device joins for one peer, from an offer that arrived
/// sealed in their session. linkQueue only.
private final class FastLinkJoin {
    let linkId: String
    let peer: String
    let ssid: String
    let host: String
    let port: UInt16
    /// When the engine gives up on it; no dial after this.
    let deadline: TimeInterval
    /// iOS reported the join.
    var joined = false
    /// Looks in a row, one per redial, that found the phone off the group.
    var offGroup = 0
    /// The stream dialed toward the host for this link.
    weak var stream: AnyObject?

    init(linkId: String, peer: String, ssid: String, host: String, port: UInt16, deadline: TimeInterval) {
        self.linkId = linkId
        self.peer = peer
        self.ssid = ssid
        self.host = host
        self.port = port
        self.deadline = deadline
    }
}

/// An iPhone joins a fast link (ADR 0038) and never hosts one: iOS cannot
/// open an access point from code. The network is joined with
/// `NEHotspotConfiguration` and `joinOnce`, which shows the user a prompt
/// every time and leaves the network when the app leaves the foreground; the
/// stream is dialed to the address and port the offer named, claiming the
/// host, so a preamble proving anyone else closes it. The passphrase is
/// never logged.
extension WifiDirectManager {
    /// Gives a dial this long to become ready before it is tried again.
    private static let FAST_LINK_REDIAL: TimeInterval = 2.0
    /// Looks in a row, one per redial, in which the Wi-Fi path's gateway is
    /// not the offered host, before the join is given up as `failed`. `apply`
    /// can finish with no error while the phone stays on its own Wi-Fi (a
    /// wrong passphrase, a group already gone), and the link would otherwise
    /// wait out its offer, two minutes, with nothing said and the host's
    /// group refusing every other link. Twenty seconds: on an iPhone 16 the
    /// group's address arrived about 3 s after the join.
    private static let FAST_LINK_OFF_GROUP_LIMIT = 10
    /// Looks before the prompt, 2 s apart, that find the offered host already
    /// the gateway before the offer is refused `invalid`. Eight seconds: a
    /// phone leaving a fast link's group was back on its own Wi-Fi in about 4.
    private static let FAST_LINK_PRE_JOIN_LOOKS = 4

    /// Whether `error` is the hotspot error `code`. The domain is checked:
    /// another framework's error that shares the number would otherwise read
    /// as the user's refusal, starting the peer's decline pause, or as an
    /// offer that named the network the phone is already on.
    private static func isHotspotError(
        _ error: NSError?, _ code: NEHotspotConfigurationError
    ) -> Bool {
        guard let error = error else { return false }
        return error.domain == NEHotspotConfigurationErrorDomain
            && error.code == code.rawValue
    }

    /// Runs what the engine asked for. linkQueue only.
    fileprivate func drainFastLinkDirectives() {
        guard state == .running else { return }
        while let directive = protocolInstance.wifiDirectNextFastLinkDirective() {
            // From the background a join would prompt a user who is not
            // looking, and iOS would drop the network 15 s later anyway.
            if isPaused, directive.kind != "close" {
                protocolInstance.wifiDirectFastLinkEnded(linkId: directive.linkId, reason: "background")
                continue
            }
            switch directive.kind {
            case "join":
                joinFastLink(directive)
            case "close":
                if fastLink?.linkId == directive.linkId { endFastLink(reason: nil) }
            default:
                // "host": this device declared it cannot.
                protocolInstance.wifiDirectFastLinkEnded(linkId: directive.linkId, reason: "host_failed")
            }
        }
    }

    private func joinFastLink(_ directive: FastLinkDirective) {
        guard fastLink == nil,
              let ssid = directive.ssid,
              let passphrase = directive.passphrase,
              let host = directive.host,
              let port = directive.port else {
            protocolInstance.wifiDirectFastLinkEnded(linkId: directive.linkId, reason: "failed")
            return
        }
        let link = FastLinkJoin(
            linkId: directive.linkId,
            peer: directive.peerId,
            ssid: ssid,
            host: host,
            port: port,
            deadline: ProcessInfo.processInfo.systemUptime + TimeInterval(directive.expiresInMs) / 1000
        )
        // A stream already reaches the peer: it carries what the link would,
        // and the prompt would take the phone off the network that stream
        // runs on.
        if let held = peers.handle(for: link.peer), streams.contains(held) {
            protocolInstance.wifiDirectFastLinkEnded(linkId: link.linkId, reason: "superseded")
            return
        }
        fastLink = link
        checkBeforeJoin(link, passphrase: passphrase, looks: 0)
    }

    /// Before the prompt, the offered host must not already be the gateway of
    /// the network the phone is on. The dial goes out once the Wi-Fi gateway
    /// is the host (`dialFastLink`), and `apply` can finish with no error
    /// while the phone stays on its own Wi-Fi, so an offer naming the user's
    /// own router would otherwise be dialled there, again on every redial
    /// until the offer expired. A match is looked at again before it is
    /// refused: right after a link ends the phone is still leaving that
    /// group, whose owner has the same address as every Android group's.
    private func checkBeforeJoin(_ link: FastLinkJoin, passphrase: String, looks: Int) {
        lookAtWifiPath { [weak self] path in
            guard let self = self, self.fastLink === link else { return }
            if !Self.isGateway(link.host, of: path) {
                self.applyFastLinkJoin(link, passphrase: passphrase)
            } else if looks + 1 < Self.FAST_LINK_PRE_JOIN_LOOKS {
                self.linkQueue.asyncAfter(deadline: .now() + Self.FAST_LINK_REDIAL) { [weak self] in
                    self?.checkBeforeJoin(link, passphrase: passphrase, looks: looks + 1)
                }
            } else {
                self.emitDiagnostic("warning", "Fast link: the offered host is the gateway of the network the phone is already on", context: [
                    "linkId": link.linkId,
                ])
                self.fastLink = nil
                self.protocolInstance.wifiDirectFastLinkEnded(linkId: link.linkId, reason: "invalid")
            }
        }
    }

    private func applyFastLinkJoin(_ link: FastLinkJoin, passphrase: String) {
        emitDiagnostic("info", "Fast link: joining a network", context: ["linkId": link.linkId])
        let configuration = NEHotspotConfiguration(ssid: link.ssid, passphrase: passphrase, isWEP: false)
        configuration.joinOnce = true
        NEHotspotConfigurationManager.shared.apply(configuration) { [weak self] error in
            self?.linkQueue.async { self?.fastLinkJoinFinished(link, error: error as NSError?) }
        }
    }

    /// One look at the Wi-Fi path, on linkQueue. The handler holds the
    /// monitor, so it is dropped on the first update or each look would keep
    /// one alive.
    private func lookAtWifiPath(_ body: @escaping (NWPath) -> Void) {
        let monitor = NWPathMonitor(requiredInterfaceType: .wifi)
        monitor.pathUpdateHandler = { path in
            monitor.pathUpdateHandler = nil
            monitor.cancel()
            body(path)
        }
        monitor.start(queue: linkQueue)
    }

    private func fastLinkJoinFinished(_ link: FastLinkJoin, error: NSError?) {
        guard fastLink === link else {
            // Closed while the user read the prompt: leave what was joined.
            NEHotspotConfigurationManager.shared.removeConfiguration(forSSID: link.ssid)
            // A refusal still counts against the peer, which may have closed
            // its link on purpose to keep the user's Cancel from registering.
            if Self.isHotspotError(error, .userDenied) {
                protocolInstance.wifiDirectFastLinkEnded(linkId: link.linkId, reason: "declined")
            }
            return
        }
        if Self.isHotspotError(error, .alreadyAssociated) {
            // A fast link's network is new and its name random, so a phone
            // already on it was never on a fast link: the peer named the
            // network the phone was on, and a dial would go to the offer's
            // address on the user's own network. Nothing was configured, so
            // nothing is removed; the user's network stays as it was.
            emitDiagnostic("warning", "Fast link: the offer named the network the phone is already on", context: [
                "linkId": link.linkId,
            ])
            fastLink = nil
            protocolInstance.wifiDirectFastLinkEnded(linkId: link.linkId, reason: "invalid")
            return
        }
        if let error = error {
            let reason = Self.isHotspotError(error, .userDenied) ? "declined" : "failed"
            emitDiagnostic("warning", "Fast link: the join did not happen", context: [
                "linkId": link.linkId,
                "reason": reason,
                "code": error.code,
            ])
            fastLink = nil
            NEHotspotConfigurationManager.shared.removeConfiguration(forSSID: link.ssid)
            protocolInstance.wifiDirectFastLinkEnded(linkId: link.linkId, reason: reason)
            return
        }
        link.joined = true
        dialFastLink(link)
    }

    /// Dials the host's listener, bound to Wi-Fi, claiming the host. There is
    /// no discovery record on the network: the offer named the address. A
    /// dial that ends without proving the host is tried again until the
    /// engine's deadline, since the first one often races the address
    /// arriving over DHCP.
    private func dialFastLink(_ link: FastLinkJoin) {
        guard fastLink === link, link.joined, state == .running,
              ProcessInfo.processInfo.systemUptime < link.deadline else { return }
        if let held = peers.handle(for: link.peer), streams.contains(held) { return }
        if let current = link.stream as? Stream, streams.contains(current) {
            // Still connecting (waiting on the path, or mid-preamble).
            scheduleFastLinkRedial(link)
            return
        }
        guard let port = NWEndpoint.Port(rawValue: link.port) else { return }
        // A full stream table is a "not yet" like any other: returning ended
        // the redial chain, and the join sat out its offer with the host's
        // group on the air. The deadline above still bounds it.
        guard streams.count < Self.MAX_STREAMS else {
            scheduleFastLinkRedial(link)
            return
        }
        // Dialed only from the offered network. `apply` can finish with no
        // error while the phone stays on its own Wi-Fi (a wrong passphrase,
        // a group already gone), and `.wifi` alone would then send the dial,
        // and the preamble, to whatever the offer named on the user's home
        // network. The host is the group's owner, so the phone is on the
        // group exactly when its Wi-Fi path's gateway is the offered host;
        // `checkBeforeJoin` refused a host that was already the gateway of
        // the network the phone was on. Not `NEHotspotNetwork.fetchCurrent`: a `joinOnce` network is not
        // one the app configured as iOS counts it, so without location
        // permission that names no network at all, and every join failed.
        // Until the phone is on it, the dial waits, for at most
        // FAST_LINK_OFF_GROUP_LIMIT looks.
        lookAtWifiPath { [weak self] path in
            guard let self = self, self.fastLink === link else { return }
            if Self.isGateway(link.host, of: path) {
                link.offGroup = 0
                self.openFastLinkStream(link, port: port)
            } else if link.offGroup + 1 >= Self.FAST_LINK_OFF_GROUP_LIMIT {
                self.emitDiagnostic("error", "Fast link: the phone is not on the offered network", context: [
                    "linkId": link.linkId,
                ])
                self.endFastLink(reason: "failed")
            } else {
                link.offGroup += 1
                self.scheduleFastLinkRedial(link)
            }
        }
    }

    /// Whether `held` positively runs over a network other than the
    /// group: its path has a gateway, and none is the offered host. That is
    /// a phone the join left on its own Wi-Fi. A path with no gateway yet is
    /// a link-local one on the group before DHCP, and counts as on it. A
    /// stream a page dialled to the WebRTC Direct listener is never the
    /// link's: the listener is bound to the network the phone was on.
    private static func runsOffTheGroup(_ held: Held, host: String) -> Bool {
        guard let stream = held as? Stream else { return true }
        guard let path = stream.connection.currentPath, !path.gateways.isEmpty else {
            return false
        }
        return !isGateway(host, of: path)
    }

    /// Whether `host`, an IP literal, is a gateway of `path`.
    private static func isGateway(_ host: String, of path: NWPath) -> Bool {
        let v4 = IPv4Address(host)
        let v6 = IPv6Address(host)
        return path.gateways.contains { gateway in
            switch gateway {
            case let .hostPort(host: .ipv4(address), port: _):
                return address.rawValue == v4?.rawValue
            case let .hostPort(host: .ipv6(address), port: _):
                return address.rawValue == v6?.rawValue
            default:
                return false
            }
        }
    }

    /// Opens the link's stream, bound to Wi-Fi, claiming the host. linkQueue
    /// only, and only once the phone is on the link's network.
    private func openFastLinkStream(_ link: FastLinkJoin, port: NWEndpoint.Port) {
        guard link.joined, state == .running,
              ProcessInfo.processInfo.systemUptime < link.deadline else { return }
        guard streams.count < Self.MAX_STREAMS else {
            scheduleFastLinkRedial(link)
            return
        }
        if let held = peers.handle(for: link.peer), streams.contains(held) { return }
        if let current = link.stream as? Stream, streams.contains(current) {
            scheduleFastLinkRedial(link)
            return
        }
        let parameters = Self.makeParameters()
        parameters.requiredInterfaceType = .wifi
        let stream = Stream(
            connection: NWConnection(
                to: .hostPort(host: NWEndpoint.Host(link.host), port: port),
                using: parameters
            ),
            outbound: true,
            dialed: nil
        )
        peers.claim(link.peer, for: stream)
        link.stream = stream
        open(stream)
        scheduleFastLinkRedial(link)
    }

    private func scheduleFastLinkRedial(_ link: FastLinkJoin) {
        linkQueue.asyncAfter(deadline: .now() + Self.FAST_LINK_REDIAL) { [weak self] in
            self?.dialFastLink(link)
        }
    }

    /// Leaves the link's network and ends its stream. With a reason, the
    /// engine is told (the platform's own end); without one, the engine
    /// asked. linkQueue only.
    fileprivate func endFastLink(reason: String?) {
        guard let link = fastLink else { return }
        fastLink = nil
        (link.stream as? Stream)?.connection.cancel()
        NEHotspotConfigurationManager.shared.removeConfiguration(forSSID: link.ssid)
        if let reason = reason {
            protocolInstance.wifiDirectFastLinkEnded(linkId: link.linkId, reason: reason)
        }
    }
}

/// `wifiDirect.fastLink`: how far this application takes part in fast links
/// (ADR 0038). `.any` joins an offer it did not ask for; `.requestedOnly`
/// takes part only in links it asked for and answers the peer's initiative
/// `declined`; `.off` declares no role.
public enum FastLinkMode: Equatable {
    case off, requestedOnly, any

    /// The mode a config value names, or `current` when it names none: an
    /// enable that does not mention `fastLink` keeps the setting. An unknown
    /// value is `.off`, so a typo never widens what the application allowed.
    static func parse(_ value: Any?, current: FastLinkMode) -> FastLinkMode {
        // An explicit JavaScript null keeps the setting too, as on Android.
        guard let value = value, !(value is NSNull) else { return current }
        switch value as? String {
        case "any": return .any
        case "requestedOnly": return .requestedOnly
        default: return .off
        }
    }
}
