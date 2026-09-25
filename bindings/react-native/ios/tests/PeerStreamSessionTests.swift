//
// PeerStreamSessionTests.swift
//
// Drives PeerStreamSession with string handles, a fake carrier and a manual
// clock: the iOS Wi-Fi Direct manager's per-peer behaviour without a
// Multipeer session, which no CI runner has. Mirrors android's
// PeerStreamSocketsTest, keep in sync.
//
// The verifier is a stand-in (the real one is Rust, needs the native library,
// and is tested against the identity assertion vectors in Rust): an
// "assertion" here is a `peer-…` address in ASCII, zero-padded to the 96-byte
// floor, and nothing else verifies. What a test cannot see is the manager's
// session handling (advertising, browsing, inviting), which needs a device.
// The device run is recorded in the PR (docs/bridges C9).
//

import XCTest
@testable import OfflineProtocol

final class PeerStreamSessionTests: XCTestCase {

    private struct Refused: Error {}

    private final class Host: PeerStreamHost {
        let me: String
        var events: [String] = []
        var bodies: [Data] = []
        var failAssertion = false
        var onConnected: (() -> Void)?

        init(_ me: String) { self.me = me }

        func peerStreamIdentityAssertion() throws -> Data {
            if failAssertion { throw Refused() }
            return PeerStreamSessionTests.assertion(me)
        }
        func peerStreamLocalAddress() -> String? { me }
        func peerStreamVerify(_ assertion: Data) throws -> String {
            let name = assertion.prefix { $0 != 0 }
            guard let address = String(data: name, encoding: .ascii),
                  address.hasPrefix("peer-"),
                  address.allSatisfy({ $0.isLowercase || $0.isNumber || $0 == "-" }),
                  assertion.dropFirst(name.count).allSatisfy({ $0 == 0 }) else { throw Refused() }
            return address
        }
        func peerStreamConnected(_ address: String) {
            onConnected?()
            events.append("connected:\(address)")
        }
        func peerStreamReceived(_ address: String, _ body: Data) {
            events.append("message:\(address):\(body.count)")
            bodies.append(body)
        }
        func peerStreamLost(_ address: String) { events.append("lost:\(address)") }
        func peerStreamDiagnostic(_ level: String, _ message: String, _ context: [String: Any]) {}
    }

    static func assertion(_ address: String) -> Data {
        var data = Data(address.utf8)
        data.append(Data(count: PeerStreamFraming.preambleMinBytes - data.count))
        return data
    }

    private func frame(_ body: Data) -> Data { PeerStreamFraming.frame(body)! }

    private var host: Host!
    private var sent: [(handle: String, frame: Data)] = []
    private var disconnected: [String] = []
    private var timers: [(delay: TimeInterval, block: () -> Void)] = []
    private var failSend = false
    private var session: PeerStreamSession<String>!

    override func setUp() {
        super.setUp()
        host = Host("peer-a")
        sent = []
        disconnected = []
        timers = []
        failSend = false
        session = PeerStreamSession<String>(
            host: host,
            preambleTimeout: 10,
            send: { [unowned self] frame, handle in
                if self.failSend { throw Refused() }
                self.sent.append((handle, frame))
            },
            disconnect: { [unowned self] handle in self.disconnected.append(handle) },
            schedule: { [unowned self] delay, block in self.timers.append((delay, block)) }
        )
    }

    private func fireTimers() {
        let due = timers
        timers = []
        for timer in due { timer.block() }
    }

    /// Connects `handle` and has it prove `address`.
    private func prove(_ handle: String, as address: String) {
        session.connected(handle)
        session.received(frame(Self.assertion(address)), from: handle)
    }

    // MARK: - The exchange

    func testConnectSendsOurFramedPreambleFirstAndOnce() {
        session.connected("h1")
        XCTAssertEqual(sent.count, 1)
        XCTAssertEqual(sent[0].handle, "h1")
        XCTAssertEqual(sent[0].frame, frame(Self.assertion("peer-a")))
        session.connected("h1")
        XCTAssertEqual(sent.count, 1)
        XCTAssertEqual(timers.count, 1)
        XCTAssertEqual(timers[0].delay, 10)
    }

    func testAPreambleAnnouncesAndLaterMessagesAreDelivered() {
        prove("h1", as: "peer-b")
        XCTAssertEqual(host.events, ["connected:peer-b"])
        XCTAssertEqual(session.handle(for: "peer-b"), "h1")
        XCTAssertFalse(session.isEmpty)

        session.received(frame(Data("hello".utf8)), from: "h1")
        XCTAssertEqual(host.events.last, "message:peer-b:5")
        XCTAssertEqual(host.bodies.last, Data("hello".utf8))

        session.ended("h1")
        XCTAssertEqual(host.events, ["connected:peer-b", "message:peer-b:5", "lost:peer-b"])
        XCTAssertTrue(session.isEmpty)
    }

    func testAMessageBeforeTheConnectEventIsStillThePreamble() {
        // The carrier may deliver the peer's first message before our connect
        // callback runs; the preamble is accepted and ours still goes out,
        // before the announcement, because announcing makes the core send.
        host.onConnected = { [unowned self] in
            XCTAssertEqual(self.sent.count, 1, "our preamble must precede the announcement")
        }
        session.received(frame(Self.assertion("peer-b")), from: "h1")
        XCTAssertEqual(host.events, ["connected:peer-b"])
        XCTAssertEqual(sent.map { $0.frame }, [frame(Self.assertion("peer-a"))])
        session.connected("h1")
        XCTAssertEqual(sent.count, 1, "the connect event does not send it twice")
        XCTAssertTrue(timers.isEmpty, "a proved peer needs no deadline")
    }

