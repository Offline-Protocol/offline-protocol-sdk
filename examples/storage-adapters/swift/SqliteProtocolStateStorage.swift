import Foundation
import SQLite3

/// A SQLite-backed protocol-state adapter, as a worked reference.
///
/// This is the whole "bring your own backend" path in one file. It exists to
/// prove the claim rather than to be copied verbatim: the SDK ships a working
/// file-backed provider, and an application only reaches for something like
/// this when it already has a store it wants its data inside.
///
/// What the SDK guarantees to an adapter:
/// - Values are opaque bytes. Records whose category requires sealing are
///   encrypted before `store` is called, so this class never sees document
///   content or message plaintext and cannot weaken the at-rest posture. It
///   also means values must round-trip as `Data`, never through `String`.
/// - `keyType` is a namespace and `keyId` an identifier within it. Both are
///   opaque; the data layer composes ids like `space/doc/0000000000000001`,
///   so do not treat them as file paths.
///
/// What the adapter owes: the contract checked by `runStorageConformance`.
/// Green is the definition of "this backend is supported".
///
/// Logout: an application that points documents here MUST call
/// `DataStore.wipeAll()` on logout. `wipePersistedState` clears the default
/// provider's directory, which this database is not inside.
final class SqliteProtocolStateStorage: ProtocolStateStorageProvider {

    private var db: OpaquePointer?
    // sqlite3 handles are not safe to use concurrently in the default
    // threading mode, and the SDK calls in from whichever thread is doing
    // protocol work.
    private let queue = DispatchQueue(label: "offline-protocol.sqlite-state")

    init(path: String) throws {
        var handle: OpaquePointer?
        guard sqlite3_open(path, &handle) == SQLITE_OK, let opened = handle else {
            throw MlsStorageError.StoreFailed(message: "failed to open \(path)")
        }
        self.db = opened

        // WAL keeps restore reads from blocking background writes. FULL
        // synchronous, not NORMAL: these records are the crash-recovery
        // state, so a store() that returns before the bytes are durable
        // turns a power loss into silent data loss.
        try exec("PRAGMA journal_mode=WAL")
        try exec("PRAGMA synchronous=FULL")
        try exec("""
            CREATE TABLE IF NOT EXISTS protocol_state (
                key_type TEXT NOT NULL,
                key_id   TEXT NOT NULL,
                value    BLOB NOT NULL,
                PRIMARY KEY (key_type, key_id)
            )
            """)
    }

    deinit {
        if let db { sqlite3_close(db) }
    }

    func store(keyType: String, keyId: String, data: Data) throws {
        // INSERT ... ON CONFLICT DO UPDATE, not a bare INSERT: a second store
        // under the same key must replace. A backend that keeps the first
        // value is the defect `store_overwrites` exists to catch, and it stays
        // invisible until data is stale.
        let sql = """
            INSERT INTO protocol_state (key_type, key_id, value) VALUES (?, ?, ?)
            ON CONFLICT (key_type, key_id) DO UPDATE SET value = excluded.value
            """
        try queue.sync {
            let statement = try prepare(sql)
            defer { sqlite3_finalize(statement) }
            bindText(statement, 1, keyType)
            bindText(statement, 2, keyId)
            // An empty Data must still store as an empty blob rather than
            // NULL: an empty value is a value, and reading it back as missing
            // is a distinct answer the SDK acts on differently.
            data.withUnsafeBytes { buffer in
                sqlite3_bind_blob(statement, 3, buffer.baseAddress, Int32(data.count), SQLITE_TRANSIENT)
            }
            guard sqlite3_step(statement) == SQLITE_DONE else {
                throw MlsStorageError.StoreFailed(message: lastError())
            }
        }
    }

    func load(keyType: String, keyId: String) throws -> Data? {
        try queue.sync {
            let statement = try prepare(
                "SELECT value FROM protocol_state WHERE key_type = ? AND key_id = ?")
            defer { sqlite3_finalize(statement) }
            bindText(statement, 1, keyType)
            bindText(statement, 2, keyId)

            switch sqlite3_step(statement) {
            case SQLITE_ROW:
                let count = Int(sqlite3_column_bytes(statement, 0))
                guard let bytes = sqlite3_column_blob(statement, 0) else {
                    // A zero-length blob reports a null pointer; that is an
                    // empty value, not an absent one.
                    return Data()
                }
                return Data(bytes: bytes, count: count)
            case SQLITE_DONE:
                // A missing row is nil, not an error: the SDK asks for records
                // that legitimately do not exist yet on every launch.
                return nil
            default:
                // LoadFailed, not CorruptedData. CorruptedData is a permanent
                // verdict the SDK settles messages on; a failed read is not.
                throw MlsStorageError.LoadFailed(message: lastError())
            }
        }
    }

    func delete(keyType: String, keyId: String) throws {
        try queue.sync {
            let statement = try prepare(
                "DELETE FROM protocol_state WHERE key_type = ? AND key_id = ?")
            defer { sqlite3_finalize(statement) }
            bindText(statement, 1, keyType)
            bindText(statement, 2, keyId)
            guard sqlite3_step(statement) == SQLITE_DONE else {
                throw MlsStorageError.DeleteFailed(message: lastError())
            }
        }
        // Deleting an absent key is deliberately not an error: the data layer
        // removes folded delta records a crash may already have taken.
    }

    func listKeys(keyType: String) throws -> [String] {
        try queue.sync {
            let statement = try prepare(
                "SELECT key_id FROM protocol_state WHERE key_type = ?")
            defer { sqlite3_finalize(statement) }
            bindText(statement, 1, keyType)

            var keys: [String] = []
            while true {
                switch sqlite3_step(statement) {
                case SQLITE_ROW:
                    if let text = sqlite3_column_text(statement, 0) {
                        keys.append(String(cString: text))
                    }
                case SQLITE_DONE:
                    return keys
                default:
                    throw MlsStorageError.LoadFailed(message: lastError())
                }
            }
        }
    }

    // MARK: - sqlite plumbing

    private static let SQLITE_TRANSIENT = unsafeBitCast(
        -1, to: sqlite3_destructor_type.self)

    private func prepare(_ sql: String) throws -> OpaquePointer? {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            throw MlsStorageError.LoadFailed(message: lastError())
        }
        return statement
    }

    private func bindText(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, Self.SQLITE_TRANSIENT)
    }

    private func exec(_ sql: String) throws {
        guard sqlite3_exec(db, sql, nil, nil, nil) == SQLITE_OK else {
            throw MlsStorageError.StoreFailed(message: lastError())
        }
    }

    private func lastError() -> String {
        guard let db, let message = sqlite3_errmsg(db) else { return "unknown sqlite error" }
        return String(cString: message)
    }
}

private let SQLITE_TRANSIENT = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
