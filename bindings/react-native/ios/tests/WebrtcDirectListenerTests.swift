//
// WebrtcDirectListenerTests.swift
//
// Drives WebrtcDirectListener with a fake engine over real loopback UDP
// sockets: the socket, the pump, the clock, the close and the code, without
// the native library. The engine's own listener (ICE, DTLS, the channel) is
// tested in Rust; a page dialling a phone needs a device (docs/bridges C9).
// Mirrors android's WebrtcDirectListenerTest, keep in sync.
//

import Darwin
import XCTest
@testable import OfflineProtocol

final class WebrtcDirectListenerTests: XCTestCase {

    private struct Refused: Error {}

    /// The engine's side, scripted. Every method runs on the listener's
    /// queue; the test reads it through `queue.sync`.
    private final class FakeEngine: WebrtcDirectEngine {
        var code = WebrtcDirectDialCode(
            fragment: "op-direct=1&h=127.0.0.1&p=1&c=x&u=ufrag&w=SECRETPASSWORDSECRETPASSWORD&a=off1x",
            host: "127.0.0.1", port: 1, address: "off1x")
        var opened: [(ip: String, port: UInt16)] = []
        var received: [(ip: String, port: UInt16, data: Data)] = []
        var datagrams: [WebrtcDirectListener.Datagram] = []
        var events: [WebrtcDirectListener.Event] = []
        var proved: Set<UInt64> = []
        var sends: [(stream: UInt64, body: Data)] = []
        var refuseSends = false
        var closedStreams: [UInt64] = []
        var closes = 0
        var ticks = 0
        var nextTick: UInt32?
        var onReceive: ((String, UInt16, Data) -> Void)?

        func open(localIp: String, localPort: UInt16) throws -> WebrtcDirectDialCode {
            opened.append((localIp, localPort))
            return code
        }

        func close() {
            closes += 1
            // A `closed` for every proved stream, and something the close
            // queued to send.
            for stream in proved.sorted() { events.append(.closed(stream: stream)) }
            proved = []
        }

        func datagramReceived(sourceIp: String, sourcePort: UInt16, data: Data) throws {
            received.append((sourceIp, sourcePort, data))
            onReceive?(sourceIp, sourcePort, data)
        }

        func nextDatagram() -> WebrtcDirectListener.Datagram? {
            datagrams.isEmpty ? nil : datagrams.removeFirst()
        }

        func nextEvent() -> WebrtcDirectListener.Event? {
            guard !events.isEmpty else { return nil }
            let event = events.removeFirst()
            if case let .proved(stream, _, _, _) = event { proved.insert(stream) }
            return event
        }

        func send(stream: UInt64, body: Data) throws {
            if refuseSends {
                // Over the bound: refused, and the stream closes.
                proved.remove(stream)
                events.append(.closed(stream: stream))
                throw Refused()
            }
            sends.append((stream, body))
        }

        func closeStream(_ stream: UInt64) {
            closedStreams.append(stream)
            if proved.remove(stream) != nil { events.append(.closed(stream: stream)) }
        }

        func tick() -> UInt32? {
            ticks += 1
            return nextTick
        }
    }

    private final class Sink: WebrtcDirectSink {
        var seen: [String] = []
        var depth = 0
        var deepest = 0
        /// Called inside the handler for a `proved`, as the stream table's
        /// refusal or supersede would.
        var onProved: ((WebrtcDirectListener, UInt64) -> Void)?

        func webrtcDirectProved(_ listener: WebrtcDirectListener, stream: UInt64, address: String, remoteIp: String) {
            enter()
            defer { depth -= 1 }
            seen.append("proved:\(stream):\(address):\(remoteIp)")
            onProved?(listener, stream)
        }

        func webrtcDirectBody(_ listener: WebrtcDirectListener, stream: UInt64, data: Data) {
            enter()
            defer { depth -= 1 }
            seen.append("body:\(stream):\(String(decoding: data, as: UTF8.self))")
        }

        func webrtcDirectClosed(_ listener: WebrtcDirectListener, stream: UInt64) {
            enter()
            defer { depth -= 1 }
            seen.append("closed:\(stream)")
        }

