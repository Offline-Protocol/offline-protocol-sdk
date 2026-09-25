//
// WifiDirectManager.swift
// OfflineProtocol
//
// WiFi Direct transport implementation using MultipeerConnectivity
// Note: iOS uses MultipeerConnectivity framework which provides similar
// functionality to Android's WiFi Direct (Wi-Fi P2P)
//

import Foundation
import MultipeerConnectivity

/// WiFi Direct Manager implementing TransportManager for peer-to-peer WiFi communication
/// Uses Apple's MultipeerConnectivity framework which provides WiFi Direct-like functionality
///
/// ## Every peer proves its address before it carries anything
///
/// The engine registers this transport as its peer-stream slot, and
/// `docs/spec/stream-framing.md` is the contract. When a Multipeer peer
/// connects, each side sends its identity assertion as its first message and
/// verifies the other's with `verifyIdentityAssertion`, the one verifier every
/// platform uses. The address it derives is the only id this manager hands the
/// core: to `wifiDirectPeerConnected`, as the `senderId` of each body, and to
/// `wifiDirectPeerDisconnected`. `MCPeerID.displayName` never reaches the core.
/// It carries the remote's app-chosen profile, which nothing binds to a key and
/// which is commonly a shared constant, and announcing it once put unprovable
/// ids into the core's capacity-bounded `known_peers`.
///
/// What the chapter asks of a receiver lives in `PeerStreamSession`, which
/// this manager forwards each per-peer event to: every message is one frame
/// whose `u32` big-endian prefix equals the rest of the message (the session
/// already delivers whole messages), the preamble must verify within
/// `PREAMBLE_TIMEOUT`, and one peer is announced per address, the newer
/// superseding the older without a loss report. It is Foundation-only and
/// unit-tested with a fake session and clock. What stays here:
/// - The service type is `offlineprotocol`, which the framework publishes as
///   `_offlineprotocol._tcp` and `_offlineprotocol._udp`; an app's
///   `NSBonjourServices` must list both, or local-network privacy blocks
///   discovery with no error. The discovery info carries `txtvers` and `addr`,
///   and `addr` is a claim the preamble must prove for a peer we invite.
///
/// Every per-peer transition (connect, each received message, disconnect) runs
/// on `linkQueue`, one serial queue, so a peer's first message cannot overtake
/// its connect callback and no body reaches the core after its loss report.
/// The core re-adds a neighbour on any inbound body, so that ordering is what
/// keeps a reported-lost peer lost. Mirrors android's WifiDirectManager.kt.
public class WifiDirectManager: NSObject, TransportManager {
    
    // MARK: - TransportManager Protocol
    
    public let transportId = "wifi_direct"
    public let transportName = "WiFi Direct (MultipeerConnectivity)"
    /// Read through [stateLock] like the session below, because
    /// it is touched by the same three threads: the lifecycle writes it from
    /// the bridge queue, and the send path reads it from whichever thread the
    /// Rust callback arrives on. This is the iOS half of the `@Volatile` the
    /// Android manager's `state` carries, and it is what makes the claim on
    /// [onMessagesAvailable] — that the drain's reads are safe from anywhere —
    /// actually true.
    public var state: TransportState {
        stateLock.lock(); defer { stateLock.unlock() }
        return _state
    }
    public weak var delegate: TransportManagerDelegate?
    
    // MARK: - Constants
    
    /// MCSession has a hard limit of 8 peers. Stay at 7 to avoid overwhelming the daemon
    /// and prevent connection storms in dense environments (e.g. festival, protest).
    private let WIFI_DIRECT_MAX_PEERS_SAFE = 7

    /// Returns true when at or over the connection budget (so we must not invite or accept).
    /// Used in browser and advertiser delegates; exposed for unit tests.
    static func atConnectionBudgetLimit(connectedCount: Int) -> Bool {
        return connectedCount >= 7
    }

    /// `offlineprotocol`: fifteen characters, the Multipeer maximum, and the
    /// service label stream-framing.md requires. It was `offline-proto`, which
    /// no app's `NSBonjourServices` listed under the documented name.
    private let SERVICE_TYPE = "offlineprotocol"
    /// How long a connected peer may take to prove its address. Local policy,
    /// not wire format (stream-framing.md).
    private let PREAMBLE_TIMEOUT: TimeInterval = 10.0
    private let DISCOVERY_TIMEOUT: TimeInterval = 30.0
    private let CONNECTION_TIMEOUT: TimeInterval = 30.0
    
    // MARK: - Properties
    
    private let protocolInstance: OfflineProtocol
    private let deviceId: String
    
