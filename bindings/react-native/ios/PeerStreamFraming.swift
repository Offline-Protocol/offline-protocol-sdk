//
// PeerStreamFraming.swift
// OfflineProtocol
//
// The parts of docs/spec/stream-framing.md a platform manager owns: the
// length prefix, its bounds, the position rule that makes the first body on a
// stream the peer's identity assertion, one announced stream per address, and
// the DNS-SD advert a peer finds it by.
//
// Foundation and CryptoKit only, so the SwiftPM harness tests it without a socket
// (PeerStreamFramingTests replays the chapter's conformance vectors).
// WifiDirectManager owns the connections and calls into this. Mirrors
// android's PeerStreamFraming.kt, keep in sync.
//

import CryptoKit
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

    /// The body of one whole frame, as `PeerStreamReader` cuts it off the
    /// stream.
    ///
    /// The prefix must account for every byte after it: a message carrying
    /// more or less than its prefix says is refused. The reader already
    /// refused a length over the ceiling before buffering its body; the
    /// preamble floor is checked here, where the position is known.
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

    /// The DNS-SD TXT record, built by hand because the chapter requires
    /// `txtvers=1` to be the first entry and `NWTXTRecord` does not promise an
    /// order. `addr` is absent until this device has an identity. An entry
    /// longer than its one length byte can say is left out rather than
    /// trapping the host app; an address is far shorter.
    static func txtRecord(address: String?) -> Data {
        var entries = ["txtvers=1"]
        if let address = address, !address.isEmpty {
            entries.append("addr=\(address)")
        }
        var out = Data()
        for entry in entries {
            let bytes = Data(entry.utf8)
            guard let length = UInt8(exactly: bytes.count) else { continue }
            out.append(length)
            out.append(bytes)
        }
        return out
    }

    /// The DNS-SD instance name: a digest of the address, as the Python
    /// manager names its own. A restarted listener (every return from the
    /// background) then replaces its record in each peer's cache instead of
    /// publishing a second one beside the stale one. Random only while there
    /// is no identity, when the record carries no address and no browser
    /// dials it.
    static func instanceName(address: String?) -> String {
        guard let address = address else { return "op-\(UUID().uuidString.prefix(8).lowercased())" }
        let digest = SHA256.hash(data: Data(address.utf8))
        return "op-" + digest.prefix(8).map { String(format: "%02x", $0) }.joined()
    }
}

/// Cuts whole frames, prefix included, off a byte stream (the chapter's
/// "What a receiver owes", steps one and two).
///
/// A prefix over the ceiling, or zero, is refused the moment its four bytes
/// are in, before a byte of the body is buffered: a reader that buffered first
/// would hand the peer a megabyte of this device's memory per stream for four
/// bytes. So the reader holds at most one frame, the ceiling plus the prefix.
/// After a refusal the stream is garbage and the reader returns nothing more;
/// the owner closes the stream.
///
/// Not thread-safe. One stream's owner drives its instance from one queue.
final class PeerStreamReader {
    private var buffer = Data()
    private var refused = false

    /// Every frame `chunk` completes, in order, ending with a refusal if one
    /// was met.
    func append(_ chunk: Data) -> [Result<Data, PeerStreamRefusal>] {
        guard !refused else { return [] }
        buffer.append(chunk)
        var out: [Result<Data, PeerStreamRefusal>] = []
        // Frames are read at an offset and the buffer compacted once at the
        // end. Compacting per frame re-copied the rest of the buffer each
        // time, so a chunk of many small frames cost its size squared.
        var start = buffer.startIndex
        while buffer.endIndex - start >= PeerStreamFraming.prefixBytes {
            let length = UInt32(buffer[start]) << 24 | UInt32(buffer[start + 1]) << 16
                | UInt32(buffer[start + 2]) << 8 | UInt32(buffer[start + 3])
            // The floor is `unframe`'s to check, which knows the position.
            if let refusal = PeerStreamFraming.refusal(forLength: length, preamble: false) {
                refused = true
                buffer = Data()
                out.append(.failure(refusal))
                return out
            }
            let end = start + PeerStreamFraming.prefixBytes + Int(length)
            guard buffer.endIndex >= end else { break }
            out.append(.success(Data(buffer[start..<end])))
            start = end
        }
        if start != buffer.startIndex {
            buffer = Data(buffer[start...])
        }
        return out
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
/// Policy: the stream the lower address opened is kept, and between two of
/// those the newer supersedes the older. Both ends compute it alike, which is
/// the point: both ends of a pair may dial, and so does a Python host on the
/// same LAN, so without a shared rule each end would keep the stream the other
/// closes, and the pair would reconnect forever. It is the Python manager's
/// `_new_stream_wins`, and `ios_and_python_peer_streams_keep_the_same_stream`
/// pins the two copies together (ADR 0027). "Newer" among winners is what
/// lets the lower address reconnect past its own half-open stream; the higher
/// address's reconnect waits for keepalive to end the stale one. The cost,
/// recorded in R16, is that a replayer can end a real stream when its copy is
/// the winning kind; it cannot use the stream it gets.
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
        /// True when an older stream holds the address and wins: close this
        /// one without a report, and leave the older untouched.
        var refused = false
    }

    /// Whether a new stream for `peer` takes the address over from the one
    /// that holds it. `outbound` is whether this device opened the new
    /// stream. Addresses compare by their UTF-8 bytes, which is the code point
    /// order Python's `<` uses. With no address of our own there is nothing to
    /// order by, and the announced stream stays.
    static func newStreamWins(outbound: Bool, localAddress: String?, peer: String) -> Bool {
        guard let local = localAddress else { return false }
        let weOpen = local.utf8.lexicographicallyPrecedes(peer.utf8)
        return outbound == weOpen
    }

    private let lock = NSLock()
    private var byAddress: [String: Handle] = [:]
    private var byHandle: [Handle: String] = [:]

    func announce(
        _ handle: Handle, address: String, outbound: Bool, localAddress: String?
    ) -> Announcement {
        lock.lock(); defer { lock.unlock() }
        if byHandle[handle] != nil {
            // A stream proves one address, once. A second announcement, for
            // the same address or another, is a caller bug answered as a
            // no-op: the count stays right and the first address stays held.
            // Not a precondition, because this runs inside the host app and a
            // trap here would take the app down for a bookkeeping mistake.
            return Announcement(firstForAddress: false, superseded: nil)
        }
        if byAddress[address] != nil,
           !Self.newStreamWins(outbound: outbound, localAddress: localAddress, peer: address) {
            return Announcement(firstForAddress: false, superseded: nil, refused: true)
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