        private func enter() {
            depth += 1
            deepest = max(deepest, depth)
        }
    }

    private let queue = DispatchQueue(label: "WebrtcDirectListenerTests")
    private var engine: FakeEngine!
    private var sink: Sink!
    private var diagnostics: [String] = []
    private var listener: WebrtcDirectListener!
    private var client: Int32 = -1
    /// Every reason the listener reported to `onClosed`, with whether it was
    /// still open then and what the sink had seen by then.
    private var reported: [(reason: String, open: Bool, seen: [String])] = []

    override func setUp() {
        super.setUp()
        engine = FakeEngine()
        sink = Sink()
        diagnostics = []
        reported = []
        listener = WebrtcDirectListener(
            engine: engine, sink: sink, queue: queue,
            diagnostic: { [unowned self] level, message, context in
                self.diagnostics.append("\(level) \(message) \(context)")
            },
            onClosed: { [unowned self] reason in
                self.reported.append((reason, self.listener.isOpen, self.sink.seen))
            })
        client = -1
    }

    override func tearDown() {
        queue.sync { listener.close() }
        if client >= 0 { Darwin.close(client) }
        super.tearDown()
    }

    // MARK: - Helpers

    /// Opens the listener on a loopback socket, on its queue.
    @discardableResult
    private func open() throws -> WebrtcDirectDialCode {
        let socket = try WebrtcDirectListener.bindSocket(ip: "127.0.0.1", interface: nil)
        return try queue.sync { try listener.open(socket: socket) }
    }

    /// The port the listener's engine was opened on.
    private var listenerPort: UInt16 { queue.sync { engine.opened.last?.port ?? 0 } }

