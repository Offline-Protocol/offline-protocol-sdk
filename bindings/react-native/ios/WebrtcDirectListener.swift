//
// WebrtcDirectListener.swift
// OfflineProtocol
//
// The platform's half of a WebRTC Direct listener (ADR 0039): one UDP
// socket, the clock, and the pump between them and the engine's sans-I/O
// listener (`webrtc_direct_*` in the UDL, whose comments are the contract).
//
// Foundation and Darwin only, behind `WebrtcDirectEngine`, so that
// WebrtcDirectListenerTests drives it with a fake engine over loopback
// sockets without the native library. WifiDirectManager owns the one
// listener, adapts the generated methods to `WebrtcDirectEngine`, and is the
// `WebrtcDirectSink` whose stream table decides what the core hears. Mirrors
// android's WebrtcDirectListener.kt, keep in sync.
//

import Darwin
import Foundation

/// A WebRTC Direct listener's dial code. `fragment` is what a page dials:
/// the application shows it after `#` in a URL or as a QR code, so it never
/// reaches a web server. `host`, `port` and `address` are the listener's
/// socket and this device's address, for display.
///
/// `fragment` carries the listener's ICE password and admits whoever holds it
/// while the listener is open (R35), so this type's description, debug
/// description and mirror all leave it out: a stray interpolation, a
/// `print`, a `dump` or a diagnostic that takes the whole value cannot leak
/// it. The generated `WebrtcDirectCode` prints every field, which is why it
/// never leaves the manager's adapter.
public struct WebrtcDirectDialCode: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public let fragment: String
    public let host: String
    public let port: UInt16
    public let address: String

    init(fragment: String, host: String, port: UInt16, address: String) {
        self.fragment = fragment
        self.host = host
        self.port = port
        self.address = address
    }

    public var description: String {
        "WebrtcDirectDialCode(host: \(host), port: \(port), address: \(address))"
    }

    public var debugDescription: String { description }

    public var customMirror: Mirror {
        Mirror(self, children: ["host": host, "port": port, "address": address], displayStyle: .struct)
    }
}

/// Why the listener or its socket refused.
struct WebrtcDirectListenerError: LocalizedError, Equatable {
    let message: String

    var errorDescription: String? { message }
}

/// The engine's `webrtc_direct_*` methods, in this file's types.
/// `WifiDirectManager` adapts the generated ones; tests supply a fake.
protocol WebrtcDirectEngine {
    func open(localIp: String, localPort: UInt16) throws -> WebrtcDirectDialCode
    func close()
    func datagramReceived(sourceIp: String, sourcePort: UInt16, data: Data) throws
    func nextDatagram() -> WebrtcDirectListener.Datagram?
    func nextEvent() -> WebrtcDirectListener.Event?
    func send(stream: UInt64, body: Data) throws
    func closeStream(_ stream: UInt64)
    /// Milliseconds until the next tick is due, or nil when nothing is timed.
    func tick() -> UInt32?
}

/// What the listener's events mean to the stream table. Called on the
/// listener's queue, inside its pump: a handler may call back into the
/// listener (a refusal closing a stream, a supersede), and that call only
/// marks the pump to go round again.
protocol WebrtcDirectSink: AnyObject {
    func webrtcDirectProved(
        _ listener: WebrtcDirectListener, stream: UInt64, address: String, remoteIp: String
    )
    func webrtcDirectBody(_ listener: WebrtcDirectListener, stream: UInt64, data: Data)
    func webrtcDirectClosed(_ listener: WebrtcDirectListener, stream: UInt64)
}

