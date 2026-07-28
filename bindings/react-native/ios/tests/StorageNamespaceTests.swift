import XCTest
@testable import OfflineProtocol

final class StorageNamespaceTests: XCTestCase {
    func testAccountNamespaceIsStableAndOpaque() {
        XCTAssertEqual(
            StorageNamespace.account(appId: "test-app", userId: "test-user-1"),
            "account-814873e0cbdb2a1f25f14b31625e7f904cf9923e55b415b91ca4b29b210c12a1"
        )
    }

    func testAccountNamespaceSeparatesAccounts() {
        XCTAssertNotEqual(
            StorageNamespace.account(appId: "chat", userId: "alice"),
            StorageNamespace.account(appId: "chat", userId: "bob")
        )
        XCTAssertNotEqual(
            StorageNamespace.account(appId: "chat", userId: "alice"),
            StorageNamespace.account(appId: "other-chat", userId: "alice")
        )
    }

    func testGeneratedNamespacesPassValidation() throws {
        let namespace = StorageNamespace.account(appId: "chat", userId: "alice")
        XCTAssertEqual(try StorageNamespace.requireAccount(namespace), namespace)
    }

    /// A namespace becomes a directory component and a Keychain service suffix,
    /// so anything that could escape or collide must be refused at the door.
    func testMalformedNamespacesAreRefused() {
        for value in [
            "",
            "account-",
            "../../etc",
            "account-" + String(repeating: "a", count: 63),
            "account-" + String(repeating: "a", count: 65),
            "account-" + String(repeating: "A", count: 64),
            "account-" + String(repeating: "g", count: 64)
        ] {
            XCTAssertThrowsError(
                try StorageNamespace.requireAccount(value),
                "expected \(value) to be refused"
            )
        }
    }
}
