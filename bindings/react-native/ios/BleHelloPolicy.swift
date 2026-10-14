import Foundation

/// Whether the central writes its hello on a link, and what it writes.
///
/// The Bluetooth LE framing chapter's optional Hello characteristic
/// (`6E400006-...`) is how a central says who it is on the link it opened. A
/// peripheral that receives Message writes from a central it cannot name
/// otherwise has two choices, both bad: dial the central back to read its
/// identity (the iOS and Android peripherals do, at a cost of seconds to tens
/// of seconds per new link), or attribute the writes to the central's
/// connection id. The Python service's peripheral does the second, and the core
/// then refuses our key package, whose sender must be the link's identity, so
/// an iPhone could find and verify a Python box and never start a session
/// with it through that link.
///
/// The value is the identity assertion this device serves on its own Identity
/// characteristic. It is written once per connection, with response, and only
/// when it fits one write. Anything else (a peer with no Hello, an assertion
/// not built yet, one out of the chapter's bounds) writes nothing, and the
/// link is resolved as it was before Hello existed: a hello is never a reason
/// to drop a link.
enum BleHelloPolicy {
    /// The chapter's bounds on a hello value. A peripheral refuses anything
    /// outside them before verifying, so writing one would only spend a write.
    static let minLength = 96
    static let maxLength = 512

    /// The bytes to write, or nil to write none.
    ///
    /// - Parameters:
    ///   - identity: the encoded assertion this device serves, if it has one.
    ///   - peerServesHello: whether the chosen service instance has Hello.
    ///   - alreadyWritten: whether this connection already wrote its hello.
    ///     Characteristic discovery replays on a live link, and the hello is
    ///     per connection.
    ///   - maximumWriteLength: `maximumWriteValueLength(for: .withoutResponse)`,
    ///     the payload of one ATT packet. Not the `.withResponse` figure: iOS
    ///     reports 512 there and sends anything longer than one packet as a
    ///     prepared write, which a peripheral must refuse for a hello.
    static func helloToWrite(
        identity: Data?,
        peerServesHello: Bool,
        alreadyWritten: Bool,
        maximumWriteLength: Int
    ) -> Data? {
        guard peerServesHello, !alreadyWritten, let identity = identity else { return nil }
        guard identity.count >= minLength, identity.count <= maxLength else { return nil }
        guard identity.count <= maximumWriteLength else { return nil }
        return identity
    }
}