/// One UDP socket and the pump that holds the UDL's contract for it:
///
/// - every datagram the socket receives goes to the engine, unfiltered,
///   because the engine answers only an authenticated check;
/// - after every call into the engine, every datagram it queued is sent from
///   this socket and every event it queued is handed to the sink;
/// - `tick` runs again within the milliseconds it returned, and after any
///   other call, since a datagram or a send can bring the next deadline
///   forward.
///
/// ## A BSD socket, not Network framework
///
/// The engine names a destination for every datagram it queues: the answer
/// to a page's check, and every DTLS and SCTP packet after it, to whichever
/// page sent it, from the one port the code names. A datagram socket with a
/// read source does exactly that: one socket, `recvfrom` from anyone,
/// `sendto` anywhere. An `NWListener` over UDP instead hands over one
/// `NWConnection` per remote, created on its own schedule, and has no way to
/// send to a remote it has not made a connection for; a second
/// `NWConnection` to that remote binds a port of its own unless local
/// endpoint reuse is forced, and an answer from any port but the code's is
/// not the candidate the page checked, so its ICE agent drops it and the
/// dial never completes. Local network privacy applies to both on iOS 14 and
/// later: the application needs `NSLocalNetworkUsageDescription`, and the
/// user's consent.
///
/// ## One queue
///
/// Every method is called on `queue`, the queue the read source and the
/// timer run on too (`WifiDirectManager`'s `linkQueue`, where its stream
/// table lives). So the engine's listener is never entered from two threads
/// at once and its events are handled in the order it queued them, and a
/// handler that calls back in (`closeStream`, `send`) runs on the thread
/// already pumping and only marks the pump to go round again, so events are
/// never handled inside one another.
///
/// The dial code is returned to the caller and never logged: the diagnostics
/// carry the host, port and address, never the fragment.
final class WebrtcDirectListener {
    /// A datagram the engine queued, for this socket to send.
    struct Datagram: Equatable {
        let destinationIp: String
        let destinationPort: UInt16
        let data: Data
    }

    /// What the engine reports. `address` on `proved` is the address the
    /// engine's verifier derived, never a claimed one; `remoteIp` is where the
    /// dial's first check came from, for the per-host cap, never an identity.
    enum Event: Equatable {
        case proved(stream: UInt64, address: String, remoteIp: String, remotePort: UInt16)
        case body(stream: UInt64, data: Data)
        case closed(stream: UInt64)
    }

    private let engine: WebrtcDirectEngine
    private weak var sink: WebrtcDirectSink?
    private let queue: DispatchQueue
    private let diagnostic: (_ level: String, _ message: String, _ context: [String: Any]) -> Void
    /// The application's report of a `close(reason:)` that names a reason
    /// (one of the `closed*` tokens): at most once, after the close. A close
    /// with no reason (the application's own, or an open replacing this
    /// listener) reports nothing, since the application asked for it.
    private let onClosed: (_ reason: String) -> Void
    /// The socket failed without a `close()` of ours (the interface went).
    private let onSocketLost: (WebrtcDirectListener) -> Void

    private var socket: Int32 = -1
    private var readSource: DispatchSourceRead?
    private var timer: DispatchSourceTimer?
    private var pumping = false
    private var again = false
    private var closed = false
    /// Whether a refused send was reported: once per listener, since a
    /// denial refuses every one.
    private var reportedRefusedSend = false
    /// One receive buffer, the UDP maximum, so that nothing larger is
    /// truncated into a different packet. A WebRTC datagram fits a path MTU.
    private var buffer = [UInt8](repeating: 0, count: 65_535)

    /// The code the engine returned, once open.
    private(set) var code: WebrtcDirectDialCode?
    /// The IPv4 address the socket is bound to, once open.
    private(set) var boundIp: String?

    var isOpen: Bool { !closed && code != nil }

    init(
        engine: WebrtcDirectEngine,
        sink: WebrtcDirectSink,
        queue: DispatchQueue,
        diagnostic: @escaping (_ level: String, _ message: String, _ context: [String: Any]) -> Void,
        onClosed: @escaping (_ reason: String) -> Void = { _ in },
        onSocketLost: @escaping (WebrtcDirectListener) -> Void = { _ in }
    ) {
        self.engine = engine
        self.sink = sink
        self.queue = queue
        self.diagnostic = diagnostic
        self.onClosed = onClosed
        self.onSocketLost = onSocketLost
    }

    deinit {
        // A listener dropped without close(): the sources stop, and the
        // read source's cancel closes the socket.
        timer?.cancel()
        if let readSource = readSource {
            readSource.cancel()
        } else if socket >= 0 {
            Darwin.close(socket)
        }
    }

    // MARK: - Why the bridge closed it

    // The `reason` of `webrtc_direct_closed` (`src/types.ts`
    // WebrtcDirectClosedReason, which Android's `hotspot` completes). Without
    // the event an application kept showing a code whose listener was gone,
    // and every page that dialled it timed out with nothing to say why.

    /// The application left the foreground.
    static let closedBackground = "background"
    /// The transport paused.
    static let closedPaused = "paused"
    /// The Wi-Fi network went, or lost the bound address.
    static let closedNetwork = "network"
    /// The socket failed without a close of ours.
    static let closedSocket = "socket"
    /// The transport stopped.
    static let closedStopped = "stopped"

    /// The bridge event tag apps match on (`src/types.ts` WebrtcDirectClosedEvent).
    static let closedEventType = "webrtc_direct_closed"

