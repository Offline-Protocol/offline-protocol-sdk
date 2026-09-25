//
// PeerStreamFraming.swift
// OfflineProtocol
//
// The parts of docs/spec/stream-framing.md a platform manager owns: the
// length prefix, its bounds, the position rule that makes the first body on a
// stream the peer's identity assertion, and one announced stream per address.
//
// Foundation only, so the SwiftPM harness tests it without a Multipeer session
// (PeerStreamFramingTests replays the chapter's conformance vectors).
// WifiDirectManager owns the session and calls into this. Mirrors android's
// PeerStreamFraming.kt, keep in sync.
//

import Foundation

/// Why a frame or a preamble was refused. The stream closes in every case.
struct PeerStreamRefusal: Error, Equatable {
    let reason: String
}

enum PeerStreamFraming {
    /// A `u32` big-endian length precedes every body.
    static let prefixBytes = 4

    /// The inclusive ceiling on a body, 1 MiB. Hand-mirrored from the chapter
    /// and the transport crate's `DEFAULT_MAX_MESSAGE_SIZE`, and pinned as a
    /// literal by the unit test (docs/bridges C5).
    static let maxBodyBytes = 1_048_576

    /// The identity assertion's own floor: a key and a signature.
    static let preambleMinBytes = 96

    /// The one frame `body` travels as, or nil for a body the far side would
    /// refuse, so a local bug fails here rather than as a peer that leaves.
    static func frame(_ body: Data) -> Data? {
        guard !body.isEmpty, body.count <= maxBodyBytes else { return nil }
        let n = UInt32(body.count)
        var out = Data(capacity: prefixBytes + body.count)
        out.append(UInt8(truncatingIfNeeded: n >> 24))
        out.append(UInt8(truncatingIfNeeded: n >> 16))
        out.append(UInt8(truncatingIfNeeded: n >> 8))
        out.append(UInt8(truncatingIfNeeded: n))
        out.append(body)
        return out
    }

    /// The refusal for a prefix, or nil if a body of `length` may be read.
    static func refusal(forLength length: UInt32, preamble: Bool) -> PeerStreamRefusal? {
        if length == 0 { return PeerStreamRefusal(reason: "zero length") }
        if length > UInt32(maxBodyBytes) { return PeerStreamRefusal(reason: "length over the ceiling") }
        if preamble && length < UInt32(preambleMinBytes) {
            return PeerStreamRefusal(reason: "preamble under the floor")
        }
        return nil
    }

    /// The body of one whole frame delivered by a message-oriented carrier.
    ///
    /// A Multipeer session already delivers whole messages, and the chapter
    /// still wraps each as one frame so that the ceiling is read off the same
    /// four bytes on every carrier. The prefix must therefore account for
    /// every byte after it: a message carrying more or less than its prefix
    /// says is refused, since there is no stream to resynchronise on.
    static func unframe(_ message: Data, preamble: Bool) -> Result<Data, PeerStreamRefusal> {
        guard message.count >= prefixBytes else {
            return .failure(PeerStreamRefusal(reason: "shorter than a prefix"))
        }
        let bytes = [UInt8](message.prefix(prefixBytes))
        let length = UInt32(bytes[0]) << 24 | UInt32(bytes[1]) << 16
            | UInt32(bytes[2]) << 8 | UInt32(bytes[3])
        if let refused = refusal(forLength: length, preamble: preamble) {
            return .failure(refused)
        }
        guard message.count - prefixBytes == Int(length) else {
            return .failure(PeerStreamRefusal(reason: "body length differs from its prefix"))
        }
        // Rebased, so the caller can index from zero.
        return .success(Data(message.dropFirst(prefixBytes)))
    }
}

/// The position rule for one stream: the first body is the peer's identity
/// assertion, and every later body is a message from the address it proved.
///
/// `verify` is `verifyIdentityAssertion` in production (steps one to three of
/// the Bluetooth LE chapter: parse, check the signature, derive) and returns
/// the derived address or throws. `expected` is a claim the stream was opened
/// toward, if any (step four); `localAddress` is ours, because a preamble that
/// proves our own address is a copy of ours played back to us.
///
/// Not thread-safe. One stream's owner drives its instance from one queue.
final class PeerStreamPreamble {
    enum Outcome: Equatable {
        /// The preamble verified: announce the address.
        case announce(String)
        /// A message from the proved address: hand the body upward.
        case deliver(address: String, body: Data)
        /// Close the stream and announce nothing.
        case refuse(String)
    }