    // MultipeerConnectivity components
    private var advertiser: MCNearbyServiceAdvertiser?
    private var browser: MCNearbyServiceBrowser?

    // Message sending (event-driven, no polling)
    private let messageQueue = DispatchQueue(label: "com.offlineprotocol.wifidirect.messages")

    /// Every per-peer transition runs here, in order: the connect, each
    /// received message, the disconnect, and the preamble deadline. See the
    /// type's documentation for why one serial queue is the invariant.
    private let linkQueue = DispatchQueue(label: "com.offlineprotocol.wifidirect.links")
    private static let linkQueueKey = DispatchSpecificKey<Bool>()

    /// Each connected peer from its first message to its disconnect: the
    /// preamble, the framing, one announced peer per address. This manager
    /// owns the session and forwards per-peer events on `linkQueue`; see
    /// `PeerStreamSession` for the rest. Set once in `init`, before any
    /// callback can arrive, and never reassigned.
    private var peers: PeerStreamSession<MCPeerID>!

    /// Guards [_state], [_session] and [_isPaused], which three different
    /// threads touch: the lifecycle writes them from the bridge queue, the
    /// MCSession / browser / advertiser delegates from MultipeerConnectivity's
    /// own queues, and the send path reads them from [messageQueue]. The proved
    /// peers live in `links`, which has its own lock under the same rule. They were
    /// plain properties, which is the same unsynchronised cross-thread access
    /// #338 removed from BleManager's `peripheralRSSI`.
    ///
    /// Held across one field read or write and nothing else — never across a
    /// UniFFI call, an `MCSession.send`, or a delegate callback. Every accessor
    /// below returns a snapshot so callers work on values, not on shared
    /// storage. That is also why it can be a plain [NSLock]: no accessor calls
    /// out, so nothing can re-enter and need recursion.
    private let stateLock = NSLock()
    private var _state: TransportState = .unavailable

    /// True between `pause()` and `resume()`. Mirrors `InternetManager`'s flag
    /// of the same name.
    ///
    /// This manager's `pause()` stopped browsing and nothing else, so the send
    /// path ignored it entirely: `onMessagesAvailable` runs
    /// `drainAndSendMessages`, which gated on `state` — and pause does not
    /// change `state` — so a paused transport drained the whole queue, each
    /// message taking the core's global protocol mutex. Guarded here rather
    /// than only at the callback because `drainAndSendMessages` is also reached
    /// from `resume()`, and because the drain loop re-reads it every iteration.
    private var _isPaused = false
    private var _session: MCSession?

    private func setState(_ newState: TransportState) {
        stateLock.lock(); defer { stateLock.unlock() }
        _state = newState
    }

