//
// WebrtcDirectDialledStreamsTests.swift
//
// A stream a page dialled to the WebRTC Direct listener (ADR 0039) is a row
// in the one stream table: PeerStreamSession and PeerStreamLinks, driven here
// with string handles, and the caps in PeerStreamDialPolicy. A handle that
// starts with `dial` is a dialled stream, `out` a TCP stream this side opened,
// anything else an accepted TCP stream. Mirrors android's
// WebrtcDirectDialledStreamsTest, keep in sync.
//
// The verifier is PeerStreamSessionTests' stand-in: an "assertion" is a
// `peer-…` address in ASCII, zero-padded to the 96-byte floor. The manager's
// wiring (the socket, the listener's events into this table) needs a device
// (docs/bridges C9); its lines are pinned by
// `every_peer_stream_manager_keeps_the_newer_dialled_stream`.
//

import XCTest
@testable import OfflineProtocol

final class WebrtcDirectDialledStreamsTests: XCTestCase {

    private struct Refused: Error {}

    private final class Host: PeerStreamHost {
        var me: String
        var events: [String] = []
        var diagnostics: [String] = []

        init(_ me: String) { self.me = me }

        func peerStreamIdentityAssertion() throws -> Data { WebrtcDirectDialledStreamsTests.assertion(me) }
        func peerStreamLocalAddress() -> String? { me }
        func peerStreamVerify(_ assertion: Data) throws -> String {
            let name = assertion.prefix { $0 != 0 }
            guard let address = String(data: name, encoding: .ascii), address.hasPrefix("peer-") else {
                throw Refused()
            }
            return address
        }
        func peerStreamConnected(_ address: String) { events.append("connected:\(address)") }
        func peerStreamReceived(_ address: String, _ body: Data) {
            events.append("message:\(address):\(String(decoding: body, as: UTF8.self))")
        }
        func peerStreamLost(_ address: String) { events.append("lost:\(address)") }
        func peerStreamDiagnostic(_ level: String, _ message: String, _ context: [String: Any]) {
            diagnostics.append("\(message) \(context)")
        }
    }

    static func assertion(_ address: String) -> Data {
        var data = Data(address.utf8)
        data.append(Data(count: PeerStreamFraming.preambleMinBytes - data.count))
        return data
    }

    private var host: Host!
    private var sent: [String] = []
    private var disconnected: [String] = []
    /// What each `admits` call was handed, `-` for nil.
    private var admitted: [String] = []
    private var session: PeerStreamSession<String>!

    override func setUp() {
        super.setUp()
        // The higher address of the pair by default: under the lower-address
        // rule alone an accepted stream would then take a held address over,
        // so a test that sees it refused sees the dialled rule.
        host = Host("peer-z")
        sent = []
        disconnected = []
        admitted = []
        session = PeerStreamSession<String>(
            host: host,
            preambleTimeout: 10,
            send: { [unowned self] _, handle in self.sent.append(handle) },
            disconnect: { [unowned self] handle in self.disconnected.append(handle) },
            schedule: { _, _ in },
            isOutbound: { $0.hasPrefix("out") },
            isDialled: { $0.hasPrefix("dial") }
        )
    }

    @discardableResult
    private func dial(_ handle: String, as address: String, admit: Bool = true) -> Bool {
        session.dialProved(handle, address: address) { replacing in
            self.admitted.append(replacing ?? "-")
            return admit
        }
    }

    /// A TCP stream connects and proves `address` with its preamble.
    private func prove(_ handle: String, as address: String) {
        session.connected(handle)
        session.received(PeerStreamFraming.frame(Self.assertion(address))!, from: handle)
    }

    // MARK: - One stream

    func testADialledStreamIsAnnouncedOnceCarriesBothWaysAndIsLostOnce() {
        XCTAssertTrue(dial("dial1", as: "peer-b"))
        XCTAssertEqual(host.events, ["connected:peer-b"])
        // The listener sent our preamble when the channel opened.
        XCTAssertEqual(sent, [])
        XCTAssertEqual(session.handle(for: "peer-b"), "dial1")
        XCTAssertEqual(admitted, ["-"])

        session.dialReceived(Data("hello".utf8), from: "dial1")
        XCTAssertEqual(host.events, ["connected:peer-b", "message:peer-b:hello"])

        session.ended("dial1")
        session.ended("dial1")
        XCTAssertEqual(host.events, ["connected:peer-b", "message:peer-b:hello", "lost:peer-b"])
        XCTAssertNil(session.handle(for: "peer-b"))
        XCTAssertEqual(disconnected, [])
    }

