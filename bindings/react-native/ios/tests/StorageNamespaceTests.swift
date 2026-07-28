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
}
