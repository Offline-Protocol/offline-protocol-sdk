//
// PeerStreamFramingTests.swift
//
// Pins the Wi-Fi Direct manager's half of docs/spec/stream-framing.md: the
// length prefix and its bounds, the position rule for the preamble, and one
// announced peer per address. Mirrors android's PeerStreamFramingTest, keep
// in sync.
//
// The framing cases replay the chapter's conformance vectors
// (crates/offline-protocol-transport/tests/data/stream-framing-v1.vectors.json),
// so this reader and the Rust reference are checked against the same bytes.
// The verifier is a stand-in here: the real one is Rust, needs the native
// library, and is tested against the identity assertion vectors in Rust.
//

import XCTest
@testable import OfflineProtocol

final class PeerStreamFramingTests: XCTestCase {

    private struct Refused: Error {}

    private static let vectors: [String: Any] = {
        // tests/ -> ios/ -> react-native/ -> bindings/ -> repository root
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
        let url = root.appendingPathComponent(
            "crates/offline-protocol-transport/tests/data/stream-framing-v1.vectors.json")
        let data = try! Data(contentsOf: url)
        return try! JSONSerialization.jsonObject(with: data) as! [String: Any]
    }()

    private func hex(_ s: String) -> Data {
        var out = Data(capacity: s.count / 2)
        var i = s.startIndex
        while i < s.endIndex {
            let j = s.index(i, offsetBy: 2)
            out.append(UInt8(s[i..<j], radix: 16)!)
            i = j
        }
        return out
    }

    private func object(_ key: String) -> [String: Any] {
        Self.vectors[key] as! [String: Any]
    }

    private var preambleBody: Data { hex(object("preamble")["body_hex"] as! String) }
    private var preambleAddress: String { object("preamble")["address"] as! String }

    /// Verifies exactly the vector preamble, as the real verifier would.
    private var verifier: (Data) throws -> String {
        let body = preambleBody
        let address = preambleAddress
        return { candidate in
            guard candidate == body else { throw Refused() }
            return address
        }
    }

    // MARK: - The hand-mirrored sizes (docs/bridges C5)

    func testTheSizesAreTheChapters() {
        // Literals, not a read of the vector file: a test that read them from
        // the source it checks would agree with any edit made to both.
        XCTAssertEqual(PeerStreamFraming.prefixBytes, 4)
        XCTAssertEqual(PeerStreamFraming.maxBodyBytes, 1_048_576)
        XCTAssertEqual(PeerStreamFraming.preambleMinBytes, 96)
        XCTAssertEqual(Self.vectors["ceiling"] as? Int, 1_048_576)
        XCTAssertEqual(Self.vectors["preamble_floor"] as? Int, 96)
    }

    // MARK: - The frame

    func testFrameWritesTheVectorBytes() {
        for key in ["preamble", "message"] {
            let v = object(key)
            XCTAssertEqual(
                PeerStreamFraming.frame(hex(v["body_hex"] as! String)),
                hex(v["frame_hex"] as! String),
                key
            )
        }
    }

    func testFrameRefusesWhatAReceiverWouldRefuse() {
        XCTAssertNil(PeerStreamFraming.frame(Data()))
        XCTAssertNil(PeerStreamFraming.frame(Data(count: 1_048_577)))
        XCTAssertEqual(PeerStreamFraming.frame(Data(count: 1_048_576))?.count, 4 + 1_048_576)
    }

    // MARK: - The reader

    func testTheExchangeAnnouncesThenDelivers() throws {
        let exchange = object("exchange")
        let frames = (exchange["frames"] as! [String]).map(hex)
        let preamble = PeerStreamPreamble(verify: verifier)

        let first = try PeerStreamFraming.unframe(frames[0], preamble: true).get()
        XCTAssertEqual(preamble.accept(first), .announce(exchange["announces"] as! String))

        let second = try PeerStreamFraming.unframe(frames[1], preamble: false).get()
        let delivered = (exchange["delivers"] as! [[String: Any]])[0]
        XCTAssertEqual(
            preamble.accept(second),
            .deliver(address: delivered["from"] as! String, body: hex(delivered["body_hex"] as! String))
        )
    }

