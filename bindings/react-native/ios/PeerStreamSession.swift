//
// PeerStreamSession.swift
// OfflineProtocol
//
// docs/spec/stream-framing.md over a message-oriented carrier: what happens to
// one connected peer from its first message to its disconnect.
//
// WifiDirectManager owns the Multipeer session, the advertiser and the
// browser, and forwards each per-peer event here from its one serial queue.
// Everything between is here, generic over the carrier's peer handle and
// behind `PeerStreamHost`, so that PeerStreamSessionTests drives it with
// string handles and a fake clock, without MultipeerConnectivity or the native
// library. Mirrors android's PeerStreamSockets.kt, keep in sync.
//
// The rules, each of which a test pins:
// - Our preamble goes to a peer as soon as it connects, without waiting for
//   theirs, and theirs must verify within `preambleTimeout`.
// - The host is told only the address the preamble proved: `peerConnected`
//   once per address, each body attributed to it, and `peerDisconnected` once,
//   if and only if the peer still held the address when it ended.
// - One announced peer per address, newer superseding older, through
//   `PeerStreamLinks`. A superseded peer is disconnected and reports nothing.
// - Every message is one frame whose prefix equals the rest of the message;
//   anything else disconnects the peer.
//

import Foundation

/// The core, as far as a peer is concerned.
protocol PeerStreamHost: AnyObject {
    /// This device's identity assertion, sent as our preamble. Throws without an identity.
    func peerStreamIdentityAssertion() throws -> Data
    /// Our own address, so a copy of our preamble played back is refused.
    func peerStreamLocalAddress() -> String?
    /// `verifyIdentityAssertion`: the derived address, or a throw.
    func peerStreamVerify(_ assertion: Data) throws -> String
    func peerStreamConnected(_ address: String)
    func peerStreamReceived(_ address: String, _ body: Data)
    func peerStreamLost(_ address: String)
    func peerStreamDiagnostic(_ level: String, _ message: String, _ context: [String: Any])
}

/// Not thread-safe except where noted: the owner calls every method from one
/// serial queue, and `schedule` must run its block on that same queue. That
/// one queue is what keeps a peer's first message from overtaking its connect
/// and a delivery from landing after a loss report. The core re-adds a
/// neighbour on any inbound body, so the second is what keeps a
/// reported-lost peer lost.
final class PeerStreamSession<Handle: Hashable> {
    private weak var host: PeerStreamHost?
    private let send: (Data, Handle) throws -> Void
    private let disconnect: (Handle) -> Void
    private let schedule: (TimeInterval, @escaping () -> Void) -> Void
    private let preambleTimeout: TimeInterval

    /// The peers that proved an address. Thread-safe on its own lock, and read
    /// by the send path from any thread.
    private let links = PeerStreamLinks<Handle>()

    private final class Link {
        let preamble: PeerStreamPreamble
        var sentPreamble = false
        var deadlineArmed = false
        /// Set once the peer is being disconnected: nothing more from it is
        /// read, and nothing about it is reported again.
        var refused = false

        init(preamble: PeerStreamPreamble) {
            self.preamble = preamble
        }
    }
    private var linkStates: [Handle: Link] = [:]
    private var claims: [Handle: String] = [:]

    /// - Parameters:
    ///   - send: writes one whole frame to one peer.
    ///   - disconnect: ends one peer's connection. Its own disconnect event
    ///     may follow and finds nothing left to report.
    ///   - schedule: runs a block after a delay on the owner's serial queue.
    init(
        host: PeerStreamHost,
        preambleTimeout: TimeInterval = 10.0,
        send: @escaping (Data, Handle) throws -> Void,
        disconnect: @escaping (Handle) -> Void,
        schedule: @escaping (TimeInterval, @escaping () -> Void) -> Void
    ) {
        self.host = host
        self.preambleTimeout = preambleTimeout
        self.send = send
        self.disconnect = disconnect
        self.schedule = schedule
    }

    // MARK: - Thread-safe reads

    /// The peer that proved `address`, for the send path. Any thread.
    func handle(for address: String) -> Handle? {
        links.handle(for: address)
    }

    /// Whether any peer has proved an address. Any thread.
    var isEmpty: Bool {
        links.isEmpty
    }

    // MARK: - Events (the owner's serial queue)