    private let verify: (Data) throws -> String
    private let expected: String?
    private let localAddress: String?

    /// The proved address, once the preamble verified.
    private(set) var address: String?

    var awaitingPreamble: Bool { address == nil }

    init(
        verify: @escaping (Data) throws -> String,
        expected: String? = nil,
        localAddress: String? = nil
    ) {
        self.verify = verify
        self.expected = expected
        self.localAddress = localAddress
    }

    func accept(_ body: Data) -> Outcome {
        if let address = address {
            return .deliver(address: address, body: body)
        }
        guard body.count >= PeerStreamFraming.preambleMinBytes else {
            return .refuse("preamble under the floor")
        }
        let derived: String
        do {
            derived = try verify(body)
        } catch {
            return .refuse("preamble did not verify: \(error)")
        }
        // Exact, never case-folded: the Bluetooth LE chapter's reason holds.
        if let expected = expected, derived != expected {
            return .refuse("preamble proved an address other than the one expected")
        }
        if let localAddress = localAddress, derived == localAddress {
            return .refuse("preamble proved our own address")
        }
        address = derived
        return .announce(derived)
    }
}

/// One announced stream per address (the chapter's "What a receiver owes").
///
/// The core keys its peer-stream links by address, so a second announcement is
/// not a second link, and the first loss report removes the only one. A copied
/// preamble is enough to open a second stream for a live address, so the count
/// has to be kept here, where the streams are.
///
/// Policy: the newer stream supersedes the older. On a phone the duplicate is
/// almost always the same peer reconnecting past a stream that went half-open
/// (a Multipeer peer that restarted and came back under a new `MCPeerID`), and
/// refusing the newer one would leave that peer unreachable until the stale
/// one timed out. The cost, recorded in R16, is that a replayer can choose when
/// a real stream ends; it cannot use the stream it gets.
///
/// Thread-safe, and deliberately knows nothing of the protocol: the send path
/// reads it from whichever thread the core calls `onMessagesAvailable` on,
/// holding its global mutex, so nothing may ever call the core while holding
/// this lock.
final class PeerStreamLinks<Handle: Hashable> {
    struct Announcement: Equatable {
        /// True when no stream held this address: tell the core.
        let firstForAddress: Bool
        /// The older stream for the same address, to close without a loss report.
        let superseded: Handle?
    }

    private let lock = NSLock()
    private var byAddress: [String: Handle] = [:]
    private var byHandle: [Handle: String] = [:]

    func announce(_ handle: Handle, address: String) -> Announcement {
        lock.lock(); defer { lock.unlock() }
        if let held = byHandle[handle] {
            // A stream proves one address, once. Re-announcing the same one is
            // a caller bug answered as a no-op, which keeps the count right.
            precondition(held == address, "a stream cannot prove two addresses")
            return Announcement(firstForAddress: false, superseded: nil)
        }
        let older = byAddress.updateValue(handle, forKey: address)
        byHandle[handle] = address
        if let older = older { byHandle.removeValue(forKey: older) }
        return Announcement(firstForAddress: older == nil, superseded: older)
    }

    /// Forgets `handle` and returns the address to report lost, or nil when
    /// there is nothing to report: the stream never proved a peer, or a newer
    /// stream for the same address superseded it and still holds the link.
    @discardableResult
    func remove(_ handle: Handle) -> String? {
        lock.lock(); defer { lock.unlock() }
        guard let address = byHandle.removeValue(forKey: handle) else { return nil }
        if byAddress[address] == handle { byAddress.removeValue(forKey: address) }
        return address
    }

    /// Forgets every stream, returning each announced one with its address.
    func removeAll() -> [(handle: Handle, address: String)] {
        lock.lock(); defer { lock.unlock() }
        let live = byAddress.map { (handle: $0.value, address: $0.key) }
        byAddress.removeAll()
        byHandle.removeAll()
        return live
    }

    func handle(for address: String) -> Handle? {
        lock.lock(); defer { lock.unlock() }
        return byAddress[address]
    }

    func address(of handle: Handle) -> String? {
        lock.lock(); defer { lock.unlock() }
        return byHandle[handle]
    }

    var isEmpty: Bool {
        lock.lock(); defer { lock.unlock() }
        return byAddress.isEmpty
    }
}
