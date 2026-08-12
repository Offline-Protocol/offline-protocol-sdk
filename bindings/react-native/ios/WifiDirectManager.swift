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
/// ## wifiDirectPeerIdIsUnavailable — why nothing here reaches the protocol layer
///
/// `MCPeerID.displayName` carries the remote's `deviceId`, which is its
/// app-chosen `profile`: a local storage selector, commonly a shared constant
/// like "default", with no key behind it. Announcing it as a protocol id is
/// the same unauthenticated-advertisement problem the BLE cross-check closes,
/// except MultipeerConnectivity offers no signed identity to check it against
/// — nothing here can prove a name.
///
/// So peers are not announced and inbound frames are not ingested. Nothing is
/// lost by that: `WifiDirectTransport` is never registered with the transport
/// manager (see `OfflineProtocol::new` and `rebuild_transports_for_identity`
/// in the UniFFI crate), so frames were already dropped and no send could ever
/// leave. What the announcements *did* do was enter an unprovable id into the
/// core's capacity-bounded `known_peers` — evicting genuine neighbours — and
/// start an auto key exchange toward it.
///
/// Restoring the transport means exchanging and verifying the same signed
/// identity blob BLE serves, and registering `WifiDirectTransport`. Both are
/// out of scope here. Mirrors android's WifiDirectManager.kt.
public class WifiDirectManager: NSObject, TransportManager {
    
    // MARK: - TransportManager Protocol
    
    public let transportId = "wifi_direct"
    public let transportName = "WiFi Direct (MultipeerConnectivity)"
    /// Read through [stateLock] like the session and peer map below, because
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

    private let SERVICE_TYPE = "offline-proto"
    private let DISCOVERY_TIMEOUT: TimeInterval = 30.0
    private let CONNECTION_TIMEOUT: TimeInterval = 30.0
    
    // MARK: - Properties
    
    private let protocolInstance: OfflineProtocol
    private let deviceId: String
    private let peerId: MCPeerID
    
    // MultipeerConnectivity components
    private var advertiser: MCNearbyServiceAdvertiser?
    private var browser: MCNearbyServiceBrowser?

    // Message sending (event-driven, no polling)
    private let messageQueue = DispatchQueue(label: "com.offlineprotocol.wifidirect.messages")

    /// Guards [_state], [_session] and [_connectedPeers], which three different
    /// threads touch: the lifecycle writes them from the bridge queue, the
    /// MCSession / browser / advertiser delegates from MultipeerConnectivity's
    /// own queues, and the send path reads them from [messageQueue]. They were
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
    private var _session: MCSession?
    private var _connectedPeers: [MCPeerID: String] = [:] // MCPeerID -> deviceId

    private func setState(_ newState: TransportState) {
        stateLock.lock(); defer { stateLock.unlock() }
        _state = newState
    }

    private var session: MCSession? {
        get { stateLock.lock(); defer { stateLock.unlock() }; return _session }
        set { stateLock.lock(); defer { stateLock.unlock() }; _session = newValue }
    }

    private var hasConnectedPeers: Bool {
        stateLock.lock(); defer { stateLock.unlock() }
        return !_connectedPeers.isEmpty
    }

    private func deviceId(forPeer peerID: MCPeerID) -> String? {
        stateLock.lock(); defer { stateLock.unlock() }
        return _connectedPeers[peerID]
    }

    private func peers(matching recipientId: String) -> [MCPeerID] {
        stateLock.lock(); defer { stateLock.unlock() }
        return _connectedPeers.filter { $0.value == recipientId }.map { $0.key }
    }

    private func setPeer(_ peerID: MCPeerID, deviceId: String) {
        stateLock.lock(); defer { stateLock.unlock() }
        _connectedPeers[peerID] = deviceId
    }

    /// Removes [peerID] and returns the device id it was bound to, so the
    /// caller can branch on "was it there" without a second lock acquisition
    /// that another thread could interleave.
    @discardableResult
    private func removePeer(_ peerID: MCPeerID) -> String? {
        stateLock.lock(); defer { stateLock.unlock() }
        return _connectedPeers.removeValue(forKey: peerID)
    }

    private func removeAllPeers() {
        stateLock.lock(); defer { stateLock.unlock() }
        _connectedPeers.removeAll()
    }

    private var transportStartAt: Date?

    // MARK: - Initialization
    