    /// A loopback UDP socket for the page's side, with a receive timeout.
    private func makeClient() throws -> (socket: Int32, port: UInt16) {
        let socket = try WebrtcDirectListener.bindSocket(ip: "127.0.0.1", interface: nil)
        // Blocking, with a deadline, for the test's reads.
        _ = fcntl(socket, F_SETFL, fcntl(socket, F_GETFL, 0) & ~O_NONBLOCK)
        var timeout = timeval(tv_sec: 2, tv_usec: 0)
        setsockopt(socket, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        var local = sockaddr_in()
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        _ = withUnsafeMutablePointer(to: &local) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(socket, $0, &length) }
        }
        client = socket
        return (socket, UInt16(bigEndian: local.sin_port))
    }

    private func sendTo(_ socket: Int32, port: UInt16, _ text: String) {
        var to = sockaddr_in()
        to.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        to.sin_family = sa_family_t(AF_INET)
        to.sin_port = port.bigEndian
        inet_pton(AF_INET, "127.0.0.1", &to.sin_addr)
        let data = Data(text.utf8)
        _ = data.withUnsafeBytes { raw in
            withUnsafePointer(to: &to) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    sendto(socket, raw.baseAddress, raw.count, 0, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
        }
    }

    /// One datagram on `socket`, or nil at its receive timeout.
    private func receive(_ socket: Int32) -> String? {
        var buffer = [UInt8](repeating: 0, count: 2048)
        let count = recv(socket, &buffer, buffer.count, 0)
        guard count >= 0 else { return nil }
        return String(decoding: buffer[0..<count], as: UTF8.self)
    }

    private func waitUntil(_ what: String, timeout: TimeInterval = 2, _ condition: () -> Bool) {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() && Date() < deadline { usleep(5_000) }
        XCTAssertTrue(condition(), "timed out waiting for \(what)")
    }

    // MARK: - The socket

    func testASocketBoundToNoOneAddressIsRefusedBeforeTheEngineIsAsked() throws {
        XCTAssertThrowsError(try WebrtcDirectListener.bindSocket(ip: "0.0.0.0", interface: nil))
        XCTAssertThrowsError(try WebrtcDirectListener.bindSocket(ip: "not-an-address", interface: nil))

        let unbound = Darwin.socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        XCTAssertGreaterThanOrEqual(unbound, 0)
        XCTAssertThrowsError(try queue.sync { try listener.open(socket: unbound) })
        XCTAssertTrue(queue.sync { engine.opened.isEmpty })
        XCTAssertFalse(queue.sync { listener.isOpen })
        // The listener closed the socket it refused.
        XCTAssertEqual(fcntl(unbound, F_GETFD), -1)
    }

    func testTheEngineIsOpenedOnTheSocketsOwnAddressAndPort() throws {
        try open()
        let opened = queue.sync { engine.opened }
        XCTAssertEqual(opened.count, 1)
        XCTAssertEqual(opened.first?.ip, "127.0.0.1")
        XCTAssertNotEqual(opened.first?.port, 0)
        XCTAssertTrue(queue.sync { listener.isOpen })
        XCTAssertEqual(queue.sync { listener.boundIp }, "127.0.0.1")
    }

    func testEveryDatagramReachesTheEngineAndEveryQueuedDatagramIsSent() throws {
        try open()
        let page = try makeClient()
        queue.sync {
            engine.onReceive = { [unowned self] ip, port, data in
                self.engine.datagrams.append(WebrtcDirectListener.Datagram(
                    destinationIp: ip, destinationPort: port,
                    data: Data("answer to ".utf8) + data))
            }
        }
        sendTo(page.socket, port: listenerPort, "check one")
        XCTAssertEqual(receive(page.socket), "answer to check one")
        sendTo(page.socket, port: listenerPort, "check two")
        XCTAssertEqual(receive(page.socket), "answer to check two")

        let received = queue.sync { engine.received }
        XCTAssertEqual(received.map { String(decoding: $0.data, as: UTF8.self) }, ["check one", "check two"])
        XCTAssertEqual(received.first?.ip, "127.0.0.1")
        XCTAssertEqual(received.first?.port, page.port)
    }

    func testARefusedSendIsReportedOnceAndTheListenerStaysOpen() throws {
        // A broadcast without SO_BROADCAST is refused by the socket, as a
        // local network denial refuses every answer to a page.
        queue.sync {
            engine.datagrams = [
                WebrtcDirectListener.Datagram(destinationIp: "255.255.255.255", destinationPort: 9, data: Data("a".utf8)),
                WebrtcDirectListener.Datagram(destinationIp: "255.255.255.255", destinationPort: 9, data: Data("b".utf8)),
            ]
        }
        try open()
        let reported = diagnostics.filter { $0.contains("NSLocalNetworkUsageDescription") }
        XCTAssertEqual(reported.count, 1, "\(diagnostics)")
        XCTAssertTrue(queue.sync { listener.isOpen })
    }

    // MARK: - The pump

    func testTheClockIsReArmedFromEachTick() throws {
        queue.sync { engine.nextTick = 20 }
        try open()
        waitUntil("several ticks") { queue.sync { engine.ticks } >= 4 }
        // On the queue no tick runs, so exactly one deadline is armed here:
        // it fires once more, returns nil, and that disarms the clock. Counted
        // from this point rather than after a fixed sleep, because a loaded
        // runner can fire a 20 ms deadline much later than 20 ms.
        let armed = queue.sync { () -> Int in
            engine.nextTick = nil
            return engine.ticks
        }
        waitUntil("the armed tick") { queue.sync { engine.ticks } == armed + 1 }
        usleep(150_000)
        XCTAssertEqual(queue.sync { engine.ticks }, armed + 1)
    }

    func testEventsReachTheSinkInOrderWithTheirStreamAndAddress() throws {
        queue.sync {
            engine.events = [
                .proved(stream: 1, address: "off1a", remoteIp: "192.168.1.20", remotePort: 5000),
                .body(stream: 1, data: Data("hi".utf8)),
            ]
        }
        try open()
        XCTAssertEqual(queue.sync { sink.seen }, ["proved:1:off1a:192.168.1.20", "body:1:hi"])
    }

    func testACallBackInsideAHandlerIsHandledAfterTheEventThatCausedIt() throws {
        queue.sync {
            engine.events = [
                .proved(stream: 1, address: "off1a", remoteIp: "192.168.1.20", remotePort: 5000),
                .proved(stream: 2, address: "off1a", remoteIp: "192.168.1.20", remotePort: 5001),
            ]
            // The second supersedes the first, as the stream table would.
            sink.onProved = { listener, stream in
                if stream == 2 { listener.closeStream(1) }
            }
        }
        try open()
        XCTAssertEqual(queue.sync { sink.seen }, ["proved:1:off1a:192.168.1.20", "proved:2:off1a:192.168.1.20", "closed:1"])
        XCTAssertEqual(queue.sync { sink.deepest }, 1, "an event was handled inside another's handler")
        XCTAssertEqual(queue.sync { engine.closedStreams }, [1])
    }

    func testASendTheEngineRefusesStillPumpsItsClose() throws {
        queue.sync { engine.events = [.proved(stream: 7, address: "off1a", remoteIp: "10.0.0.2", remotePort: 1)] }
        try open()
        try queue.sync { try listener.send(stream: 7, body: Data("ok".utf8)) }
        XCTAssertEqual(queue.sync { engine.sends.map { $0.stream } }, [7])
        queue.sync { engine.refuseSends = true }
        XCTAssertThrowsError(try queue.sync { try listener.send(stream: 7, body: Data("over".utf8)) })
        XCTAssertEqual(queue.sync { sink.seen.last }, "closed:7")
    }

    // MARK: - The close

    func testClosingReportsEachProvedStreamOnceSendsWhatItQueuedThenClosesTheSocket() throws {
        queue.sync {
            engine.events = [
                .proved(stream: 1, address: "off1a", remoteIp: "127.0.0.1", remotePort: 1),
                .proved(stream: 2, address: "off1b", remoteIp: "127.0.0.1", remotePort: 2),
            ]
        }
        try open()
        let page = try makeClient()
        let port = listenerPort
        queue.sync {
            engine.datagrams = [WebrtcDirectListener.Datagram(
                destinationIp: "127.0.0.1", destinationPort: page.port, data: Data("goodbye".utf8))]
            listener.close()
            listener.close()
        }
        XCTAssertEqual(receive(page.socket), "goodbye", "what the close queued went out before the socket closed")
        XCTAssertEqual(queue.sync { engine.closes }, 1)
        XCTAssertEqual(queue.sync { sink.seen.filter { $0.hasPrefix("closed:") } }, ["closed:1", "closed:2"])
        XCTAssertFalse(queue.sync { listener.isOpen })

        // Nothing reaches the engine any more, and nothing is sent.
        sendTo(page.socket, port: port, "late check")
        usleep(100_000)
        XCTAssertTrue(queue.sync { engine.received.isEmpty })
        XCTAssertThrowsError(try queue.sync { try listener.send(stream: 1, body: Data("x".utf8)) })
        queue.sync { listener.closeStream(1) }
        XCTAssertEqual(queue.sync { engine.closedStreams }, [])
    }

    // MARK: - Why the bridge closed it

    /// The application hears the bridge's own close (the network went, the
    /// application left the foreground, the transport stopped) and stops
    /// showing a code whose listener is gone. Once, after the streams ended.
    func testACloseThatNamesAReasonReportsItOnceAfterTheListenerClosed() throws {
        queue.sync {
            engine.events = [.proved(stream: 1, address: "off1a", remoteIp: "127.0.0.1", remotePort: 1)]
        }
        try open()
        queue.sync {
            listener.close(reason: WebrtcDirectListener.closedNetwork)
            listener.close(reason: WebrtcDirectListener.closedSocket)
        }
        let reports = queue.sync { reported }
        XCTAssertEqual(reports.map(\.reason), ["network"])
        XCTAssertEqual(reports.first?.open, false, "reported while still open")
        XCTAssertEqual(reports.first?.seen.last, "closed:1", "reported before its streams ended")
    }

    /// Its own `closeWebrtcDirect`, or an open replacing the listener, names
    /// no reason: the application already knows.
    func testACloseTheApplicationAskedForReportsNothing() throws {
        try open()
        queue.sync {
            listener.close()
            listener.close(reason: WebrtcDirectListener.closedStopped)
        }
        XCTAssertTrue(queue.sync { reported.isEmpty })
    }

    /// The engine replaced it (an open of another listener succeeded): its
    /// socket stops, but the engine's listener, now the new one, is never
    /// closed from here, and nothing is reported.
    func testARetiredListenerLeavesTheEngineOpenAndReportsNothing() throws {
        try open()
        queue.sync {
            listener.retire()
            listener.close(reason: WebrtcDirectListener.closedNetwork)
        }
        XCTAssertEqual(queue.sync { engine.closes }, 0)
        XCTAssertTrue(queue.sync { reported.isEmpty })
        XCTAssertThrowsError(try queue.sync { try listener.send(stream: 1, body: Data("x".utf8)) })
    }

    /// Never opened: there is no listener the application was shown.
    func testAListenerNeverOpenedReportsNothing() {
        queue.sync { listener.close(reason: WebrtcDirectListener.closedStopped) }
        XCTAssertTrue(queue.sync { reported.isEmpty })
    }

    /// Pinned literally: apps switch on these (`src/types.ts`
    /// WebrtcDirectClosedEvent and WebrtcDirectClosedReason).
    func testTheClosedEventCarriesTheTagAppsMatchOnAndItsReason() throws {
        XCTAssertEqual(WebrtcDirectListener.closedEventType, "webrtc_direct_closed")
        XCTAssertEqual(
            [WebrtcDirectListener.closedBackground, WebrtcDirectListener.closedPaused,
             WebrtcDirectListener.closedNetwork, WebrtcDirectListener.closedSocket,
             WebrtcDirectListener.closedStopped],
            ["background", "paused", "network", "socket", "stopped"])
        let json = WebrtcDirectListener.closedEventJson(reason: "background")
        let event = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: String])
        XCTAssertEqual(event, ["type": "webrtc_direct_closed", "reason": "background"])
    }

    // MARK: - The code

    func testTheCodeIsReturnedAndNeverReachesADiagnosticOrADescription() throws {
        let code = try open()
        queue.sync { listener.close() }
        XCTAssertTrue(code.fragment.contains("SECRETPASSWORD"))

        var dumped = ""
        dump(code, to: &dumped)
        for text in [
            "\(code)", String(describing: code), String(reflecting: code), dumped,
            "\(Mirror(reflecting: code).children.map { "\($0.label ?? ""):\($0.value)" })",
        ] + diagnostics {
            XCTAssertFalse(text.contains("SECRET"), "the code leaked into: \(text)")
            XCTAssertFalse(text.contains("op-direct"), "the code leaked into: \(text)")
        }
        XCTAssertTrue(diagnostics.contains { $0.contains("WebRTC Direct listener open") && $0.contains("off1x") })
        XCTAssertTrue("\(code)".contains("127.0.0.1"))
    }

    // MARK: - Addresses

    func testListenAddressTakesAPrivateIPv4NeverLinkLocalPublicLoopbackOrIPv6() {
        XCTAssertEqual(WebrtcDirectListener.listenAddress(["8.8.8.8", "fe80::1", "169.254.3.4", "192.168.1.20"]), "192.168.1.20")
        XCTAssertEqual(WebrtcDirectListener.listenAddress(["10.1.2.3"]), "10.1.2.3")
        XCTAssertEqual(WebrtcDirectListener.listenAddress(["172.15.0.1", "172.16.0.1"]), "172.16.0.1")
        XCTAssertNil(WebrtcDirectListener.listenAddress(["172.32.0.1"]))
        // The engine refuses an IPv4 link-local host candidate.
        XCTAssertNil(WebrtcDirectListener.listenAddress(["8.8.8.8", "169.254.3.4"]))
        XCTAssertNil(WebrtcDirectListener.listenAddress(["127.0.0.1", "8.8.8.8", "::1", "fd00::1"]))
        XCTAssertNil(WebrtcDirectListener.listenAddress([]))
    }

    func testTheInterfaceAddressesAreRead() {
        XCTAssertTrue(WebrtcDirectListener.ipv4Addresses(ofInterface: "lo0").contains("127.0.0.1"))
        XCTAssertEqual(WebrtcDirectListener.ipv4Addresses(ofInterface: "no-such-interface"), [])
    }
}