    /// The `webrtc_direct_closed` payload for `reason`, as event JSON.
    static func closedEventJson(reason: String) -> String {
        let payload: [String: Any] = ["type": closedEventType, "reason": reason]
        if let data = try? JSONSerialization.data(withJSONObject: payload, options: []),
           let json = String(data: data, encoding: .utf8) {
            return json
        }
        // Two strings always serialise, and a token needs no escaping.
        return "{\"type\":\"\(closedEventType)\",\"reason\":\"\(reason)\"}"
    }

    // MARK: - Open and close

    /// Opens the engine's listener on `socket`, which the caller bound to one
    /// IPv4 address (`bindSocket`), and starts receiving. A socket on the
    /// wildcard or with no port is refused before the engine is asked: a code
    /// names one address, and a socket on every interface would admit checks
    /// from networks the code was never shown on. On a throw the socket is
    /// closed and nothing is left open.
    func open(socket: Int32) throws -> WebrtcDirectDialCode {
        guard !closed, code == nil else {
            Darwin.close(socket)
            throw WebrtcDirectListenerError(message: "this WebRTC Direct listener was already opened")
        }
        guard let local = Self.localAddress(of: socket), local.ip != "0.0.0.0", local.port != 0 else {
            closed = true
            Darwin.close(socket)
            throw WebrtcDirectListenerError(message: "a WebRTC Direct listener needs a socket bound to one address")
        }
        let opened: WebrtcDirectDialCode
        do {
            // The socket's real address and port, never 0: the code names them.
            opened = try engine.open(localIp: local.ip, localPort: local.port)
        } catch {
            closed = true
            Darwin.close(socket)
            throw error
        }
        self.socket = socket
        code = opened
        boundIp = local.ip

        let read = DispatchSource.makeReadSource(fileDescriptor: socket, queue: queue)
        read.setEventHandler { [weak self] in self?.receive() }
        read.setCancelHandler { Darwin.close(socket) }
        readSource = read
        let clock = DispatchSource.makeTimerSource(queue: queue)
        clock.setEventHandler { [weak self] in self?.pump() }
        clock.schedule(deadline: .distantFuture)
        timer = clock
        clock.resume()
        read.resume()

        // The address and port, never the fragment.
        diagnostic("info", "WebRTC Direct listener open", [
            "host": opened.host,
            "port": Int(opened.port),
            "address": opened.address,
        ])
        pump()
        return opened
    }

    /// Closes the engine's listener, hands its last events (a `closed` for
    /// every proved stream) to the sink while the socket can still send what
    /// the close queued, then closes the socket. Idempotent: only the first
    /// close of an open listener reports, and only when it names a `reason`
    /// (see `onClosed`).
    func close(reason: String? = nil) {
        guard !closed else { return }
        guard code != nil else {
            // Never opened: the engine's listener, if any, is another's.
            closed = true
            return
        }
        engine.close()
        pump(tick: false)
        closed = true
        timer?.cancel()
        timer = nil
        readSource?.cancel()
        readSource = nil
        socket = -1
        diagnostic("info", "WebRTC Direct listener closed", [:])
        if let reason = reason { onClosed(reason) }
    }

    /// Stops this listener's socket and timer once the engine has replaced
    /// it (an open of another listener succeeded), without closing the
    /// engine's listener: that is the new one now, and a `close` here would
    /// close it. The engine dropped what this one still had to send and
    /// queued a `closed` for each of its proved streams, which the new
    /// listener hands to the sink. Reports nothing: the application asked
    /// for the replacement.
    func retire() {
        guard !closed else { return }
        closed = true
        guard code != nil else { return }
        timer?.cancel()
        timer = nil
        readSource?.cancel()
        readSource = nil
        socket = -1
        diagnostic("info", "WebRTC Direct listener closed", [:])
    }

    // MARK: - Streams

    /// One body on a proved stream; the engine frames it. Throws when the
    /// engine refuses it (a stream that ended, or more than its bound
    /// queued, which leaves the stream open).
    func send(stream: UInt64, body: Data) throws {
        guard !closed else {
            throw WebrtcDirectListenerError(message: "the WebRTC Direct listener is closed")
        }
        // The datagrams the send queued, and the `closed` for a stream it
        // ended, go out now, refused or not.
        defer { pump() }
        try engine.send(stream: stream, body: body)
    }

    /// Ends one stream. The engine reports it closed if it was proved, and
    /// by then the table has let it go, so the report announces nothing.
    func closeStream(_ stream: UInt64) {
        guard !closed else { return }
        engine.closeStream(stream)
        pump()
    }