    public init(protocol protocolInstance: OfflineProtocol, deviceId: String) {
        self.protocolInstance = protocolInstance
        self.deviceId = deviceId
        self.peerId = MCPeerID(displayName: deviceId)
        super.init()
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
        
        updateState(.starting)
        transportStartAt = Date()
        
        // Create session
        session = MCSession(
            peer: peerId,
            securityIdentity: nil,
            encryptionPreference: .required
        )
        session?.delegate = self
        
        // Start advertising
        advertiser = MCNearbyServiceAdvertiser(
            peer: peerId,
            discoveryInfo: ["deviceId": deviceId],
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

        // Disconnect session
        session?.disconnect()
        session = nil
        removeAllPeers()
        
        // Notify protocol
        try? protocolInstance.wifiDirectStatusChanged(isConnected: false)
        
        updateState(.stopped)
        emitDiagnostic("info", "WiFi Direct transport stopped")
    }
    
    public func pause() {
        browser?.stopBrowsingForPeers()
    }

    public func resume() {
        if state == .running {
            browser?.startBrowsingForPeers()
            // Drain any messages that accumulated while paused
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
        guard state == .running, hasConnectedPeers else { return }

        messageQueue.async { [weak self] in
            guard let self = self else { return }

            while self.state == .running,
                  let message = self.protocolInstance.wifiDirectGetNextMessage() {
                self.sendMessage(recipientId: message.recipientId, data: Data(message.data))
            }
        }
    }
    
    private func sendMessage(recipientId: String, data: Data) {
        guard let session = session else {
            emitDiagnostic("warning", "Cannot send message - no session")
            return
        }
        
        // Find the peer with matching device ID
        let targetPeers = peers(matching: recipientId)
        
        if targetPeers.isEmpty {
            // Send to all connected peers (broadcast)
            let allPeers = session.connectedPeers
            if allPeers.isEmpty {
                emitDiagnostic("warning", "No connected peers to send to")
                return
            }
            
            do {
                try session.send(data, toPeers: allPeers, with: .reliable)
                emitDiagnostic("debug", "Message broadcast to all peers", context: [
                    "peerCount": allPeers.count,
                    "dataSize": data.count
                ])
            } catch {
                emitDiagnostic("error", "Failed to send message", context: [
                    "error": error.localizedDescription
                ])
            }
        } else {
            // Send to specific peer
            do {
                try session.send(data, toPeers: targetPeers, with: .reliable)
                emitDiagnostic("debug", "Message sent to peer", context: [
                    "recipientId": recipientId,
                    "dataSize": data.count
                ])
            } catch {
                emitDiagnostic("error", "Failed to send message", context: [
                    "recipientId": recipientId,
                    "error": error.localizedDescription
                ])
            }
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
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            
            switch state {
            case .connected:
                let peerId = peerID.displayName
                self.setPeer(peerID, deviceId: peerId)

                // NOT announced to the protocol layer — see
                // `wifiDirectPeerIdIsUnavailable` on the type. `displayName`
                // is the remote's app-chosen profile, which nothing binds to a
                // key and which is commonly a shared constant.
                self.emitDiagnostic("warning", "Wi-Fi Direct peer not announced: unproven id", context: [
                    "displayName": peerId
                ])

            case .notConnected:
                if let peerId = self.removePeer(peerID) {
                    // No disconnect notification: nothing was announced, so
                    // there is no neighbor for the core to lose.
                    self.emitDiagnostic("info", "Wi-Fi Direct peer disconnected", context: [
                        "displayName": peerId
                    ])
                }

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
        let senderId = deviceId(forPeer: peerID) ?? peerID.displayName

        // Dropped, not ingested — see `wifiDirectPeerIdIsUnavailable`.
        // Attributing the frame to an unproven `displayName` would set it as
        // the transport peer identity, which the core then compares against
        // `Message.sender` and rejects.
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            self.emitDiagnostic("warning", "Wi-Fi Direct frame dropped: sender cannot be identified", context: [
                "displayName": senderId,
                "dataSize": data.count
            ])
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
        
        // Don't invite ourselves
        guard peerID != peerId else { return }

        // One snapshot for the whole decision. This read used to happen three
        // separate times and end in `session!`, so a stop() landing between
        // the guard and the invite crashed on the force-unwrap — the exact
        // race the accessors above exist to close, at the one call site that
        // still reached around them. A nil session means the transport is
        // down, which is nothing to invite anyone to.
        guard let session = session else { return }

        // Don't invite if already connected
        guard !session.connectedPeers.contains(peerID) else { return }

        // Enforce connection budget: avoid MCSession overflow and connection storms.
        guard !Self.atConnectionBudgetLimit(connectedCount: session.connectedPeers.count) else {
            return
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

extension WifiDirectManager: @unchecked Sendable {}

