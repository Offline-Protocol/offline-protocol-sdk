//
// PeerStreamSession.swift
// OfflineProtocol
//
// docs/spec/stream-framing.md for one connected peer, from its first frame
// to its disconnect.
//
// WifiDirectManager owns the listener, the browser and the connections, cuts
// each stream into frames with `PeerStreamReader`, and forwards each per-peer
// event here from its one serial queue. Everything between is here, generic
// over the carrier's peer handle and behind `PeerStreamHost`, so that
// PeerStreamSessionTests drives it with string handles and a fake clock,
// without Network framework or the native library. Mirrors android's
// PeerStreamSockets.kt, keep in sync.
//
// The rules, each of which a test pins:
// - Our preamble goes to a peer as soon as it connects, without waiting for
//   theirs, and always before the peer is announced; theirs must verify
//   within `preambleTimeout`.
// - The host is told only the address the preamble proved: `peerConnected`
//   once per address, each body attributed to it, and `peerDisconnected` once,
//   if and only if the peer still held the address when it ended.
// - One announced peer per address, through `PeerStreamLinks`: the stream the
//   lower address opened wins, and the newer of two such. A superseded or
//   refused peer is disconnected and reports nothing.
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
    private let isOutbound: (Handle) -> Bool
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
    ///   - isOutbound: whether this device opened the handle's stream, which
    ///     decides which of two streams for one address is kept. A property
    ///     of the handle rather than of an event, so no event order can get
    ///     it wrong.
    init(
        host: PeerStreamHost,
        preambleTimeout: TimeInterval = 10.0,
        send: @escaping (Data, Handle) throws -> Void,
        disconnect: @escaping (Handle) -> Void,
        schedule: @escaping (TimeInterval, @escaping () -> Void) -> Void,
        isOutbound: @escaping (Handle) -> Bool
    ) {
        self.host = host
        self.preambleTimeout = preambleTimeout
        self.send = send
        self.disconnect = disconnect
        self.schedule = schedule
        self.isOutbound = isOutbound
    }

    // MARK: - Thread-safe reads

    /// The peer that proved `address`, for the send path. Any thread.
    func handle(for address: String) -> Handle? {
        links.handle(for: address)
    }

    /// The address `handle`'s preamble proved, until the stream ends, or nil.
    /// Whether or not the stream went on to hold it: one refused as the
    /// losing duplicate proved its address all the same. The owner's queue.
    func provedAddress(of handle: Handle) -> String? {
        linkStates[handle]?.preamble.address
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
        guard !link.refused, sendPreambleIfNeeded(handle, link) else { return }

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
        // here then, and our preamble goes out before the peer is announced.
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
            // Ours before the announcement, when the peer's preamble beat our
            // connect event. Announcing makes the core send at once (the
            // outbox flush and the key package run inside the call), and
            // those go out on another queue; a message that reached the peer
            // ahead of our assertion would be refused and the peer lost.
            guard sendPreambleIfNeeded(handle, link) else { return }
            let announcement = links.announce(
                handle, address: address, outbound: isOutbound(handle),
                localAddress: host.peerStreamLocalAddress())
            if announcement.refused {
                // The losing kind of a second stream for a held address. The
                // far end computes the same rule and keeps the same stream,
                // so this closes with no report and the held one is untouched.
                link.refused = true
                disconnect(handle)
                host.peerStreamDiagnostic("info", "Peer stream refused: another stream holds the address", ["address": address])
                return
            }
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

    /// Sends our preamble once. Ours goes first and without waiting for
    /// theirs, so neither side can hold the other half-open by staying
    /// silent. False when it could not be sent, and the peer was refused.
    private func sendPreambleIfNeeded(_ handle: Handle, _ link: Link) -> Bool {
        if link.sentPreamble { return true }
        do {
            guard let host = host else { return false }
            let assertion = try host.peerStreamIdentityAssertion()
            guard let frame = PeerStreamFraming.frame(assertion) else {
                refuse(handle, link, reason: "our assertion is outside the frame bounds")
                return false
            }
            try send(frame, handle)
            link.sentPreamble = true
            return true
        } catch {
            refuse(handle, link, reason: "no preamble sent: \(error)")
            return false
        }
    }

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

/// When to dial an advertised address, and when to dial it again.
///
/// Foundation-only so `PeerStreamDialPolicyTests` drives it without a
/// network; `WifiDirectManager` owns the timers and the connections and asks
/// this for every decision. Not thread-safe: the manager calls it from its one
/// serial queue.
///
/// The ladder climbs only on a redial this returns. It used to climb on every
/// end of an outbound stream, before the check that declines a redial for an
/// address another stream holds, so the higher address of a pair, whose dial
/// loses to the lower's stream on every first contact, climbed a step per
/// lost race and waited out a long delay the first time it truly had to
/// reconnect.
struct PeerStreamDialPolicy {
    /// How long the higher address of a pair waits before dialing, so that
    /// the lower one's stream, which both ends keep, usually arrives first.
    static let higherAddressDelay: TimeInterval = 5.0
    static let redialInitialDelay: TimeInterval = 1.0
    static let redialMaxDelay: TimeInterval = 60.0
    /// Streams open at once, proved or not. Android's `Limits.maxStreams`.
    static let maxStreams = 16
    /// The inbound share of `maxStreams`. The rest is left to dials, so a
    /// listener full of strangers' sockets never stops this device reaching
    /// the peers it browses.
    static let maxInbound = 12
    /// Inbound streams one remote address may hold, proved or not. A host
    /// needs one, two while it reconnects past its own stale stream. Without
    /// the bound one address on the LAN fills the inbound share alone, with
    /// silent sockets that each hold a slot for the preamble deadline, or with
    /// as many self-made identities. The Python manager's
    /// `MAX_STREAMS_PER_HOST`, scaled to this budget. It counts addresses,
    /// not machines: one with many IPv6 addresses is many hosts here. What
    /// keeps dials possible whatever the listener holds is `maxInbound`'s
    /// reserve, so raising `maxInbound` to `maxStreams` gives that up.
    static let maxInboundPerHost = 4

    /// Addresses with a dial scheduled or an outbound stream open.
    private var dialing = Set<String>()
    private var redialDelay: [String: TimeInterval] = [:]

    /// A peer advertised `address`. The delay to dial after, or nil when no
    /// dial is due: one is already under way, or a stream holds the address.
    mutating func discovered(_ address: String, weAreLower: Bool, held: Bool) -> TimeInterval? {
        return schedule(address, held: held, after: weAreLower ? 0 : Self.higherAddressDelay)
    }

    /// An outbound stream toward `address` ended. The delay to redial after,
    /// or nil when the advert is gone or another stream holds the address.
    mutating func ended(_ address: String, advertised: Bool, held: Bool) -> TimeInterval? {
        dialing.remove(address)
        guard advertised else { return nil }
        let delay = redialDelay[address] ?? Self.redialInitialDelay
        guard schedule(address, held: held, after: delay) != nil else { return nil }
        redialDelay[address] = min(delay * 2, Self.redialMaxDelay)
        return delay
    }

    /// A scheduled dial opened nothing because it is not due: the advert
    /// went, the address is now held, or the transport paused (resuming
    /// rebuilds the browser, which reports every record again).
    mutating func abandoned(_ address: String) {
        dialing.remove(address)
    }

    /// A scheduled dial found every stream slot taken. The delay to try again
    /// after, on the redial ladder. Abandoning it lost the peer for good: an
    /// ending stream redials only its own address, and a browse change dials
    /// only a fresh record, so nothing would have dialed it again.
    mutating func noSlot(_ address: String) -> TimeInterval? {
        return ended(address, advertised: true, held: false)
    }

    /// An outbound stream toward `address` proved it: the ladder starts over.
    mutating func proved(_ address: String) {
        redialDelay.removeValue(forKey: address)
    }

    mutating func reset() {
        dialing = []
        redialDelay = [:]
    }

    /// Whether the listener takes a connection from `host`, given the hosts
    /// of the inbound streams open now and the count of all open streams.
    static func admitsInbound<Host: Equatable>(from host: Host, inboundFrom hosts: [Host], open: Int) -> Bool {
        return open < maxStreams && hosts.count < maxInbound
            && hosts.filter { $0 == host }.count < maxInboundPerHost
    }

    /// The record each advertised address is dialed at, from every peer
    /// record the browser holds now (`records`, our own already left out).
    ///
    /// Rebuilt whole on each change, never patched per record, because one
    /// address can be advertised by two records at once: a peer whose record
    /// outlived it in our cache, beside the one it published on return.
    /// Removing a record by its address took the live record's address with
    /// it, and with no advert the address was never dialed again. Of two
    /// records for one address, one in `fresh` (added or changed by this
    /// change) wins, being the newest; then the one in `current`, which a dial
    /// may already be using. A record in `unprovable` is left out: its dial
    /// was answered by a peer that did not prove the address it advertises,
    /// and dialing it again would only fail the same way.
    static func adverts<Endpoint: Hashable>(
        _ records: [(address: String, endpoint: Endpoint)],
        fresh: Set<Endpoint>,
        current: [String: Endpoint],
        unprovable: Set<Endpoint> = []
    ) -> [String: Endpoint] {
        func rank(_ address: String, _ endpoint: Endpoint) -> Int {
            fresh.contains(endpoint) ? 2 : current[address] == endpoint ? 1 : 0
        }
        var out: [String: Endpoint] = [:]
        for record in records where !unprovable.contains(record.endpoint) {
            if let kept = out[record.address],
               rank(record.address, kept) >= rank(record.address, record.endpoint) { continue }
            out[record.address] = record.endpoint
        }
        return out
    }

    private mutating func schedule(_ address: String, held: Bool, after delay: TimeInterval) -> TimeInterval? {
        guard !held, !dialing.contains(address) else { return nil }
        dialing.insert(address)
        return delay
    }
}