    func testAFailedPreambleSendBeforeTheConnectEventAnnouncesNothing() {
        failSend = true
        session.received(frame(Self.assertion("peer-b")), from: "h1")
        XCTAssertEqual(disconnected, ["h1"])
        XCTAssertTrue(host.events.isEmpty)
        XCTAssertTrue(session.isEmpty)
    }

    func testABodyOfExactlyTheCeilingIsDelivered() {
        prove("h1", as: "peer-b")
        session.received(frame(Data(count: 1_048_576)), from: "h1")
        XCTAssertEqual(host.events.last, "message:peer-b:1048576")
    }

    // MARK: - Refusals

    func testAPreambleThatDoesNotVerifyDisconnectsAndAnnouncesNothing() {
        prove("h1", as: "bad_peer")
        XCTAssertEqual(disconnected, ["h1"])
        XCTAssertTrue(host.events.isEmpty)
        // Nothing more from it is read, even a valid preamble.
        session.received(frame(Self.assertion("peer-b")), from: "h1")
        XCTAssertTrue(host.events.isEmpty)
        session.ended("h1")
        XCTAssertTrue(host.events.isEmpty)
    }

    func testAMessageBeforeAnyPreambleIsRefused() {
        session.connected("h1")
        session.received(frame(Data(repeating: 0xF5, count: 149)), from: "h1")
        XCTAssertEqual(disconnected, ["h1"])
        XCTAssertTrue(host.events.isEmpty)
    }

    func testOurOwnAddressPlayedBackIsRefused() {
        prove("h1", as: "peer-a")
        XCTAssertEqual(disconnected, ["h1"])
        XCTAssertTrue(host.events.isEmpty)
    }

    func testAClaimIsCheckedAgainstThePreamble() {
        session.claim("peer-c", for: "h1")
        prove("h1", as: "peer-b")
        XCTAssertEqual(disconnected, ["h1"])
        XCTAssertTrue(host.events.isEmpty)

        session.claim("peer-b", for: "h2")
        prove("h2", as: "peer-b")
        XCTAssertEqual(host.events, ["connected:peer-b"])
    }

    func testAFrameRefusedAfterTheAnnouncementReportsOneLossNow() {
        prove("h1", as: "peer-b")
        var bad = frame(Data("hello".utf8))
        bad.append(0) // a message that disagrees with its prefix
        session.received(bad, from: "h1")
        XCTAssertEqual(disconnected, ["h1"])
        XCTAssertEqual(host.events, ["connected:peer-b", "lost:peer-b"])
        // The carrier's own disconnect event then finds nothing to report.
        session.ended("h1")
        XCTAssertEqual(host.events, ["connected:peer-b", "lost:peer-b"])
    }

    func testAnOversizePrefixIsRefused() {
        prove("h1", as: "peer-b")
        session.received(Data([0x00, 0x10, 0x00, 0x01]), from: "h1")
        XCTAssertEqual(host.events, ["connected:peer-b", "lost:peer-b"])
    }

    func testASilentPeerIsDisconnectedAtTheDeadline() {
        session.connected("h1")
        fireTimers()
        XCTAssertEqual(disconnected, ["h1"])
        XCTAssertTrue(host.events.isEmpty)
    }

    func testTheDeadlineDoesNothingOnceThePeerProvedItself() {
        prove("h1", as: "peer-b")
        fireTimers()
        XCTAssertTrue(disconnected.isEmpty)
        XCTAssertEqual(host.events, ["connected:peer-b"])
    }

    func testNoIdentityToPresentDisconnects() {
        host.failAssertion = true
        session.connected("h1")
        XCTAssertEqual(disconnected, ["h1"])
        XCTAssertTrue(sent.isEmpty)
    }

    func testAFailedPreambleSendDisconnects() {
        failSend = true
        session.connected("h1")
        XCTAssertEqual(disconnected, ["h1"])
    }

    // MARK: - One announced peer per address

    func testANewerPeerSupersedesWithOneAnnouncementAndOneLoss() {
        prove("old", as: "peer-b")
        prove("new", as: "peer-b")
        XCTAssertEqual(disconnected, ["old"])
        XCTAssertEqual(host.events, ["connected:peer-b"], "no second announcement, no loss")
        XCTAssertEqual(session.handle(for: "peer-b"), "new")

        // The superseded peer's late message and its disconnect are silent.
        session.received(frame(Data("late".utf8)), from: "old")
        session.ended("old")
        XCTAssertEqual(host.events, ["connected:peer-b"])

        session.received(frame(Data("fresh".utf8)), from: "new")
        session.ended("new")
        XCTAssertEqual(host.events, ["connected:peer-b", "message:peer-b:5", "lost:peer-b"])
    }

    func testEndAllReportsEachAnnouncedPeerOnce() {
        prove("h1", as: "peer-b")
        prove("h2", as: "peer-c")
        session.connected("h3") // never proves anything
        session.endAll()
        XCTAssertEqual(Set(host.events.filter { $0.hasPrefix("lost:") }), ["lost:peer-b", "lost:peer-c"])
        XCTAssertEqual(host.events.filter { $0.hasPrefix("lost:") }.count, 2)
        XCTAssertTrue(session.isEmpty)

        // The session's own disconnect events afterwards report nothing.
        let before = host.events
        session.ended("h1")
        session.ended("h2")
        session.ended("h3")
        XCTAssertEqual(host.events, before)
        // A pending deadline for the unproved peer finds it gone.
        fireTimers()
        XCTAssertTrue(disconnected.isEmpty)
    }

    func testAnUnprovedPeerHasNothingToSendTo() {
        session.connected("h1")
        XCTAssertNil(session.handle(for: "peer-b"))
        XCTAssertTrue(session.isEmpty)
    }
}
