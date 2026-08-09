import XCTest
@testable import OfflineProtocol

final class StorageNamespaceTests: XCTestCase {
    func testAccountNamespaceIsStableAndOpaque() {
        XCTAssertEqual(
            StorageNamespace.account(appId: "test-app", profile: "test-user-1"),
            "account-814873e0cbdb2a1f25f14b31625e7f904cf9923e55b415b91ca4b29b210c12a1"
        )
    }

    func testAccountNamespaceSeparatesAccounts() {
        XCTAssertNotEqual(
            StorageNamespace.account(appId: "chat", profile: "alice"),
            StorageNamespace.account(appId: "chat", profile: "bob")
        )
        XCTAssertNotEqual(
            StorageNamespace.account(appId: "chat", profile: "alice"),
            StorageNamespace.account(appId: "other-chat", profile: "alice")
        )
    }

    func testGeneratedNamespacesPassValidation() throws {
        let namespace = StorageNamespace.account(appId: "chat", profile: "alice")
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
            "account-" + String(repeating: "g", count: 64),
            // Fullwidth forms: `Character.isHexDigit` accepts these and
            // `String.count` measures them as one each, so the natural Swift
            // spelling of this check would let them through — while the Android
            // and Python validators, which match a literal `[0-9a-f]`, would
            // not. All three have to refuse the same set.
            "account-" + String(repeating: "\u{FF41}", count: 64),
            "account-" + String(repeating: "\u{FF10}", count: 64)
        ] {
            XCTAssertThrowsError(
                try StorageNamespace.requireAccount(value),
                "expected \(value) to be refused"
            )
        }
    }
}
