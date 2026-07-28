//
// ProtocolStateStorage.swift
// OfflineProtocol
//
// App-container storage for restartable delivery and protocol state.
//
// Values are opaque bytes and are written verbatim: the SDK seals the
// categories that can carry message plaintext or media key material before
// handing them over, so they arrive as ciphertext. Do not inspect, re-encode,
// or truncate them.
//

import CryptoKit
import Foundation

#if SWIFT_PACKAGE
enum MlsStorageError: Error, Equatable {
    case StoreFailed(message: String)
    case LoadFailed(message: String)
    case DeleteFailed(message: String)
    case CorruptedData(message: String)
}

protocol ProtocolStateStorageProvider {
    func store(keyType: String, keyId: String, data: [UInt8]) throws
    func load(keyType: String, keyId: String) throws -> [UInt8]?
    func delete(keyType: String, keyId: String) throws
    func listKeys(keyType: String) throws -> [String]
}

// Same shape, distinct domain — mirrors the generated UniFFI protocol so
// `MlsSecureStorage` compiles under the test harness too.
protocol MlsStorageProvider {
    func store(keyType: String, keyId: String, data: [UInt8]) throws
    func load(keyType: String, keyId: String) throws -> [UInt8]?
    func delete(keyType: String, keyId: String) throws
    func listKeys(keyType: String) throws -> [String]
}
#endif

/// On-disk record format shared by every built-in protocol-state provider.
///
/// Filenames are a fixed-length lowercase hex digest rather than an encoding of
/// the key itself. An encoding cannot be a correct filesystem key: it is
/// case-sensitive (so `AAG` and `AAa` name the same file on a case-insensitive
/// volume, and one record silently overwrites the other) and its length grows
/// with the key (so a long-but-valid protocol id overruns `NAME_MAX`). A digest
/// is fixed-length, lowercase, and collision-free in practice.
///
/// Because a digest is one-way, the exact key cannot be recovered from the
/// name, so each record carries it in a header instead. That also makes every
/// file independently attributable and lets a read verify it opened the record
/// it asked for.
///
///     bytes 0..<4   magic "OPS1"
///     bytes 4..<6   key_type length, big-endian u16
///     bytes 6..<8   key_id length, big-endian u16
///     then          key_type UTF-8, key_id UTF-8, value bytes
///
/// Keep this format, its limits, and the golden vectors in
/// `ProtocolStateStorageTests` in sync with the Android and Python providers.
enum ProtocolStateRecord {
    static let magic: [UInt8] = Array("OPS1".utf8)

    /// Longest accepted `key_type` / `key_id`, in UTF-8 bytes. Core's own
    /// identifiers are far shorter; this bounds what a header may claim.
    static let maxComponentBytes = 4096

    /// Provider ceiling on a record's value. Deliberately a generous superset
    /// of core's `MAX_PROTOCOL_STATE_RECORD_BYTES` (4 MiB) plus its seal
    /// envelope, so this never rejects a record the SDK legitimately wrote —
    /// it exists to bound allocation for a file the SDK never wrote.
    static let maxValueBytes = 8 * 1024 * 1024

    /// Largest file the reader will pull into memory.
    static let maxFileBytes = 8 + 2 * maxComponentBytes + maxValueBytes

    /// Longest possible header, used to bound the partial read `listKeys` does.
    static let maxHeaderBytes = 8 + 2 * maxComponentBytes

    /// Ceiling on entries one `listKeys` will open. Core caps every category
    /// far below this; a directory holding more has been tampered with.
    ///
    /// The bound counts entries *examined*, not keys returned. Counting the
    /// latter would not bound the tampered case at all — the case this exists
    /// for: an entry whose header does not parse yields no key, so a directory
    /// full of unparseable `k_` files would be opened in its entirety on every
    /// launch while the counter sat at zero.
    static let maxListedKeys = 65_536