    // MARK: - The pump

    /// Sends what the engine queued, hands its events on, fires its timers,
    /// then arms the clock from the tick's return.
    private func pump(tick: Bool = true) {
        if pumping {
            again = true
            return
        }
        guard !closed else { return }
        pumping = true
        defer { pumping = false }
        var delay: UInt32?
        repeat {
            again = false
            flush()
            delay = tick && !closed ? engine.tick() : nil
        } while flush() || again
        if tick { arm(delay) }
    }

    /// True when anything moved: a datagram sent or an event handled.
    @discardableResult
    private func flush() -> Bool {
        var moved = false
        while let datagram = engine.nextDatagram() {
            moved = true
            send(datagram)
        }
        while let event = engine.nextEvent() {
            moved = true
            switch event {
            case let .proved(stream, address, remoteIp, _):
                sink?.webrtcDirectProved(self, stream: stream, address: address, remoteIp: remoteIp)
            case let .body(stream, data):
                sink?.webrtcDirectBody(self, stream: stream, data: data)
            case let .closed(stream):
                sink?.webrtcDirectClosed(self, stream: stream)
            }
            // A handler's own calls (a close, a send) queue datagrams.
            while let datagram = engine.nextDatagram() {
                send(datagram)
            }
        }
        return moved
    }

    private func arm(_ delay: UInt32?) {
        guard let timer = timer, !closed else { return }
        if let delay = delay {
            // At least 1 ms: a tick that keeps answering 0 would otherwise
            // hold linkQueue, which carries the whole stream table, in a spin.
            timer.schedule(deadline: .now() + .milliseconds(Int(max(delay, 1))))
        } else {
            timer.schedule(deadline: .distantFuture)
        }
    }

    // MARK: - The socket