    private var isPaused: Bool {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _isPaused }
        set { stateLock.lock(); defer { stateLock.unlock() }; _isPaused = newValue }
    }

    private var session: MCSession? {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _session }
        set { stateLock.lock(); defer { stateLock.unlock() }; _session = newValue }
    }

    /// Whether any peer has proved an address. Reads `peers`, not
    /// `stateLock`: the proved peers are the only ones there is anything to
    /// send to.
    private var hasConnectedPeers: Bool {
        return !peers.isEmpty
    }

    private var transportStartAt: Date?

    // MARK: - Initialization
    
    public init(protocol protocolInstance: OfflineProtocol, deviceId: String) {
        self.protocolInstance = protocolInstance
        self.deviceId = deviceId
        super.init()
        linkQueue.setSpecific(key: Self.linkQueueKey, value: true)
        peers = PeerStreamSession<MCPeerID>(
            host: self,
            preambleTimeout: PREAMBLE_TIMEOUT,
            send: { [weak self] frame, peer in
                guard let session = self?.session else { throw TransportError.notRunning }
                try session.send(frame, toPeers: [peer], with: .reliable)
            },
            disconnect: { [weak self] peer in
                self?.session?.cancelConnectPeer(peer)
            },
            schedule: { [weak self] delay, block in
                self?.linkQueue.asyncAfter(deadline: .now() + delay, execute: block)
            }
        )
    }
    
    deinit {
        stop()
    }
    
    // MARK: - TransportManager Implementation
    
    public func isAvailable() -> Bool {
        // MultipeerConnectivity is available on all iOS devices
        return true
    }
    
    public func start() throws {
        guard state != .running else {
            throw TransportError.alreadyRunning
        }
        
        emitDiagnostic("info", "Starting WiFi Direct transport", context: [
            "deviceId": deviceId
        ])

        // An explicit start() means "run": a pause() from a previous session
        // must not leave this fresh transport connected-but-mute. Mirrors
        // `InternetManager.start()`.
        isPaused = false

        updateState(.starting)
        transportStartAt = Date()
        
        // A fresh MCPeerID per start, never one per manager. PeerStreamSession
        // keys each peer's state by its MCPeerID, and a remote that stopped
        // and started again under the same one could have its new connect
        // land before the old session's disconnect: the connect would find a
        // preamble already sent and an address already proved, send nothing,
        // and the remote would refuse us at its deadline. A new id makes every
        // restart a new peer, which the supersede rule already handles.
        let peerId = MCPeerID(displayName: deviceId)

        // Create session
        session = MCSession(
            peer: peerId,
            securityIdentity: nil,
            encryptionPreference: .required
        )
        session?.delegate = self
        
        // Start advertising. `addr` is a hint a browser checks the preamble
        // against, and absent until this device has an identity. The profile
        // (`deviceId`) is no longer advertised: it is not an identity.
        var discoveryInfo = ["txtvers": "1"]
        if let address = protocolInstance.localAddress(), !address.isEmpty {
            discoveryInfo["addr"] = address
        }
        advertiser = MCNearbyServiceAdvertiser(
            peer: peerId,
            discoveryInfo: discoveryInfo,
            serviceType: SERVICE_TYPE
        )
        advertiser?.delegate = self
        advertiser?.startAdvertisingPeer()

        // Start browsing
        browser = MCNearbyServiceBrowser(peer: peerId, serviceType: SERVICE_TYPE)
        browser?.delegate = self
        browser?.startBrowsingForPeers()

        updateState(.running)
        
        // Notify protocol
        try? protocolInstance.wifiDirectStatusChanged(isConnected: true)
        
        emitDiagnostic("info", "WiFi Direct transport started")
    }
    
    public func stop() {
        guard state == .running || state == .starting else {
            return
        }
        
        updateState(.stopping)
        
        // Stop browsing
        browser?.stopBrowsingForPeers()
        browser = nil

        // Stop advertising
        advertiser?.stopAdvertisingPeer()
        advertiser = nil

        // Report every proved peer lost while the core still holds its link,
        // on linkQueue so no delivery lands after a loss report. The session's
        // own .notConnected callbacks then find nothing left to report.
        onLinkQueueSync {
            peers.endAll()
        }

        // Disconnect session
        session?.disconnect()
        session = nil
        
        // Notify protocol
        try? protocolInstance.wifiDirectStatusChanged(isConnected: false)
        
        updateState(.stopped)
        emitDiagnostic("info", "WiFi Direct transport stopped")
    }
    
    public func pause() {
        // Set before browsing stops. This is what actually pauses the send
        // path — see `isPaused`; stopping the browser only stops finding new
        // peers, and this manager has no timer to cancel.
        isPaused = true
        browser?.stopBrowsingForPeers()
    }

    public func resume() {
        isPaused = false
        if state == .running {
            browser?.startBrowsingForPeers()
            // Drain any messages that accumulated while paused. Required
            // rather than tidy: the core does not re-issue
            // `onMessagesAvailable` for messages it already announced, and
            // this manager has no fallback timer to pick them up.
            drainAndSendMessages()
        }
    }
    
    // MARK: - Message Handling (Event-Driven)
    
    /// Called by the Rust transport callback when new outgoing messages are available.
    /// Replaces timer-based `startMessagePolling`.
    ///
    /// Goes straight to the drain rather than hopping through main first, as
    /// the Reticulum and Nostr managers already do. The hop bought nothing:
    /// the only work it did on main was read `state` and the peer map, both of
    /// which are now synchronised and readable from anywhere, and it put a
    /// scheduling dependency on the UI thread into the send path of a
    /// transport that never touches the UI.
    public func onMessagesAvailable() {
        drainAndSendMessages()
    }

    /// Drains the Rust message queue and sends each message over MultipeerConnectivity.
    ///
    /// Unbounded, where the Android manager's mirror of this spends a batch
    /// budget and reposts. The asymmetry is deliberate: the budget there exists
    /// because that looper is shared — it also delivers the Wi-Fi P2P framework
    /// callbacks and the broadcast receiver, and a broadcast that misses its
    /// dispatch budget is an ANR wherever the receiver runs — and because a
    /// `stop()` waits on that same thread without a bound. Neither holds here.
    /// `messageQueue` is this manager's alone, carries no framework callbacks,
    /// and no lifecycle path waits on it, so a long drain delays nothing but
    /// the next drain. Add a budget here only if one of those three facts
    /// changes.
    ///
    /// Unbounded is not the same as unconditional, though, which is the one
    /// thing the Android mirror gets for free and this does not. Re-entering
    /// `drainAndSendMessages` after each batch re-runs its guard; a single
    /// `while` does not, so the state has to be re-read *inside* the loop. The
    /// guard below runs before the hop, and a `stop()` landing after it leaves
    /// every remaining iteration taking the core's global mutex to fetch a
    /// message that `sendMessage` then drops for want of a session — a warning
    /// per message, against a transport that is already down.
    private func drainAndSendMessages() {
        guard !isPaused, state == .running, hasConnectedPeers else { return }

        messageQueue.async { [weak self] in
            guard let self = self else { return }

            // `isPaused` re-read every iteration alongside the state, and for
            // the same reason the state is: this loop can run for as long as
            // the queue is deep, and a `pause()` landing inside it would
            // otherwise keep taking the core's global protocol mutex once per
            // remaining message against a transport the app has backgrounded.
            while !self.isPaused, self.state == .running,
                  let message = self.protocolInstance.wifiDirectGetNextMessage() {
                self.sendMessage(recipientId: message.recipientId, data: Data(message.data))
            }
        }
    }
    
    /// Frames one body and sends it to the peer that proved `recipientId`.
    ///
    /// Never broadcasts. The core returns a body only for an address a peer
    /// proved, so a recipient with no peer here means that peer left between
    /// the core's answer and this lookup; the body is dropped and the core's
    /// acknowledgement and retry cover it. The broadcast fallback this
    /// replaced handed a message for one peer to every peer in the session.
    private func sendMessage(recipientId: String, data: Data) {
        guard let session = session else {
            emitDiagnostic("warning", "Cannot send message - no session")
            return
        }
        guard let peer = peers.handle(for: recipientId) else {
            emitDiagnostic("warning", "Wi-Fi Direct body dropped: no peer holds the recipient", context: [
                "recipientId": recipientId,
                "dataSize": data.count
            ])
            return
        }
        guard let frame = PeerStreamFraming.frame(data) else {
            emitDiagnostic("error", "Wi-Fi Direct body dropped: outside the frame bounds", context: [
                "recipientId": recipientId,
                "dataSize": data.count
            ])
            return
        }
        do {
            try session.send(frame, toPeers: [peer], with: .reliable)
        } catch {
            emitDiagnostic("error", "Failed to send message", context: [
                "recipientId": recipientId,
                "error": error.localizedDescription
            ])
        }
    }

    // MARK: - Peer links (linkQueue only)

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
        // downstream work — including UniFFI calls — inside this manager's
        // critical section.
        delegate?.transportManager(self, didChangeState: newState)
    }
    
    // MARK: - Diagnostics
    
    private func emitDiagnostic(_ level: String, _ message: String, context: [String: Any] = [:]) {
        delegate?.transportManager(self, didEmitDiagnostic: level, message: message, context: context)
    }
}