    func testABodyFromAStreamThatNeverProvedIsDropped() {
        session.dialReceived(Data("hello".utf8), from: "dial9")
        XCTAssertEqual(host.events, [])
    }

    // MARK: - Two streams for one address

    func testTheNewerOfTwoDialledStreamsWinsWithOneAnnouncementAndNoLoss() {
        // The lower address of the pair: the lower-address rule alone would
        // keep the older stream here, and refuse the page that reloaded.
        host.me = "peer-a"
        XCTAssertTrue(dial("dial1", as: "peer-b"))
        XCTAssertTrue(dial("dial2", as: "peer-b"))
        XCTAssertEqual(admitted, ["-", "dial1"], "the stream about to be superseded is not counted")
        XCTAssertEqual(disconnected, ["dial1"])
        XCTAssertEqual(session.handle(for: "peer-b"), "dial2")
        XCTAssertEqual(host.events, ["connected:peer-b"])

        // The superseded stream's late bodies and its end report nothing.
        session.dialReceived(Data("late".utf8), from: "dial1")
        session.ended("dial1")
        XCTAssertEqual(host.events, ["connected:peer-b"])

        session.dialReceived(Data("new".utf8), from: "dial2")
        session.ended("dial2")
        XCTAssertEqual(host.events, ["connected:peer-b", "message:peer-b:new", "lost:peer-b"])
    }

    func testATcpStreamKeepsItsAddressAgainstADial() {
        prove("in1", as: "peer-b")
        XCTAssertFalse(dial("dial1", as: "peer-b"))
        XCTAssertEqual(admitted, ["-"], "a TCP stream is never the one a dial replaces")
        XCTAssertEqual(disconnected, ["dial1"])
        XCTAssertEqual(session.handle(for: "peer-b"), "in1")
        XCTAssertEqual(host.events, ["connected:peer-b"])

        session.dialReceived(Data("refused".utf8), from: "dial1")
        session.ended("dial1")
        XCTAssertEqual(host.events, ["connected:peer-b"])
    }

    func testADialKeepsItsAddressAgainstATcpStreamOfEitherDirection() {
        XCTAssertTrue(dial("dial1", as: "peer-b"))
        // Accepted, on the higher address: the lower-address rule would let it in.
        prove("in1", as: "peer-b")
        // Dialled, on the lower address: the lower-address rule would let it in.
        host.me = "peer-a"
        prove("out1", as: "peer-b")
        XCTAssertEqual(disconnected, ["in1", "out1"])
        XCTAssertEqual(session.handle(for: "peer-b"), "dial1")
        XCTAssertEqual(host.events, ["connected:peer-b"])
        session.ended("in1")
        session.ended("out1")
        XCTAssertEqual(host.events, ["connected:peer-b"])
    }

    func testOnceADialledStreamEndsTheAddressFollowsTheTcpRuleAgain() {
        XCTAssertTrue(dial("dial1", as: "peer-b"))
        session.ended("dial1")
        prove("in1", as: "peer-b")
        prove("in2", as: "peer-b")
        XCTAssertEqual(disconnected, ["in1"], "of two accepted streams on the higher address, the newer wins")
        XCTAssertEqual(session.handle(for: "peer-b"), "in2")
    }

    // MARK: - Refusals

    func testADialProvingThisDevicesOwnAddressIsRefusedBeforeTheCaps() {
        XCTAssertFalse(dial("dial1", as: "peer-z"))
        XCTAssertEqual(admitted, [])
        XCTAssertEqual(disconnected, ["dial1"])
        XCTAssertEqual(host.events, [])
        XCTAssertNil(session.handle(for: "peer-z"))
    }

    func testADialPastACapIsClosedAndAnnouncesNothing() {
        XCTAssertFalse(dial("dial1", as: "peer-b", admit: false))
        XCTAssertEqual(disconnected, ["dial1"])
        XCTAssertNil(session.handle(for: "peer-b"))
        session.dialReceived(Data("x".utf8), from: "dial1")
        session.ended("dial1")
        XCTAssertEqual(host.events, [])
    }