    /// The address a peer advertised before we invited it: a hint its
    /// preamble must prove (step four), never a name to announce.
    func claim(_ address: String?, for handle: Handle) {
        if let address = address, !address.isEmpty {
            claims[handle] = address
        } else {
            claims.removeValue(forKey: handle)
        }
    }

    /// A peer connected: send our preamble and start its deadline.
    func connected(_ handle: Handle) {
        let link = state(for: handle)
        guard !link.refused else { return }

        if !link.sentPreamble {
            // Ours goes first and without waiting for theirs, so neither side
            // can hold the other half-open by staying silent.
            do {
                guard let host = host else { return }
                let assertion = try host.peerStreamIdentityAssertion()
                guard let frame = PeerStreamFraming.frame(assertion) else {
                    refuse(handle, link, reason: "our assertion is outside the frame bounds")
                    return
                }
                try send(frame, handle)
                link.sentPreamble = true
            } catch {
                refuse(handle, link, reason: "no preamble sent: \(error)")
                return
            }
        }

        if link.preamble.awaitingPreamble, !link.deadlineArmed {
            link.deadlineArmed = true
            schedule(preambleTimeout) { [weak self, weak link] in
                guard let self = self, let link = link,
                      !link.refused, link.preamble.awaitingPreamble else { return }
                self.refuse(handle, link, reason: "no preamble before the deadline")
            }
        }
    }

    /// One message from a peer: its preamble if none was accepted yet,
    /// otherwise a body for the host attributed to the address it proved.
    func received(_ message: Data, from handle: Handle) {
        // A message can arrive before the connect event; the state is created
        // here then, and our preamble goes out when the connect arrives.
        let link = state(for: handle)
        guard !link.refused, let host = host else { return }

        let body: Data
        switch PeerStreamFraming.unframe(message, preamble: link.preamble.awaitingPreamble) {
        case .failure(let refusal):
            refuse(handle, link, reason: "frame refused: \(refusal.reason)")
            return
        case .success(let unframed):
            body = unframed
        }

        switch link.preamble.accept(body) {
        case .refuse(let reason):
            refuse(handle, link, reason: reason)
        case .announce(let address):
            let announcement = links.announce(handle, address: address)
            if let older = announcement.superseded {
                // Disconnected without a loss report: `links` already moved
                // the address to this peer, so the older one's end finds
                // nothing to report. Marked refused so its late messages,
                // already queued behind this one, are dropped rather than
                // delivered under an address it no longer holds.
                linkStates[older]?.refused = true
                disconnect(older)
                host.peerStreamDiagnostic("info", "Peer stream superseded", ["address": address])
            }
            if announcement.firstForAddress {
                host.peerStreamConnected(address)
            }
            host.peerStreamDiagnostic("info", "Peer stream proved", ["address": address])
        case .deliver(let address, let payload):
            host.peerStreamReceived(address, payload)
        }
    }

    /// The peer left.
    func ended(_ handle: Handle) {
        if let link = linkStates.removeValue(forKey: handle) {
            link.refused = true
        }
        claims.removeValue(forKey: handle)
        if let address = links.remove(handle) {
            host?.peerStreamLost(address)
        }
    }

    /// Everything ends: each announced peer is reported lost once, before the
    /// owner tells the core the layer is down.
    func endAll() {
        for (_, link) in linkStates {
            link.refused = true
        }
        linkStates.removeAll()
        claims.removeAll()
        for (_, address) in links.removeAll() {
            host?.peerStreamLost(address)
        }
    }

    // MARK: - Private

    private func state(for handle: Handle) -> Link {
        if let link = linkStates[handle] { return link }
        let link = Link(preamble: PeerStreamPreamble(
            verify: { [weak host] body in
                guard let host = host else { throw PeerStreamRefusal(reason: "no host") }
                return try host.peerStreamVerify(body)
            },
            expected: claims[handle],
            localAddress: host?.peerStreamLocalAddress()
        ))
        linkStates[handle] = link
        return link
    }

    /// Disconnects `handle`, reporting it lost now if it had been announced,
    /// so the report does not wait on the carrier's disconnect event.
    private func refuse(_ handle: Handle, _ link: Link, reason: String) {
        link.refused = true
        if let address = links.remove(handle) {
            host?.peerStreamLost(address)
        }
        disconnect(handle)
        host?.peerStreamDiagnostic("warning", "Peer stream refused", ["reason": reason])
    }
}