    func testAPrefixOfExactlyTheCeilingIsRead() throws {
        let accepted = (Self.vectors["accepted_prefixes"] as! [[String: Any]])[0]
        var message = hex(accepted["prefix_hex"] as! String)
        message.append(Data(repeating: 7, count: accepted["length"] as! Int))
        XCTAssertEqual(try PeerStreamFraming.unframe(message, preamble: false).get().count, 1_048_576)
    }

    func testEveryRefusalVectorClosesTheStream() {
        let refusals = Self.vectors["refusals"] as! [[String: Any]]
        XCTAssertEqual(refusals.count, 4)
        for r in refusals {
            let name = r["name"] as! String
            let message = hex((r["frames"] as! [String])[0])
            // Every vector is the stream's first frame, so it is read as the
            // preamble. A refusal is a failed unframe, or a refuse outcome
            // from the position rule; never an announcement.
            switch PeerStreamFraming.unframe(message, preamble: true) {
            case .failure:
                continue
            case .success(let body):
                if case .refuse = PeerStreamPreamble(verify: verifier).accept(body) { continue }
                XCTFail("\(name) must be refused")
            }
        }
    }

    func testARefusedLengthIsRefusedOnThePrefix() {
        for (prefix, preamble) in [
            ("00000000", false),
            ("00100001", false),
            ("80000000", false),
            ("ffffffff", false),
            ("0000005f", true),
        ] {
            guard case .failure(let refusal) = PeerStreamFraming.unframe(hex(prefix), preamble: preamble) else {
                XCTFail("\(prefix) must be refused")
                continue
            }
            // Refused for its length, not for a body it never carried.
            XCTAssertNotEqual(refusal.reason, "body length differs from its prefix", prefix)
        }
    }

    func testAMessageThatDisagreesWithItsPrefixIsRefused() {
        let frame = hex(object("message")["frame_hex"] as! String)
        guard case .failure = PeerStreamFraming.unframe(frame.dropLast(), preamble: false) else {
            return XCTFail("a short body must be refused")
        }
        var long = frame
        long.append(0)
        guard case .failure = PeerStreamFraming.unframe(long, preamble: false) else {
            return XCTFail("trailing bytes must be refused: a message is one frame")
        }
        guard case .failure = PeerStreamFraming.unframe(Data([0, 0, 1]), preamble: false) else {
            return XCTFail("a message shorter than a prefix must be refused")
        }
    }

    func testUnframeRebasesASlice() throws {
        // A Data slice keeps its parent's indices; the body must not.
        var padded = Data([0xAA, 0xBB])
        padded.append(hex(object("message")["frame_hex"] as! String))
        let slice = padded.dropFirst(2)
        let body = try PeerStreamFraming.unframe(slice, preamble: false).get()
        XCTAssertEqual(body.startIndex, 0)
        XCTAssertEqual(body, hex(object("message")["body_hex"] as! String))
    }

    // MARK: - The position rule

    func testAPreambleThatDoesNotVerifyIsRefused() {
        let preamble = PeerStreamPreamble(verify: { _ in throw Refused() })
        guard case .refuse = preamble.accept(preambleBody) else { return XCTFail("must refuse") }
        XCTAssertTrue(preamble.awaitingPreamble)
        XCTAssertNil(preamble.address)
    }

    func testAPreambleUnderTheFloorNeverReachesTheVerifier() {
        var called = false
        let preamble = PeerStreamPreamble(verify: { _ in called = true; return "x" })
        guard case .refuse = preamble.accept(Data(count: 95)) else { return XCTFail("must refuse") }
        XCTAssertFalse(called)
    }

    func testAClaimIsComparedExactly() {
        XCTAssertEqual(
            PeerStreamPreamble(verify: verifier, expected: preambleAddress).accept(preambleBody),
            .announce(preambleAddress)
        )
        guard case .refuse = PeerStreamPreamble(verify: verifier, expected: preambleAddress.uppercased())
            .accept(preambleBody) else { return XCTFail("a case-folded claim must not match") }
    }