    func testEndAllReportsEachAddressOnceWhicheverKindHoldsIt() {
        host.me = "peer-a"
        XCTAssertTrue(dial("dial1", as: "peer-b"))
        XCTAssertTrue(dial("dial2", as: "peer-b"))
        prove("in1", as: "peer-c")
        session.endAll()
        session.ended("dial1")
        session.ended("dial2")
        XCTAssertEqual(host.events.filter { $0.hasPrefix("lost:") }.sorted(), ["lost:peer-b", "lost:peer-c"])
    }

    // MARK: - PeerStreamLinks

    func testLinksOrderADialledStreamApartFromTheLowerAddressRule() {
        let links = PeerStreamLinks<String>()
        // This side is the lower address, so the TCP rule keeps an outbound
        // stream and refuses an inbound one.
        let me = "peer-a"
        XCTAssertTrue(links.announce("dial1", address: "peer-b", outbound: false, localAddress: me, dialled: true).firstForAddress)
        let newer = links.announce("dial2", address: "peer-b", outbound: false, localAddress: me, dialled: true)
        XCTAssertEqual(newer, PeerStreamLinks<String>.Announcement(firstForAddress: false, superseded: "dial1"))
        let tcp = links.announce("out1", address: "peer-b", outbound: true, localAddress: me)
        XCTAssertTrue(tcp.refused, "a TCP stream of the winning kind does not take a dialled stream's address")
        XCTAssertEqual(links.remove("dial2"), "peer-b")

        XCTAssertTrue(links.announce("out1", address: "peer-b", outbound: true, localAddress: me).firstForAddress)
        let dial = links.announce("dial3", address: "peer-b", outbound: false, localAddress: me, dialled: true)
        XCTAssertTrue(dial.refused, "a dial does not take a TCP stream's address")
        XCTAssertEqual(links.handle(for: "peer-b"), "out1")
    }

    // MARK: - The caps

    func testOnePageAddressHoldsAtMostItsShareAndAReloadAtTheCapIsAdmitted() {
        let page = "192.168.1.20"
        let four = [String?](repeating: page, count: PeerStreamDialPolicy.maxInboundPerHost)
        XCTAssertFalse(PeerStreamDialPolicy.admitsDialled(from: page, inboundFrom: four, open: 4, replacing: nil))
        XCTAssertTrue(PeerStreamDialPolicy.admitsDialled(from: page, inboundFrom: four, open: 4, replacing: page))
        XCTAssertTrue(PeerStreamDialPolicy.admitsDialled(from: "192.168.1.21", inboundFrom: four, open: 4, replacing: nil))
        // A host the replaced stream does not come from frees nothing.
        XCTAssertFalse(PeerStreamDialPolicy.admitsDialled(from: page, inboundFrom: four, open: 4, replacing: "192.168.1.99"))
    }

    func testDialsCountAgainstTheInboundShareAndTheStreamLimit() {
        let twelve: [String?] = (0..<PeerStreamDialPolicy.maxInbound).map { "10.0.0.\($0)" }
        XCTAssertFalse(PeerStreamDialPolicy.admitsDialled(from: "10.0.1.1", inboundFrom: twelve, open: 12, replacing: nil))
        XCTAssertTrue(PeerStreamDialPolicy.admitsDialled(from: "10.0.1.1", inboundFrom: twelve, open: 12, replacing: "10.0.0.3"))
        let three: [String?] = ["10.0.0.1", "10.0.0.2", "10.0.0.3"]
        XCTAssertTrue(PeerStreamDialPolicy.admitsDialled(from: "10.0.1.1", inboundFrom: three, open: 15, replacing: nil))
        XCTAssertFalse(PeerStreamDialPolicy.admitsDialled(from: "10.0.1.1", inboundFrom: three, open: 16, replacing: nil))
        XCTAssertTrue(PeerStreamDialPolicy.admitsDialled(from: "10.0.1.1", inboundFrom: three, open: 16, replacing: "10.0.0.2"))
    }

    func testTcpStreamsAndDialsFromOneAddressShareItsCap() {
        // Three accepted TCP sockets from the page's machine, then a dial.
        let host = "192.168.1.20"
        let held: [String?] = [host, host, host, nil]
        XCTAssertTrue(PeerStreamDialPolicy.admitsDialled(from: host, inboundFrom: held, open: 4, replacing: nil))
        XCTAssertFalse(PeerStreamDialPolicy.admitsDialled(from: host, inboundFrom: held + [host], open: 5, replacing: nil))
    }
}
