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
            data: Data([0, 1, 255])
        )
        XCTAssertEqual(
            try storage.load(keyType: "pending/messages", keyId: "peer with punctuation"),
            Data([0, 1, 255])
        )
        XCTAssertEqual(
            try storage.listKeys(keyType: "pending/messages"),
            ["peer with punctuation"]
        )

        try storage.store(
            keyType: "pending/messages",
            keyId: "peer with punctuation",
            data: Data([4, 5])
        )
        XCTAssertEqual(
            try storage.load(keyType: "pending/messages", keyId: "peer with punctuation"),
            Data([4, 5])
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

        try alice.store(keyType: "outbox", keyId: "message-1", data: Data([1, 2, 3]))

        XCTAssertEqual(
            try alice.load(keyType: "outbox", keyId: "message-1"),
            Data([1, 2, 3])
        )
        XCTAssertNil(try bob.load(keyType: "outbox", keyId: "message-1"))
    }

    func testRestartReopensTheSameInstallRoot() throws {
        let root = temporaryRoot("restart")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let first = try AppContainerProtocolStateStorage(root: root)
        try first.store(keyType: "outbox", keyId: "message-1", data: Data([7, 8, 9]))

        let restarted = try AppContainerProtocolStateStorage(root: root)

        XCTAssertEqual(
            try restarted.load(keyType: "outbox", keyId: "message-1"),
            Data([7, 8, 9])
        )
    }

    // MARK: - Filesystem-key safety

    /// "AAG" and "AAa" differ only in the case of one base64url character, so
    /// an encoding-based filename gives them the same name on a
    /// case-insensitive volume (APFS's macOS default) and one record silently
    /// overwrites the other. A digest name cannot collide this way.
    func testCaseFoldingIdsAreDistinctRecords() throws {
        let root = temporaryRoot("case-fold")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        try storage.store(keyType: "outbox", keyId: "AAG", data: Data([1]))
        try storage.store(keyType: "outbox", keyId: "AAa", data: Data([2]))

        XCTAssertEqual(try storage.load(keyType: "outbox", keyId: "AAG"), Data([1]))
        XCTAssertEqual(try storage.load(keyType: "outbox", keyId: "AAa"), Data([2]))
        XCTAssertEqual(try storage.listKeys(keyType: "outbox"), ["AAG", "AAa"])
    }

    /// Core accepts user ids up to 256 bytes. Base64 of 190 bytes already
    /// overruns the 255-byte NAME_MAX most filesystems enforce; a digest name
    /// is a fixed 66 characters no matter how long the key is.
    func testMaximumLengthIdsRoundTrip() throws {
        let root = temporaryRoot("long-ids")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        let longId = String(repeating: "u", count: 256)
        try storage.store(keyType: "outbox", keyId: longId, data: Data([9]))

        XCTAssertEqual(try storage.load(keyType: "outbox", keyId: longId), Data([9]))
        XCTAssertEqual(try storage.listKeys(keyType: "outbox"), [longId])
        XCTAssertEqual(
            ProtocolStateRecord.entryName(keyType: "outbox", keyId: longId).count,
            66
        )
    }

    func testEveryEntryNameIsFixedLengthAndLowercase() {
        for keyId in ["", "AAG", String(repeating: "x", count: 4096), "péer/ id"] {
            let name = ProtocolStateRecord.entryName(keyType: "outbox", keyId: keyId)
            XCTAssertEqual(name.count, 66)
            XCTAssertEqual(name, name.lowercased())
        }
    }

    // MARK: - Framing

    /// Golden vector. The Android and Python providers must produce these exact
    /// bytes and names for the same input, or a record written by one platform
    /// is unreadable by another sharing a container.
    func testFramingGoldenVector() throws {
        let framed = try ProtocolStateRecord.frame(
            keyType: "outbox",
            keyId: "m-1",
            value: Data([0xAA, 0xBB])
        )
        XCTAssertEqual(
            [UInt8](framed),
            [
                0x4F, 0x50, 0x53, 0x31, // "OPS1"
                0x00, 0x06,             // key_type length
                0x00, 0x03,             // key_id length
                0x6F, 0x75, 0x74, 0x62, 0x6F, 0x78, // "outbox"
                0x6D, 0x2D, 0x31,       // "m-1"
                0xAA, 0xBB
            ]
        )

        XCTAssertEqual(
            ProtocolStateRecord.typeDirectoryName("outbox"),
            "t_d5fac01c82279b8b061df80b3c312942e2ce27a41a48b1b7479ff07ad5a6198d"
        )
        XCTAssertEqual(
            ProtocolStateRecord.entryName(keyType: "outbox", keyId: "m-1"),
            "k_db5fcc2398ef2863d4269a61be6ea2de1f80d2889f34670c9a57c79cbe8058a1"
        )

        let header = try XCTUnwrap(ProtocolStateRecord.parseHeader(framed))
        XCTAssertEqual(header.keyType, "outbox")
        XCTAssertEqual(header.keyId, "m-1")
        XCTAssertEqual(header.valueOffset, 17)
    }

    func testEmptyValueRoundTrips() throws {
        let root = temporaryRoot("empty")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        try storage.store(keyType: "blocked_users", keyId: "peer-1", data: Data())

        XCTAssertEqual(try storage.load(keyType: "blocked_users", keyId: "peer-1"), Data())
        XCTAssertEqual(try storage.listKeys(keyType: "blocked_users"), ["peer-1"])
    }

    // MARK: - Bounded reads

    /// A record over the ceiling cannot have been written through `store`, so
    /// it must be dropped by size alone — never read into memory first.
    func testOversizedFileIsRejectedWithoutBeingRead() throws {
        let root = temporaryRoot("oversized")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        try storage.store(keyType: "outbox", keyId: "message-1", data: Data([1, 2, 3]))

        let path = root
            .appendingPathComponent(ProtocolStateRecord.typeDirectoryName("outbox"))
            .appendingPathComponent(
                ProtocolStateRecord.entryName(keyType: "outbox", keyId: "message-1")
            )
        // Sparse file: the ceiling must be enforced on the *reported* size, so
        // this never occupies real disk in CI.
        let handle = try FileHandle(forWritingTo: path)
        try handle.truncate(atOffset: UInt64(ProtocolStateRecord.maxFileBytes) + 1)
        try handle.close()

        assertCorrupted(try storage.load(keyType: "outbox", keyId: "message-1"))
        XCTAssertFalse(FileManager.default.fileExists(atPath: path.path))
    }

    func testStoreRefusesValuesOverTheCeiling() {
        XCTAssertThrowsError(
            try ProtocolStateRecord.frame(
                keyType: "outbox",
                keyId: "m-1",
                value: Data(repeating: 0, count: ProtocolStateRecord.maxValueBytes + 1)
            )
        )
    }

    /// A file whose framing does not name the key that was asked for is not
    /// that record — drop it rather than hand back someone else's bytes, and
    /// report the drop so the SDK can settle the message id the app holds.
    func testMalformedRecordIsDroppedRatherThanReturned() throws {
        let root = temporaryRoot("malformed")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        try storage.store(keyType: "outbox", keyId: "message-1", data: Data([1, 2, 3]))

        let path = root
            .appendingPathComponent(ProtocolStateRecord.typeDirectoryName("outbox"))
            .appendingPathComponent(
                ProtocolStateRecord.entryName(keyType: "outbox", keyId: "message-1")
            )
        try Data([0, 1, 2, 3, 4, 5, 6, 7, 8]).write(to: path)

        assertCorrupted(try storage.load(keyType: "outbox", keyId: "message-1"))
        XCTAssertEqual(try storage.listKeys(keyType: "outbox"), [])
    }

    /// Destruction is not absence. A record the provider had to drop must be
    /// reported as `CorruptedData`, because that is what lets the SDK settle
    /// the message id the application is still holding for it; a silent `nil`
    /// is indistinguishable from a record that was never written and leaves
    /// that id unresolved forever.
    private func assertCorrupted(
        _ expression: @autoclosure () throws -> Data?,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertThrowsError(try expression(), file: file, line: line) { error in
            guard let storageError = error as? MlsStorageError,
                  case .CorruptedData = storageError
            else {
                return XCTFail("expected CorruptedData, got \(error)", file: file, line: line)
            }
        }
    }

    func testUnframedStrayFilesAreIgnoredByListing() throws {
        let root = temporaryRoot("stray")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        try storage.store(keyType: "outbox", keyId: "message-1", data: Data([1]))

        let directory = root
            .appendingPathComponent(ProtocolStateRecord.typeDirectoryName("outbox"))
        try Data([1, 2, 3]).write(
            to: directory.appendingPathComponent("k_not-a-record")
        )
        try Data([1, 2, 3]).write(
            to: directory.appendingPathComponent("unrelated.tmp")
        )

        XCTAssertEqual(try storage.listKeys(keyType: "outbox"), ["message-1"])
    }

    /// The bound exists for a tampered container, and there the entries are
    /// exactly the ones that yield no key. Counting keys collected would leave
    /// every one of these opened on every launch while the counter sat at zero.
    func testEnumerationBoundCountsEntriesExaminedNotKeysReturned() throws {
        let root = temporaryRoot("bounded-listing")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        try storage.store(keyType: "outbox", keyId: "message-1", data: Data([1]))

        let directory = root
            .appendingPathComponent(ProtocolStateRecord.typeDirectoryName("outbox"))
        for index in 0..<10 {
            try Data([1, 2, 3]).write(
                to: directory.appendingPathComponent("k_unparseable-\(index)")
            )
        }

        let result = try storage.enumerateKeys(keyType: "outbox", limit: 4)

        XCTAssertEqual(result.examined, 4, "enumeration must stop at the bound it was given")
        XCTAssertLessThanOrEqual(result.keys.count, 1)
    }

    /// A digest names exactly one record, so two names for one key id can only
    /// come from a copy planted in the container. Restore must not walk the id
    /// twice because of it.
    func testListingDedupesARecordReachableUnderTwoNames() throws {
        let root = temporaryRoot("duplicate-names")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        try storage.store(keyType: "outbox", keyId: "message-1", data: Data([1, 2, 3]))

        let directory = root
            .appendingPathComponent(ProtocolStateRecord.typeDirectoryName("outbox"))
        let original = directory.appendingPathComponent(
            ProtocolStateRecord.entryName(keyType: "outbox", keyId: "message-1")
        )
        try Data(contentsOf: original).write(
            to: directory.appendingPathComponent("k_copy-of-message-1")
        )

        XCTAssertEqual(try storage.listKeys(keyType: "outbox"), ["message-1"])
    }

    /// `Data.write(.atomic)` writes a temporary in the same directory and
    /// renames it into place, so a crash in between orphans that file forever:
    /// enumeration filters on the `k_` prefix, so nothing ever looks at it
    /// again. The Python provider sweeps for exactly this; the three built-in
    /// stores are meant to be one implementation in three languages.
    func testStoreSweepsTemporariesLeftByAnInterruptedWrite() throws {
        let root = temporaryRoot("stale-temporaries")
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let storage = try AppContainerProtocolStateStorage(root: root)

        try storage.store(keyType: "outbox", keyId: "message-1", data: Data([1, 2, 3]))

        let directory = root
            .appendingPathComponent(ProtocolStateRecord.typeDirectoryName("outbox"))
        let orphan = directory.appendingPathComponent(".dat.nosync-interrupted")
        try Data([9, 9, 9]).write(to: orphan)

        // A second instance: the sweep runs once per type directory per
        // process, and the first store above already consumed this one's.
        let restarted = try AppContainerProtocolStateStorage(root: root)
        try restarted.store(keyType: "outbox", keyId: "message-2", data: Data([4, 5, 6]))

        XCTAssertFalse(
            FileManager.default.fileExists(atPath: orphan.path),
            "a store into the category must remove temporaries a previous process orphaned"
        )
        XCTAssertEqual(
            try restarted.listKeys(keyType: "outbox"),
            ["message-1", "message-2"],
            "the sweep must not touch real records"
        )
        XCTAssertEqual(try restarted.load(keyType: "outbox", keyId: "message-1"), Data([1, 2, 3]))
    }
}
