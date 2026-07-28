import Foundation
import XCTest
@testable import OfflineProtocol

final class ProtocolStateStorageTests: XCTestCase {
    private func temporaryRoot(_ name: String) -> URL {
        FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent(name, isDirectory: true)
    }

    func testRoundTripOverwriteListingAndIdempotentDelete() throws {
        let root = temporaryRoot("state")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        try storage.store(
            keyType: "pending/messages",
            keyId: "peer with punctuation",
            data: [0, 1, 255]
        )
        XCTAssertEqual(
            try storage.load(keyType: "pending/messages", keyId: "peer with punctuation"),
            [0, 1, 255]
        )
        XCTAssertEqual(
            try storage.listKeys(keyType: "pending/messages"),
            ["peer with punctuation"]
        )

        try storage.store(
            keyType: "pending/messages",
            keyId: "peer with punctuation",
            data: [4, 5]
        )
        XCTAssertEqual(
            try storage.load(keyType: "pending/messages", keyId: "peer with punctuation"),
            [4, 5]
        )

        try storage.delete(keyType: "pending/messages", keyId: "peer with punctuation")
        try storage.delete(keyType: "pending/messages", keyId: "peer with punctuation")
        XCTAssertNil(
            try storage.load(keyType: "pending/messages", keyId: "peer with punctuation")
        )
        XCTAssertEqual(try storage.listKeys(keyType: "pending/messages"), [])
    }

    func testSeparateAccountRootsDoNotShareState() throws {
        let parent = temporaryRoot("accounts")
        defer { try? FileManager.default.removeItem(at: parent.deletingLastPathComponent()) }
        let alice = try AppContainerProtocolStateStorage(
            root: parent.appendingPathComponent("alice", isDirectory: true)
        )
        let bob = try AppContainerProtocolStateStorage(
            root: parent.appendingPathComponent("bob", isDirectory: true)
        )

        try alice.store(keyType: "outbox", keyId: "message-1", data: [1, 2, 3])

        XCTAssertEqual(
            try alice.load(keyType: "outbox", keyId: "message-1"),
            [1, 2, 3]
        )
        XCTAssertNil(try bob.load(keyType: "outbox", keyId: "message-1"))
    }

    func testRestartReopensTheSameInstallRoot() throws {
        let root = temporaryRoot("restart")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let first = try AppContainerProtocolStateStorage(root: root)
        try first.store(keyType: "outbox", keyId: "message-1", data: [7, 8, 9])

        let restarted = try AppContainerProtocolStateStorage(root: root)

        XCTAssertEqual(
            try restarted.load(keyType: "outbox", keyId: "message-1"),
            [7, 8, 9]
        )
    }
}