    func testOurOwnAddressPlayedBackIsRefused() {
        guard case .refuse = PeerStreamPreamble(verify: verifier, localAddress: preambleAddress)
            .accept(preambleBody) else { return XCTFail("must refuse our own address") }
    }

    func testASecondAssertionAfterTheFirstIsAMessage() {
        let preamble = PeerStreamPreamble(verify: verifier)
        _ = preamble.accept(preambleBody)
        XCTAssertEqual(preamble.accept(preambleBody), .deliver(address: preambleAddress, body: preambleBody))
    }

    // MARK: - One announced peer per address

    private let a = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
    private let b = "off1qyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqn8antf"

    func testTheFirstStreamForAnAddressIsAnnounced() {
        let links = PeerStreamLinks<String>()
        XCTAssertTrue(links.isEmpty)
        XCTAssertEqual(links.announce("s1", address: a), .init(firstForAddress: true, superseded: nil))
        XCTAssertEqual(links.handle(for: a), "s1")
        XCTAssertFalse(links.isEmpty)
    }

    func testANewerStreamSupersedesAndTheCoreSeesOneAnnouncementAndOneLoss() {
        let links = PeerStreamLinks<String>()
        _ = links.announce("old", address: a)

        let second = links.announce("new", address: a)
        XCTAssertFalse(second.firstForAddress, "a second announcement would tell the core nothing new")
        XCTAssertEqual(second.superseded, "old")
        XCTAssertEqual(links.handle(for: a), "new")

        // The superseded stream's end reports nothing: reporting it would
        // remove the live link the newer stream holds.
        XCTAssertNil(links.remove("old"))
        XCTAssertEqual(links.handle(for: a), "new")

        XCTAssertEqual(links.remove("new"), a)
        XCTAssertNil(links.handle(for: a))
        XCTAssertTrue(links.isEmpty)
    }

    func testAStreamThatNeverProvedAPeerReportsNothing() {
        XCTAssertNil(PeerStreamLinks<String>().remove("never-announced"))
    }

    func testAStreamIsReportedLostOnce() {
        let links = PeerStreamLinks<String>()
        _ = links.announce("s1", address: a)
        XCTAssertEqual(links.remove("s1"), a)
        XCTAssertNil(links.remove("s1"))
    }

    func testASecondAnnouncementForOneStreamIsANoOp() {
        let links = PeerStreamLinks<String>()
        _ = links.announce("s1", address: a)
        XCTAssertEqual(links.announce("s1", address: a), .init(firstForAddress: false, superseded: nil))
        // Another address is refused the same way, and the first stays held.
        XCTAssertEqual(links.announce("s1", address: b), .init(firstForAddress: false, superseded: nil))
        XCTAssertEqual(links.handle(for: a), "s1")
        XCTAssertNil(links.handle(for: b))
        XCTAssertEqual(links.remove("s1"), a)
        XCTAssertTrue(links.isEmpty)
    }

    func testAddressesAreIndependent() {
        let links = PeerStreamLinks<String>()
        _ = links.announce("s1", address: a)
        XCTAssertEqual(links.announce("s2", address: b), .init(firstForAddress: true, superseded: nil))
        XCTAssertEqual(links.remove("s1"), a)
        XCTAssertEqual(links.handle(for: b), "s2")
    }

    func testRemoveAllReturnsOnlyTheLiveStreams() {
        let links = PeerStreamLinks<String>()
        _ = links.announce("old", address: a)
        _ = links.announce("new", address: a)
        _ = links.announce("s2", address: b)
        let live = links.removeAll().map { "\($0.handle)=\($0.address)" }
        XCTAssertEqual(Set(live), ["new=\(a)", "s2=\(b)"])
        XCTAssertTrue(links.isEmpty)
        XCTAssertNil(links.remove("new"))
    }

    func testReAnnouncingAStreamIsANoOp() {
        let links = PeerStreamLinks<String>()
        _ = links.announce("s1", address: a)
        XCTAssertEqual(links.announce("s1", address: a), .init(firstForAddress: false, superseded: nil))
        XCTAssertEqual(links.handle(for: a), "s1")
    }
}
