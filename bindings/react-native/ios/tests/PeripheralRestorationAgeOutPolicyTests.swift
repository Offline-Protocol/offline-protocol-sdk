//
// PeripheralRestorationAgeOutPolicyTests.swift
//
// Pins the age-out gate BleManager applies to peripherals iOS hands back
// via `centralManager(_:willRestoreState:)`. See
// PeripheralRestorationAgeOutPolicy.swift for the rule the fixtures below
// exercise.
//

import XCTest
@testable import OfflineProtocol

final class PeripheralRestorationAgeOutPolicyTests: XCTestCase {

    /// In-memory store for the age-out policy so the tests stay hermetic —
    /// we never touch `UserDefaults.standard`.
    private final class InMemoryStore: PeripheralRestorationStore {
        private(set) var records: [String: TimeInterval] = [:]
        private(set) var saveCount = 0

        func loadRestorationRecords() -> [String: TimeInterval] {
            return records
        }

        func saveRestorationRecords(_ records: [String: TimeInterval]) {
            self.records = records
            saveCount += 1
        }
    }

    private let ttl: TimeInterval = 60

    private func uuid(_ hex: String) -> UUID {
        return UUID(uuidString: hex)!
    }

    private let a = UUID(uuidString: "A0000000-0000-0000-0000-000000000001")!
    private let b = UUID(uuidString: "B0000000-0000-0000-0000-000000000002")!
    private let c = UUID(uuidString: "C0000000-0000-0000-0000-000000000003")!

    private let t0 = Date(timeIntervalSince1970: 1_700_000_000)

    // MARK: - Empty / cold-boot

    func testEmptyStoreMarksEveryCandidateStale() {
        // Fresh install, iOS restores three peripherals we've never observed.
        // All three must be treated as stale — the persisted map is our
        // record of "this SDK actually saw the peer," and it's empty.
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        let partition = policy.partitionRestored(candidates: [a, b, c], now: t0)

        XCTAssertEqual(partition.fresh, [])
        XCTAssertEqual(Set(partition.stale), Set([a, b, c]))
    }

    func testCandidateListIsEmptyProducesEmptyPartition() {
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        let partition = policy.partitionRestored(candidates: [], now: t0)

        XCTAssertEqual(partition.fresh, [])
        XCTAssertEqual(partition.stale, [])
    }

    // MARK: - Fresh vs stale within the TTL window

    func testRecordSeenMakesPeripheralFreshWithinTtl() {
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        policy.recordSeen(uuid: a, at: t0)

        // 30 s later — still well inside the 60 s TTL.
        let partition = policy.partitionRestored(
            candidates: [a],
            now: t0.addingTimeInterval(30)
        )
        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(partition.stale, [])
    }

    func testRecordedButExpiredPeripheralIsStale() {
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        policy.recordSeen(uuid: a, at: t0)

        // 61 s later — just past the boundary.
        let partition = policy.partitionRestored(
            candidates: [a],
            now: t0.addingTimeInterval(61)
        )
        XCTAssertEqual(partition.fresh, [])
        XCTAssertEqual(partition.stale, [a])
    }

    func testAgeExactlyAtTtlIsStale() {
        // The rule is `now - lastSeen < ttl` = fresh — exactly at the boundary
        // (age == ttl) is stale. Nail down the boundary so a future refactor
        // has to argue with the test, not with a hand-wave.
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        policy.recordSeen(uuid: a, at: t0)

        let partition = policy.partitionRestored(
            candidates: [a],
            now: t0.addingTimeInterval(ttl)
        )
        XCTAssertEqual(partition.fresh, [])
        XCTAssertEqual(partition.stale, [a])
    }

    func testMixedFreshAndStalePartitionCorrectly() {
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        policy.recordSeen(uuid: a, at: t0)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(-120)) // way past TTL
        // c is never recorded — never observed this process.