    private func receive() {
        guard !closed, socket >= 0 else { return }
        // A bounded batch per wake, so a flood cannot hold the queue the
        // whole stream table runs on; the read source fires again while the
        // socket has more.
        for _ in 0..<64 {
            var from = sockaddr_in()
            var length = socklen_t(MemoryLayout<sockaddr_in>.size)
            let socket = self.socket
            let count = buffer.withUnsafeMutableBytes { raw in
                withUnsafeMutablePointer(to: &from) { pointer in
                    pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                        recvfrom(socket, raw.baseAddress, raw.count, 0, $0, &length)
                    }
                }
            }
            if count < 0 {
                let error = errno
                switch error {
                case EAGAIN, EWOULDBLOCK:
                    return
                case EINTR, ECONNREFUSED, EHOSTUNREACH, ENETUNREACH:
                    // About one datagram, not the socket.
                    continue
                default:
                    diagnostic("warning", "WebRTC Direct socket lost", ["errno": Int(error)])
                    onSocketLost(self)
                    return
                }
            }
            guard from.sin_family == sa_family_t(AF_INET), let source = Self.string(from.sin_addr) else {
                continue
            }
            let data = Data(buffer[0..<count])
            do {
                try engine.datagramReceived(
                    sourceIp: source, sourcePort: UInt16(bigEndian: from.sin_port), data: data)
            } catch {
                // No listener (a close raced the socket): about one
                // datagram, not the socket.
            }
            pump()
            if closed { return }
        }
    }

    private func send(_ datagram: Datagram) {
        guard socket >= 0 else { return }
        var to = sockaddr_in()
        // The socket is IPv4; the engine names no other destination for it.
        guard inet_pton(AF_INET, datagram.destinationIp, &to.sin_addr) == 1 else { return }
        to.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        to.sin_family = sa_family_t(AF_INET)
        to.sin_port = datagram.destinationPort.bigEndian
        let socket = self.socket
        let sent = datagram.data.withUnsafeBytes { raw in
            withUnsafePointer(to: &to) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    sendto(socket, raw.baseAddress, raw.count, 0, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
        }
        // A failure is about one datagram (an unreachable page, a full
        // buffer): the page retransmits, and consent ends a silent stream.
        // Said once, because a local network denial refuses every send and
        // the page's dial then times out with nothing said on its side.
        if sent < 0, !reportedRefusedSend {
            let error = errno
            reportedRefusedSend = true
            diagnostic("warning", "WebRTC Direct could not answer a page: if local network access is denied, set NSLocalNetworkUsageDescription and allow it", [
                "errno": Int(error),
            ])
        }
    }

    // MARK: - Addresses

    /// A non-blocking UDP socket bound to `ip`, an IPv4 literal, on a fresh
    /// port, and to the interface named `interface` when one is named, so
    /// that nothing it sends follows the default route onto cellular. The
    /// wildcard is refused: its address cannot go in a code. The caller
    /// hands it to `open(socket:)`, which owns it from then on.
    static func bindSocket(ip: String, interface: String?) throws -> Int32 {
        var address = in_addr()
        guard inet_pton(AF_INET, ip, &address) == 1 else {
            throw WebrtcDirectListenerError(message: "\(ip) is not an IPv4 address")
        }
        guard address.s_addr != INADDR_ANY else {
            throw WebrtcDirectListenerError(message: "a WebRTC Direct listener never binds the wildcard")
        }
        let socket = Darwin.socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard socket >= 0 else {
            throw WebrtcDirectListenerError(message: "no UDP socket (errno \(errno))")
        }
        do {
            let flags = fcntl(socket, F_GETFL, 0)
            guard flags >= 0, fcntl(socket, F_SETFL, flags | O_NONBLOCK) == 0 else {
                throw WebrtcDirectListenerError(message: "the UDP socket cannot be non-blocking (errno \(errno))")
            }
            if let interface = interface {
                var index = if_nametoindex(interface)
                guard index != 0,
                      setsockopt(socket, IPPROTO_IP, IP_BOUND_IF, &index,
                                 socklen_t(MemoryLayout.size(ofValue: index))) == 0 else {
                    throw WebrtcDirectListenerError(message: "the UDP socket cannot be bound to \(interface) (errno \(errno))")
                }
            }
            var local = sockaddr_in()
            local.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
            local.sin_family = sa_family_t(AF_INET)
            local.sin_port = 0
            local.sin_addr = address
            let bound = withUnsafePointer(to: &local) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.bind(socket, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
            guard bound == 0 else {
                throw WebrtcDirectListenerError(message: "WebRTC Direct could not bind \(ip) (errno \(errno))")
            }
        } catch {
            Darwin.close(socket)
            throw error
        }
        return socket
    }

    /// The address a listener on an interface with `addresses` binds: its
    /// first private IPv4 address (RFC 1918), else nil. A page reaches IPv4
    /// on every network this runs on; an IPv6 link-local address would also
    /// need its zone, which no code carries. An IPv4 link-local address
    /// (169.254/16, a network with no DHCP) is never chosen although a code
    /// may name one: the engine's ICE agent refuses it as a host candidate,
    /// so choosing it only turned the clear "no private address" refusal
    /// into an opaque socket error.
    static func listenAddress(_ addresses: [String]) -> String? {
        let v4 = addresses.compactMap { text -> (String, [UInt8])? in
            var address = in_addr()
            guard inet_pton(AF_INET, text, &address) == 1 else { return nil }
            return (text, withUnsafeBytes(of: address.s_addr) { Array($0) })
        }
        let isPrivate: ([UInt8]) -> Bool = { b in
            b[0] == 10 || (b[0] == 172 && (16...31).contains(b[1])) || (b[0] == 192 && b[1] == 168)
        }
        return v4.first { isPrivate($0.1) }?.0
    }

    /// The IPv4 addresses the interface named `name` holds now.
    static func ipv4Addresses(ofInterface name: String) -> [String] {
        var head: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&head) == 0 else { return [] }
        defer { freeifaddrs(head) }
        var out: [String] = []
        var cursor = head
        while let entry = cursor {
            defer { cursor = entry.pointee.ifa_next }
            guard let raw = entry.pointee.ifa_addr,
                  raw.pointee.sa_family == sa_family_t(AF_INET),
                  String(cString: entry.pointee.ifa_name) == name else { continue }
            let address = raw.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { $0.pointee.sin_addr }
            if let text = string(address) { out.append(text) }
        }
        return out
    }

    private static func string(_ address: in_addr) -> String? {
        var address = address
        var text = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
        guard inet_ntop(AF_INET, &address, &text, socklen_t(text.count)) != nil else { return nil }
        return String(cString: text)
    }

    private static func localAddress(of socket: Int32) -> (ip: String, port: UInt16)? {
        var local = sockaddr_in()
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let read = withUnsafeMutablePointer(to: &local) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                getsockname(socket, $0, &length)
            }
        }
        guard read == 0, local.sin_family == sa_family_t(AF_INET),
              let ip = string(local.sin_addr) else { return nil }
        return (ip, UInt16(bigEndian: local.sin_port))
    }
}
