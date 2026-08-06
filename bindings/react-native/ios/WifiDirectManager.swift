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
public class WifiDirectManager: NSObject, TransportManager {
    
    // MARK: - TransportManager Protocol
    
    public let transportId = "wifi_direct"
    public let transportName = "WiFi Direct (MultipeerConnectivity)"
    public private(set) var state: TransportState = .unavailable
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
    private var session: MCSession?
    private var advertiser: MCNearbyServiceAdvertiser?
    private var browser: MCNearbyServiceBrowser?
    
    // Message sending (event-driven, no polling)
    private let messageQueue = DispatchQueue(label: "com.offlineprotocol.wifidirect.messages")
    
    // State tracking
    private var isAdvertising = false
    private var isBrowsing = false
    private var connectedPeers: [MCPeerID: String] = [:] // MCPeerID -> deviceId
    private var transportStartAt: Date?
    
    // Metrics
    private var bytesSent: UInt64 = 0
    private var bytesReceived: UInt64 = 0
    private var messagesSent: UInt64 = 0
    private var messagesReceived: UInt64 = 0
    
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
        isAdvertising = true
        
        // Start browsing
        browser = MCNearbyServiceBrowser(peer: peerId, serviceType: SERVICE_TYPE)
        browser?.delegate = self
        browser?.startBrowsingForPeers()
        isBrowsing = true
        
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
        isBrowsing = false
        
        // Stop advertising
        advertiser?.stopAdvertisingPeer()
        advertiser = nil
        isAdvertising = false
        
        // Disconnect session
        session?.disconnect()
        session = nil
        connectedPeers.removeAll()
        
        // Notify protocol
        try? protocolInstance.wifiDirectStatusChanged(isConnected: false)
        
        updateState(.stopped)
        emitDiagnostic("info", "WiFi Direct transport stopped")
    }
    
    public func pause() {
        browser?.stopBrowsingForPeers()
        isBrowsing = false
    }
    
    public func resume() {
        if state == .running {
            browser?.startBrowsingForPeers()
            isBrowsing = true
            // Drain any messages that accumulated while paused
            drainAndSendMessages()
        }
    }
    
    // MARK: - Message Handling (Event-Driven)
    
    /// Called by the Rust transport callback when new outgoing messages are available.
    /// Replaces timer-based `startMessagePolling`.
    public func onMessagesAvailable() {
        DispatchQueue.main.async { [weak self] in
            self?.drainAndSendMessages()
        }
    }
    
    /// Drains the Rust message queue and sends each message over MultipeerConnectivity.
    private func drainAndSendMessages() {
        guard state == .running, !connectedPeers.isEmpty else { return }
        
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            
            while let message = self.protocolInstance.wifiDirectGetNextMessage() {
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
        let targetPeers = connectedPeers.filter { $0.value == recipientId }.map { $0.key }
        
        if targetPeers.isEmpty {
            // Send to all connected peers (broadcast)
            let allPeers = session.connectedPeers
            if allPeers.isEmpty {
                emitDiagnostic("warning", "No connected peers to send to")
                return
            }
            
            do {
                try session.send(data, toPeers: allPeers, with: .reliable)
                bytesSent += UInt64(data.count)
                messagesSent += 1
                
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
                bytesSent += UInt64(data.count)
                messagesSent += 1
                
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
        state = newState
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
                self.connectedPeers[peerID] = peerId
                
                self.emitDiagnostic("info", "Peer connected", context: [
                    "peerId": peerId
                ])
                
                // Notify protocol
                try? self.protocolInstance.wifiDirectPeerConnected(peerId: peerId)
                
            case .notConnected:
                if let peerId = self.connectedPeers[peerID] {
                    self.connectedPeers.removeValue(forKey: peerID)
                    
                    self.emitDiagnostic("info", "Peer disconnected", context: [
                        "peerId": peerId
                    ])
                    
                    // Notify protocol
                    try? self.protocolInstance.wifiDirectPeerDisconnected(peerId: peerId)
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
        bytesReceived += UInt64(data.count)
        messagesReceived += 1
        
        let senderId = connectedPeers[peerID] ?? peerID.displayName
        
        messageQueue.async { [weak self] in
            guard let self = self else { return }
            
            do {
                let bytes = [UInt8](data)
                try self.protocolInstance.wifiDirectMessageReceived(senderId: senderId, data: bytes)
                
                self.emitDiagnostic("debug", "Message received from peer", context: [
                    "senderId": senderId,
                    "dataSize": data.count
                ])
            } catch {
                self.emitDiagnostic("error", "Error processing received message", context: [
                    "error": error.localizedDescription
                ])
            }
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
        
        // Enforce connection budget: MCSession limit is 8; stay at 7 to avoid daemon overload.
        let currentCount = session?.connectedPeers.count ?? connectedPeers.count
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
        isAdvertising = false
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
        
        // Don't invite if already connected
        guard session?.connectedPeers.contains(peerID) != true else { return }
        
        // Enforce connection budget: avoid MCSession overflow and connection storms.
        let currentCount = session?.connectedPeers.count ?? connectedPeers.count
        guard !Self.atConnectionBudgetLimit(connectedCount: currentCount) else { return }
        
        browser.invitePeer(peerID, to: session!, withContext: nil, timeout: CONNECTION_TIMEOUT)
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
        isBrowsing = false
    }
}

extension WifiDirectManager: @unchecked Sendable {}