        let partition = policy.partitionRestored(
            candidates: [a, b, c],
            now: t0.addingTimeInterval(10)
        )
        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(Set(partition.stale), Set([b, c]))
    }

    // MARK: - Idempotence and overwrite

    func testRecordSeenOverwritesOlderTimestamp() {
        // A stale record can be revived by a fresh observation.
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600)) // stale
        policy.recordSeen(uuid: a, at: t0)                            // fresh now

        let partition = policy.partitionRestored(candidates: [a], now: t0)
        XCTAssertEqual(partition.fresh, [a])
    }

    // MARK: - Persistence

    func testRecordSeenPersistsToStore() {
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        policy.recordSeen(uuid: a, at: t0)

        XCTAssertEqual(store.records[a.uuidString], t0.timeIntervalSince1970)
    }

    func testNewPolicyLoadsPersistedRecords() {
        // A second policy instance built against the same store must inherit
        // everything the first one persisted — this is the whole point of the
        // class: state that survives an app relaunch.
        let store = InMemoryStore()
        let policy1 = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)
        policy1.recordSeen(uuid: a, at: t0)

        let policy2 = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)
        let partition = policy2.partitionRestored(
            candidates: [a],
            now: t0.addingTimeInterval(10)
        )
        XCTAssertEqual(partition.fresh, [a])
    }

    // MARK: - Pruning stale records from the store

    func testPartitionDropsStaleCandidateFromStore() {
        // A candidate iOS hands back that we age out must be evicted from the
        // persistent map too — otherwise the map grows monotonically.
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600))

        _ = policy.partitionRestored(candidates: [a], now: t0)
        XCTAssertNil(store.records[a.uuidString])
    }

    func testPartitionSweepsExpiredEntriesNotInCandidateList() {
        // A restore event that only lists {b} still needs to age out {a} if
        // {a} has crossed the TTL. Restoration is our natural sweep trigger.
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600)) // stale
        policy.recordSeen(uuid: b, at: t0)                            // fresh

        _ = policy.partitionRestored(candidates: [b], now: t0)

        XCTAssertNil(store.records[a.uuidString])
        XCTAssertEqual(store.records[b.uuidString], t0.timeIntervalSince1970)
    }

    func testPartitionKeepsFreshEntriesNotInCandidateList() {
        // Symmetric to the sweep test: an entry that's still within the TTL
        // must be kept even when iOS didn't hand it back this restoration.
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(store: store, ttlSeconds: ttl)

        policy.recordSeen(uuid: a, at: t0) // fresh, not in candidate list

        _ = policy.partitionRestored(candidates: [], now: t0.addingTimeInterval(10))

        XCTAssertEqual(store.records[a.uuidString], t0.timeIntervalSince1970)
    }

    // MARK: - Capacity cap

    func testExceedingMaxRecordsEvictsOldestOnRecord() {
        // Pathological dev loop: cycle three peripheral UUIDs with maxRecords
        // = 2. Oldest must be evicted at the third record.
        let store = InMemoryStore()
        let policy = PeripheralRestorationAgeOutPolicy(
            store: store,
            ttlSeconds: ttl,
            maxRecords: 2
        )

        policy.recordSeen(uuid: a, at: t0)                     // oldest
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(1))
        policy.recordSeen(uuid: c, at: t0.addingTimeInterval(2)) // triggers eviction

        XCTAssertNil(store.records[a.uuidString])
        XCTAssertNotNil(store.records[b.uuidString])
        XCTAssertNotNil(store.records[c.uuidString])
        XCTAssertEqual(policy.recordedPeripheralCount(), 2)
    }

    // MARK: - Defaults

    func testDefaultTtlIsSixtySeconds() {
        XCTAssertEqual(PeripheralRestorationAgeOutPolicy.defaultTtlSeconds, 60)
    }

    func testDefaultMaxRecordsIsTwoHundred() {
        XCTAssertEqual(PeripheralRestorationAgeOutPolicy.defaultMaxRecords, 200)
    }

    func testDefaultStoreKeyIsNamespaced() {
        XCTAssertEqual(
            UserDefaultsPeripheralRestorationStore.defaultKey,
            "mesh.blemanager.peripheralLastSeen.v1"
        )
    }

    // MARK: - UserDefaults-backed store roundtrip

    func testUserDefaultsStoreRoundtripsRecords() {
        // A suite-scoped UserDefaults keeps this test hermetic (never touches
        // .standard). Save then load must round-trip the exact map.
        let suiteName = "PeripheralRestorationAgeOutPolicyTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defer {
            defaults.removePersistentDomain(forName: suiteName)
        }
        let store = UserDefaultsPeripheralRestorationStore(
            userDefaults: defaults,
            key: "test-key"
        )

        let map: [String: TimeInterval] = [
            a.uuidString: 1_700_000_000,
            b.uuidString: 1_700_000_030
        ]
        store.saveRestorationRecords(map)

        let loaded = store.loadRestorationRecords()
        XCTAssertEqual(loaded, map)
    }

    func testUserDefaultsStoreEmptySaveClearsKey() {
        // Saving an empty map must clear the key entirely so the blob doesn't
        // sit as `{}` in UserDefaults forever.
        let suiteName = "PeripheralRestorationAgeOutPolicyTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defer {
            defaults.removePersistentDomain(forName: suiteName)
        }
        let store = UserDefaultsPeripheralRestorationStore(
            userDefaults: defaults,
            key: "test-key"
        )

        store.saveRestorationRecords([a.uuidString: 1_700_000_000])
        XCTAssertNotNil(defaults.object(forKey: "test-key"))

        store.saveRestorationRecords([:])
        XCTAssertNil(defaults.object(forKey: "test-key"))
    }
}