// MARK: - MCSessionDelegate

extension WifiDirectManager: MCSessionDelegate {
    
    public func session(_ session: MCSession, peer peerID: MCPeerID, didChange state: MCSessionState) {
        linkQueue.async { [weak self] in
            guard let self = self else { return }

            switch state {
            case .connected:
                // A callback from a session this manager has since replaced
                // (a stop() then start()) has nothing to attach to.
                guard self.session === session else { return }
                self.peers.connected(peerID)

            case .notConnected:
                // Scoped to the current session like the other two callbacks.
                // stop() already reported the old session's peers through
                // endAll(), and a late disconnect from it would otherwise end
                // the same MCPeerID's live link in the new session.
                guard self.session === session else { return }
                self.peers.ended(peerID)

            case .connecting:
                self.emitDiagnostic("debug", "Connecting to peer", context: [
                    "peerId": peerID.displayName
                ])

            @unknown default:
                break
            }
        }
    }

    public func session(_ session: MCSession, didReceive data: Data, fromPeer peerID: MCPeerID) {
        linkQueue.async { [weak self] in
            guard let self = self, self.session === session else { return }
            self.peers.received(data, from: peerID)
        }
    }

    public func session(_ session: MCSession, didReceive stream: InputStream, withName streamName: String, fromPeer peerID: MCPeerID) {
        // Not used for our message-based protocol
    }
    
    public func session(_ session: MCSession, didStartReceivingResourceWithName resourceName: String, fromPeer peerID: MCPeerID, with progress: Progress) {
        // Not used for our message-based protocol
    }
    
    public func session(_ session: MCSession, didFinishReceivingResourceWithName resourceName: String, fromPeer peerID: MCPeerID, at localURL: URL?, withError error: Error?) {
        // Not used for our message-based protocol
    }
}

