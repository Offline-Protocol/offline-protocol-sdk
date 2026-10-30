import XCTest
@testable import OfflineProtocol

/// Two flags the React Native adapter lost in 0.31.0: `createInvite` keyed
/// `signed` as `sign`, so every invite came out unsigned, and
/// `checkInternetPresence` nested `force` in an `options` map no bridge opened.
/// `tools/check_rn_adapter_argument_keys.py` holds the key names; these hold
/// what the bridge does with them.
///
/// The adapter imports React and cannot compile here, so each call builds the
/// argument map in the shape the generated adapter sends. Needs the real
/// engine, so the SwiftPM harness leaves it out and the Swift package job runs
/// it.
final class AdapterArgumentReadTests: XCTestCase {

    private final class Listener: EventCallback, @unchecked Sendable {
        func onEvent(eventJson: String) {}
    }

    func testTheSignedFlagReachesTheInvite() throws {
        let host = ProtocolEngineHost(onDiagnostic: { _, _, _ in })
        try host.create(
            configJson: #"{"appId":"test-app","profile":"clinic","bleEnabled":false,"wifiDirectEnabled":false,"internetEnabled":true}"#,
            eventCallback: Listener(),
            emitEventJson: nil
        )
        defer { host.tearDown() }
        // A signed invite needs the MLS identity, and that must exist before
        // the engine starts. File stores keep the test off the keychain.
        let stores = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: stores) }
        for directory in ["mls", "state"] {
            try FileManager.default.createDirectory(
                at: stores.appendingPathComponent(directory),
                withIntermediateDirectories: true
            )
        }
        try host.requireProtocol().initializeMlsWithFileStores(
            mlsRoot: stores.appendingPathComponent("mls").path,
            stateRoot: stores.appendingPathComponent("state").path,
            storeKey: Data(repeating: 7, count: 32)
        )
        try host.startProtocolCore()

        for signed in [true, false] {
            let blob = try ProtocolEngineHostBridge.createInvite(
                host: host, env: .inert(),
                arguments: ["petname": "Clinic", "signed": NSNumber(value: signed)]
            )
            let parsed = try ProtocolEngineHostBridge.parseInvite(
                host: host, env: .inert(), arguments: ["blob": blob as Any]
            )
            XCTAssertEqual((parsed as? [String: Any])?["signed"] as? Bool, signed)
        }
    }

    func testForceIsReadFromEitherShape() {
        let on = NSNumber(value: true)
        let off = NSNumber(value: false)
        XCTAssertTrue(ProtocolEngineHostBridge.presenceForce(["options": ["force": on]]))
        XCTAssertTrue(ProtocolEngineHostBridge.presenceForce(["force": on]))
        XCTAssertFalse(ProtocolEngineHostBridge.presenceForce(["options": ["force": off]]))
        XCTAssertFalse(ProtocolEngineHostBridge.presenceForce(["options": [String: Any]()]))
        XCTAssertFalse(ProtocolEngineHostBridge.presenceForce([:]))
    }
}
