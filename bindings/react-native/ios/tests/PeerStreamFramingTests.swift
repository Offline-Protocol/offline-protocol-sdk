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
        let data = try! Data(contentsOf: vectorFile())
        return try! JSONSerialization.jsonObject(with: data) as! [String: Any]
    }()

    /// Walks up from this file until the vectors are beside it, as the Kotlin
    /// twin does. A fixed number of levels held only while this file sat in
    /// `ios/tests`, and the Swift package builds a copy of it somewhere else.
    private static func vectorFile() -> URL {
        let rel = "crates/offline-protocol-transport/tests/data/stream-framing-v1.vectors.json"
        // Standardized first: deleting the last component of a path that
        // ends in `..` appends another, and the walk never reaches the root.
        var dir = URL(fileURLWithPath: #filePath).standardizedFileURL
            .deletingLastPathComponent()
        while dir.path != "/" {
            let candidate = dir.appendingPathComponent(rel)
            if FileManager.default.fileExists(atPath: candidate.path) {
                return candidate
            }
            dir = dir.deletingLastPathComponent()
        }
        fatalError("cannot find \(rel) above \(#filePath)")
    }

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
    /// This device, lower than both, so the streams it opens win.
    private let me = "off1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq"

    private func announce(
        _ links: PeerStreamLinks<String>, _ handle: String, _ address: String,
        outbound: Bool = true
    ) -> PeerStreamLinks<String>.Announcement {
        links.announce(handle, address: address, outbound: outbound, localAddress: me)
    }

    func testTheFirstStreamForAnAddressIsAnnounced() {
        let links = PeerStreamLinks<String>()
        XCTAssertTrue(links.isEmpty)
        XCTAssertEqual(announce(links, "s1", a), .init(firstForAddress: true, superseded: nil))
        XCTAssertEqual(links.handle(for: a), "s1")
        XCTAssertFalse(links.isEmpty)
    }

    func testTheFirstStreamIsAnnouncedWhicheverKindItIs() {
        let links = PeerStreamLinks<String>()
        XCTAssertEqual(announce(links, "in", a, outbound: false), .init(firstForAddress: true, superseded: nil))
    }

    func testANewerStreamSupersedesAndTheCoreSeesOneAnnouncementAndOneLoss() {
        let links = PeerStreamLinks<String>()
        _ = announce(links, "old", a)

        let second = announce(links, "new", a)
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

    func testASecondStreamOfTheLosingKindIsRefusedAndTheHeldOneStays() {
        let links = PeerStreamLinks<String>()
        _ = announce(links, "out", a)
        XCTAssertEqual(
            announce(links, "in", a, outbound: false),
            .init(firstForAddress: false, superseded: nil, refused: true))
        XCTAssertEqual(links.handle(for: a), "out")
        XCTAssertNil(links.remove("in"), "a refused stream was never announced")
        XCTAssertEqual(links.remove("out"), a)
    }

    func testTheWinningKindIsWhateverTheLowerAddressOpened() {
        // Python's `_new_stream_wins`, case for case (ADR 0027).
        XCTAssertTrue(PeerStreamLinks<String>.newStreamWins(outbound: true, localAddress: me, peer: a))
        XCTAssertFalse(PeerStreamLinks<String>.newStreamWins(outbound: false, localAddress: me, peer: a))
        XCTAssertFalse(PeerStreamLinks<String>.newStreamWins(outbound: true, localAddress: a, peer: me))
        XCTAssertTrue(PeerStreamLinks<String>.newStreamWins(outbound: false, localAddress: a, peer: me))
        XCTAssertTrue(PeerStreamLinks<String>.newStreamWins(outbound: true, localAddress: b, peer: a),
                      "b sorts below a at the first differing character")
        XCTAssertFalse(PeerStreamLinks<String>.newStreamWins(outbound: true, localAddress: nil, peer: a))
        XCTAssertFalse(PeerStreamLinks<String>.newStreamWins(outbound: false, localAddress: nil, peer: a))
    }

    func testBothEndsOfAPairKeepTheSameStream() {
        // `mine` is the stream the lower address (me) opened; `theirs` the
        // other. Each end sees one as outbound and the other as inbound.
        for firstArrives in ["mine", "theirs"] {
            let low = PeerStreamLinks<String>()
            let high = PeerStreamLinks<String>()
            let order = firstArrives == "mine" ? ["mine", "theirs"] : ["theirs", "mine"]
            for stream in order {
                _ = low.announce(stream, address: a, outbound: stream == "mine", localAddress: me)
                _ = high.announce(stream, address: me, outbound: stream == "theirs", localAddress: a)
            }
            XCTAssertEqual(low.handle(for: a), "mine", firstArrives)
            XCTAssertEqual(high.handle(for: me), "mine", firstArrives)
        }
    }

    func testAStreamThatNeverProvedAPeerReportsNothing() {
        XCTAssertNil(PeerStreamLinks<String>().remove("never-announced"))
    }

    func testAStreamIsReportedLostOnce() {
        let links = PeerStreamLinks<String>()
        _ = announce(links, "s1", a)
        XCTAssertEqual(links.remove("s1"), a)
        XCTAssertNil(links.remove("s1"))
    }

    func testASecondAnnouncementForOneStreamIsANoOp() {
        let links = PeerStreamLinks<String>()
        _ = announce(links, "s1", a)
        XCTAssertEqual(announce(links, "s1", a), .init(firstForAddress: false, superseded: nil))
        // Another address is refused the same way, and the first stays held.
        XCTAssertEqual(announce(links, "s1", b), .init(firstForAddress: false, superseded: nil))
        XCTAssertEqual(links.handle(for: a), "s1")
        XCTAssertNil(links.handle(for: b))
        XCTAssertEqual(links.remove("s1"), a)
        XCTAssertTrue(links.isEmpty)
    }

    func testAddressesAreIndependent() {
        let links = PeerStreamLinks<String>()
        _ = announce(links, "s1", a)
        XCTAssertEqual(announce(links, "s2", b), .init(firstForAddress: true, superseded: nil))
        XCTAssertEqual(links.remove("s1"), a)
        XCTAssertEqual(links.handle(for: b), "s2")
    }

    func testRemoveAllReturnsOnlyTheLiveStreams() {
        let links = PeerStreamLinks<String>()
        _ = announce(links, "old", a)
        _ = announce(links, "new", a)
        _ = announce(links, "s2", b)
        let live = links.removeAll().map { "\($0.handle)=\($0.address)" }
        XCTAssertEqual(Set(live), ["new=\(a)", "s2=\(b)"])
        XCTAssertTrue(links.isEmpty)
        XCTAssertNil(links.remove("new"))
    }

    func testReAnnouncingAStreamIsANoOp() {
        let links = PeerStreamLinks<String>()
        _ = announce(links, "s1", a)
        XCTAssertEqual(announce(links, "s1", a), .init(firstForAddress: false, superseded: nil))
        XCTAssertEqual(links.handle(for: a), "s1")
    }

    /// The advert is the only way a peer finds this one, so a wrong length
    /// byte or key leaves the iPhone invisible to every host, silently.
    func testTheAdvertCarriesTxtversFirstThenTheAddress() {
        let address = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
        var expected = Data([9]) + Data("txtvers=1".utf8)
        expected += Data([UInt8(5 + address.utf8.count)]) + Data("addr=\(address)".utf8)
        XCTAssertEqual(PeerStreamFraming.txtRecord(address: address), expected)
        XCTAssertEqual(PeerStreamFraming.txtRecord(address: nil), Data([9]) + Data("txtvers=1".utf8))
        XCTAssertEqual(PeerStreamFraming.txtRecord(address: String(repeating: "x", count: 251)),
                       Data([9]) + Data("txtvers=1".utf8), "an entry its length byte cannot say is left out")
        // Python's `service_instance_name` for the same address.
        XCTAssertEqual(PeerStreamFraming.instanceName(address: address), "op-b701bfc1c92768b5")
        XCTAssertTrue(PeerStreamFraming.instanceName(address: nil).hasPrefix("op-"))
    }
}
