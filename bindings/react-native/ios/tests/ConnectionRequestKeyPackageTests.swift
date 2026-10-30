import XCTest
@testable import OfflineProtocol

/// `keyPackage` is optional on `acceptConnectionRequest` and
/// `sendConnectionRequest`. In 0.31.0 the React Native adapter boxed an
/// omitted one as a nil `[Int]?`, which `byteList` refused as "must be a byte
/// list", so an iPhone could accept or send a connection request only with a
/// key package. Android reads a missing list as empty and never failed.
///
/// The adapter imports React and cannot compile here, so each call builds the
/// argument map the way the generated adapter does, through
/// `BridgeResults.byteListArgument`. Needs the real engine, so the SwiftPM
/// harness leaves it out and the Swift package job runs it.
final class ConnectionRequestKeyPackageTests: XCTestCase {

    private final class Listener: EventCallback, @unchecked Sendable {
        func onEvent(eventJson: String) {}
    }

    func testAnOmittedByteListIsAbsentAndAnEmptyOneIsEmpty() throws {
        for list in [nil, []] as [[NSNumber]?] {
            let arguments = ["keyPackage": BridgeResults.byteListArgument(list)]
            XCTAssertEqual(try BridgeArgumentMaps.byteList(arguments, "keyPackage"), [])
        }
        let arguments = ["keyPackage": BridgeResults.byteListArgument([0, 7, 255])]
        XCTAssertEqual(try BridgeArgumentMaps.byteList(arguments, "keyPackage"), [0, 7, 255])
    }

    func testBothRequestsSucceedWithoutAKeyPackage() throws {
        let host = ProtocolEngineHost(onDiagnostic: { _, _, _ in })
        try host.create(
            configJson: #"{"appId":"test-app","profile":"clinic","bleEnabled":false,"wifiDirectEnabled":false,"internetEnabled":true}"#,
            eventCallback: Listener(),
            emitEventJson: nil
        )
        defer { host.tearDown() }
        try host.startProtocolCore()

        // Omitted, then the empty list apps pass to work around 0.31.0.
        for keyPackage in [nil, []] as [[NSNumber]?] {
            let sent = try ProtocolEngineHostBridge.sendConnectionRequest(
                host: host, env: .inert(),
                arguments: ["recipient": "health-worker", "senderName": "Clinic",
                            "keyPackage": BridgeResults.byteListArgument(keyPackage),
                            "initialMessage": NSNull()]
            )
            XCTAssertFalse((sent as? String ?? "").isEmpty, "no message id: \(String(describing: sent))")

            let accepted = try ProtocolEngineHostBridge.acceptConnectionRequest(
                host: host, env: .inert(),
                arguments: ["recipient": "health-worker", "accepterName": "Clinic",
                            "keyPackage": BridgeResults.byteListArgument(keyPackage)]
            )
            XCTAssertFalse((accepted as? String ?? "").isEmpty, "no message id: \(String(describing: accepted))")
        }
    }
}