// MARK: - MCNearbyServiceAdvertiserDelegate

extension WifiDirectManager: MCNearbyServiceAdvertiserDelegate {
    
    public func advertiser(_ advertiser: MCNearbyServiceAdvertiser, didReceiveInvitationFromPeer peerID: MCPeerID, withContext context: Data?, invitationHandler: @escaping (Bool, MCSession?) -> Void) {
        emitDiagnostic("info", "Received invitation from peer", context: [
            "peerId": peerID.displayName
        ])
        
        // One snapshot for the whole decision, like `foundPeer` below: reading
        // `session` again at the accept would let a concurrent stop() hand the
        // framework a different value than the budget check saw. A stopped
        // transport has nothing to accept into, so it declines instead of
        // accepting with a nil session.
        guard let session = session else {
            invitationHandler(false, nil)
            return
        }

        // Enforce connection budget: MCSession limit is 8; stay at 7 to avoid daemon overload.
        let currentCount = session.connectedPeers.count
        if Self.atConnectionBudgetLimit(connectedCount: currentCount) {
            emitDiagnostic("info", "Rejecting invitation: at connection budget limit", context: [
                "connectedCount": currentCount,
                "limit": WIFI_DIRECT_MAX_PEERS_SAFE
            ])
            invitationHandler(false, nil)
            return
        }
        invitationHandler(true, session)
    }
    
    public func advertiser(_ advertiser: MCNearbyServiceAdvertiser, didNotStartAdvertisingPeer error: Error) {
        emitDiagnostic("error", "Failed to start advertising", context: [
            "error": error.localizedDescription
        ])
    }
}

// MARK: - MCNearbyServiceBrowserDelegate

extension WifiDirectManager: MCNearbyServiceBrowserDelegate {
    
    public func browser(_ browser: MCNearbyServiceBrowser, foundPeer peerID: MCPeerID, withDiscoveryInfo info: [String : String]?) {
        emitDiagnostic("info", "Found peer", context: [
            "peerId": peerID.displayName,
            "discoveryInfo": info ?? [:]
        ])
        
        // One snapshot for the whole decision. This read used to happen three
        // separate times and end in `session!`, so a stop() landing between
        // the guard and the invite crashed on the force-unwrap — the exact
        // race the accessors above exist to close, at the one call site that
        // still reached around them. A nil session means the transport is
        // down, which is nothing to invite anyone to.
        guard let session = session else { return }

        // Don't invite ourselves. Read off the session: the id is minted per
        // start(), and the session is the one this browser belongs to.
        guard peerID != session.myPeerID else { return }

        // Don't invite if already connected
        guard !session.connectedPeers.contains(peerID) else { return }

        // Enforce connection budget: avoid MCSession overflow and connection storms.
        guard !Self.atConnectionBudgetLimit(connectedCount: session.connectedPeers.count) else {
            return
        }

        // Record the advertised address before inviting, on the queue the
        // connect callback will run on, so the preamble is checked against it.
        let claim = info?["addr"]
        linkQueue.async { [weak self] in
            self?.peers.claim(claim, for: peerID)
        }

        browser.invitePeer(peerID, to: session, withContext: nil, timeout: CONNECTION_TIMEOUT)
    }
    
    public func browser(_ browser: MCNearbyServiceBrowser, lostPeer peerID: MCPeerID) {
        emitDiagnostic("info", "Lost peer", context: [
            "peerId": peerID.displayName
        ])
    }
    
    public func browser(_ browser: MCNearbyServiceBrowser, didNotStartBrowsingForPeers error: Error) {
        emitDiagnostic("error", "Failed to start browsing", context: [
            "error": error.localizedDescription
        ])
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
            emitDiagnostic("error", "Error announcing Wi-Fi Direct peer", context: [
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
            emitDiagnostic("warning", "Wi-Fi Direct body refused by the core", context: [
                "address": address,
                "error": error.localizedDescription
            ])
        }
    }

    func peerStreamLost(_ address: String) {
        do {
            try protocolInstance.wifiDirectPeerDisconnected(peerId: address)
        } catch {
            emitDiagnostic("error", "Error reporting Wi-Fi Direct peer lost", context: [
                "error": error.localizedDescription
            ])
        }
        emitDiagnostic("info", "Wi-Fi Direct peer disconnected", context: ["address": address])
    }

    func peerStreamDiagnostic(_ level: String, _ message: String, _ context: [String: Any]) {
        emitDiagnostic(level, message, context: context)
    }
}

extension WifiDirectManager: @unchecked Sendable {}