    static func digest(_ components: [String]) -> String {
        var hasher = SHA256()
        for component in components {
            hasher.update(data: Data(component.utf8))
            hasher.update(data: Data([0]))
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    static func typeDirectoryName(_ keyType: String) -> String {
        "t_" + digest([keyType])
    }

    static func entryName(keyType: String, keyId: String) -> String {
        "k_" + digest([keyType, keyId])
    }

    static func frame(keyType: String, keyId: String, value: [UInt8]) throws -> Data {
        let typeBytes = Array(keyType.utf8)
        let idBytes = Array(keyId.utf8)
        guard typeBytes.count <= maxComponentBytes, idBytes.count <= maxComponentBytes else {
            throw MlsStorageError.StoreFailed(
                message: "protocol-state key exceeds \(maxComponentBytes) bytes"
            )
        }
        guard value.count <= maxValueBytes else {
            throw MlsStorageError.StoreFailed(
                message: "protocol-state record is \(value.count) bytes, over the "
                    + "\(maxValueBytes) byte limit"
            )
        }

        var out = Data(capacity: 8 + typeBytes.count + idBytes.count + value.count)
        out.append(contentsOf: magic)
        out.append(UInt8(typeBytes.count >> 8))
        out.append(UInt8(typeBytes.count & 0xff))
        out.append(UInt8(idBytes.count >> 8))
        out.append(UInt8(idBytes.count & 0xff))
        out.append(contentsOf: typeBytes)
        out.append(contentsOf: idBytes)
        out.append(contentsOf: value)
        return out
    }

    /// Header of a framed record: the key it belongs to and where its value
    /// starts. `nil` means the bytes are not a record this SDK wrote.
    static func parseHeader(_ bytes: [UInt8]) -> (keyType: String, keyId: String, valueOffset: Int)? {
        guard bytes.count >= 8, Array(bytes[0..<4]) == magic else {
            return nil
        }
        let typeLen = Int(bytes[4]) << 8 | Int(bytes[5])
        let idLen = Int(bytes[6]) << 8 | Int(bytes[7])
        guard typeLen <= maxComponentBytes, idLen <= maxComponentBytes,
              bytes.count >= 8 + typeLen + idLen,
              let keyType = String(bytes: bytes[8..<(8 + typeLen)], encoding: .utf8),
              let keyId = String(bytes: bytes[(8 + typeLen)..<(8 + typeLen + idLen)], encoding: .utf8)
        else {
            return nil
        }
        return (keyType, keyId, 8 + typeLen + idLen)
    }
}

/// File-backed protocol state whose lifecycle is tied to the app container.
///
/// Each entry is written atomically. The directory is excluded from device
/// backup so uninstall/reinstall and device restore cannot resurrect an old
/// outbox or retry lifecycle.
final class AppContainerProtocolStateStorage: ProtocolStateStorageProvider {
    private static let schemaDirectory = "protocol-state-v1"

    private let root: URL
    private let fileManager: FileManager
    private let lock = NSLock()

    convenience init(
        accountNamespace: String,
        fileManager: FileManager = .default
    ) throws {
        guard let applicationSupport = fileManager.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            throw MlsStorageError.StoreFailed(
                message: "Application Support directory is unavailable"
            )
        }

        let bundleComponent = Bundle.main.bundleIdentifier ?? "com.offlineprotocol"
        let root = applicationSupport
            .appendingPathComponent(bundleComponent, isDirectory: true)
            .appendingPathComponent(Self.schemaDirectory, isDirectory: true)
            .appendingPathComponent(
                try StorageNamespace.requireAccount(accountNamespace),
                isDirectory: true
            )
        try self.init(root: root, fileManager: fileManager)
    }

    init(
        root: URL,
        fileManager: FileManager = .default
    ) throws {
        self.root = root
        self.fileManager = fileManager

        do {
            try fileManager.createDirectory(
                at: root,
                withIntermediateDirectories: true
            )
            var resourceValues = URLResourceValues()
            resourceValues.isExcludedFromBackup = true
            var mutableRoot = root
            try mutableRoot.setResourceValues(resourceValues)
        } catch {
            throw MlsStorageError.StoreFailed(
                message: "Failed to initialize protocol-state directory: \(error)"
            )
        }
    }

    func store(keyType: String, keyId: String, data: [UInt8]) throws {
        let framed = try ProtocolStateRecord.frame(
            keyType: keyType,
            keyId: keyId,
            value: data
        )

        lock.lock()
        defer { lock.unlock() }

        let directory = typeDirectory(keyType)
        let url = entryURL(keyType: keyType, keyId: keyId)
        do {
            try fileManager.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
            try framed.write(to: url, options: .atomic)
        } catch {
            throw MlsStorageError.StoreFailed(
                message: "Failed to persist protocol state: \(error)"
            )
        }
        flushFile(at: url)
        flushDirectory(at: directory)
    }

    func load(keyType: String, keyId: String) throws -> [UInt8]? {
        lock.lock()
        defer { lock.unlock() }

        let url = entryURL(keyType: keyType, keyId: keyId)
        guard fileManager.fileExists(atPath: url.path) else {
            return nil
        }

        // Stat before reading. A record over the ceiling cannot have been
        // written through `store`, so it is corrupt or tampered — removing it
        // keeps a poison file from being re-examined on every boot, and keeps
        // the read itself from becoming an unbounded allocation.
        let attributes = try? fileManager.attributesOfItem(atPath: url.path)
        if let size = attributes?[.size] as? Int, size > ProtocolStateRecord.maxFileBytes {
            throw discard(url, reason: "record is \(size) bytes, over the ceiling")
        }

        let raw: Data
        do {
            raw = try Data(contentsOf: url)
        } catch {
            throw MlsStorageError.LoadFailed(
                message: "Failed to load protocol state: \(error)"
            )
        }

        // Re-check post-read: the stat can race a concurrent writer, and on a
        // filesystem that refuses to report a size it is skipped entirely.
        guard raw.count <= ProtocolStateRecord.maxFileBytes else {
            throw discard(url, reason: "record is \(raw.count) bytes, over the ceiling")
        }

        let bytes = [UInt8](raw)
        guard let header = ProtocolStateRecord.parseHeader(bytes),
              header.keyType == keyType,
              header.keyId == keyId
        else {
            // Malformed framing, or a name that resolves to some other record:
            // either way this is not the entry that was asked for.
            throw discard(url, reason: "record framing does not name \(keyType)/\(keyId)")
        }
        return Array(bytes[header.valueOffset...])
    }

    func delete(keyType: String, keyId: String) throws {
        lock.lock()
        defer { lock.unlock() }

        let url = entryURL(keyType: keyType, keyId: keyId)
        guard fileManager.fileExists(atPath: url.path) else {
            return
        }
        do {
            try fileManager.removeItem(at: url)
        } catch {
            throw MlsStorageError.DeleteFailed(
                message: "Failed to delete protocol state: \(error)"
            )
        }
        // The unlink lives in the parent directory, so it needs its own flush
        // or a crash can resurrect an entry the SDK has already settled.
        flushDirectory(at: typeDirectory(keyType))
    }

    func listKeys(keyType: String) throws -> [String] {
        enumerateKeys(keyType: keyType, limit: ProtocolStateRecord.maxListedKeys).keys
    }

    /// Enumerates one category, opening at most `limit` entries.
    ///
    /// `limit` bounds the entries *examined* rather than the keys collected,
    /// because opening a file is the cost and an entry that fails to parse
    /// yields no key: a directory of unparseable records must not be walked in
    /// full on every launch. Exposed with an explicit `limit` so the bound is
    /// testable without materializing tens of thousands of files.
    ///
    /// Key ids are deduped: a name that resolves to a record already seen (a
    /// copy planted in the container) must not make the same id appear twice.
    func enumerateKeys(keyType: String, limit: Int) -> (keys: [String], examined: Int) {
        lock.lock()
        defer { lock.unlock() }

        let directory = typeDirectory(keyType)
        guard fileManager.fileExists(atPath: directory.path) else {
            return ([], 0)
        }

        // Stream the directory rather than materializing it, and read only each
        // record's header: enumeration must stay bounded even when the
        // container has been tampered with.
        guard let enumerator = fileManager.enumerator(
            at: directory,
            includingPropertiesForKeys: [.isRegularFileKey],
            options: [.skipsHiddenFiles, .skipsSubdirectoryDescendants]
        ) else {
            return ([], 0)
        }

        var keys = Set<String>()
        var examined = 0
        for case let url as URL in enumerator {
            if examined >= limit {
                break
            }
            guard url.lastPathComponent.hasPrefix("k_") else {
                continue
            }
            examined += 1
            guard let header = readHeader(at: url), header.keyType == keyType else {
                continue
            }
            keys.insert(header.keyId)
        }
        return (keys.sorted(), examined)
    }

    private func readHeader(at url: URL) -> (keyType: String, keyId: String)? {
        guard let handle = try? FileHandle(forReadingFrom: url) else {
            return nil
        }
        defer { handle.closeFile() }
        let prefix = handle.readData(ofLength: ProtocolStateRecord.maxHeaderBytes)
        guard let header = ProtocolStateRecord.parseHeader([UInt8](prefix)) else {
            return nil
        }
        return (header.keyType, header.keyId)
    }

    /// Removes a record that can never be read and returns the error that says
    /// so.
    ///
    /// `CorruptedData`, not a silent `nil`: absence and destruction are
    /// different answers upstream. The SDK settles a destroyed record — the
    /// application is holding the message id `send_message` returned for it —
    /// while absence is simply nothing to restore, reported to no one.
    private func discard(_ url: URL, reason: String) -> MlsStorageError {
        let directory = url.deletingLastPathComponent()
        try? fileManager.removeItem(at: url)
        flushDirectory(at: directory)
        return MlsStorageError.CorruptedData(
            message: "Dropped unreadable protocol-state record: \(reason)"
        )
    }

    /// Flushes a written entry to stable storage.
    ///
    /// `Data.write(options: .atomic)` is rename-*atomic*, not durable: the
    /// rename's directory entry can commit ahead of the new file's data
    /// blocks, so a power loss can leave the record present and zero-filled.
    /// That file then fails to frame and is dropped — and core treats a
    /// returned success as persisted, most sharply for the records it seals
    /// under a key it just stored. Android's `AtomicFile` and Python's
    /// fsync-file-then-directory both flush; this is the same guarantee.
    ///
    /// `F_FULLFSYNC` rather than `fsync(2)`: on Apple platforms `fsync` only
    /// hands the blocks to the drive, which may still buffer them.
    ///
    /// Best effort. A filesystem that refuses the flush leaves the atomic
    /// rename as the strongest guarantee available, which is what every write
    /// had before.
    private func flushFile(at url: URL) {
        let descriptor = open(url.path, O_WRONLY)
        guard descriptor >= 0 else { return }
        defer { close(descriptor) }
        if fcntl(descriptor, F_FULLFSYNC) == -1 {
            _ = fsync(descriptor)
        }
    }

    /// Flushes a directory entry so a rename or unlink in it survives a crash.
    /// A directory cannot be opened for writing, so this reads.
    private func flushDirectory(at url: URL) {
        let descriptor = open(url.path, O_RDONLY)
        guard descriptor >= 0 else { return }
        defer { close(descriptor) }
        if fcntl(descriptor, F_FULLFSYNC) == -1 {
            _ = fsync(descriptor)
        }
    }

    private func typeDirectory(_ keyType: String) -> URL {
        root.appendingPathComponent(
            ProtocolStateRecord.typeDirectoryName(keyType),
            isDirectory: true
        )
    }

    private func entryURL(keyType: String, keyId: String) -> URL {
        typeDirectory(keyType).appendingPathComponent(
            ProtocolStateRecord.entryName(keyType: keyType, keyId: keyId),
            isDirectory: false
        )
    }
}
