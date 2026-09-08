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

    private let a = UUID(uuidString: "A0000000-0000-0000-0000-000000000001")!
    private let b = UUID(uuidString: "B0000000-0000-0000-0000-000000000002")!
    private let c = UUID(uuidString: "C0000000-0000-0000-0000-000000000003")!

    private let t0 = Date(timeIntervalSince1970: 1_700_000_000)

    /// A peripheral iOS hands back with a connect request still queued but no
    /// live link. This is the case the age-out is allowed to judge.
    private func pending(_ uuid: UUID) -> PeripheralRestorationCandidate {
        return PeripheralRestorationCandidate(uuid: uuid, isConnected: false)
    }

    /// A peripheral iOS hands back in `.connected` state.
    private func connected(_ uuid: UUID) -> PeripheralRestorationCandidate {
        return PeripheralRestorationCandidate(uuid: uuid, isConnected: true)
    }

    /// Most fixtures are not about the write-through throttle, so they persist
    /// on every call and assert on the store directly.
    private func makePolicy(store: PeripheralRestorationStore,
                            ttlSeconds: TimeInterval? = nil,
                            maxRecords: Int = PeripheralRestorationAgeOutPolicy.defaultMaxRecords,
                            persistIntervalSeconds: TimeInterval = 0) -> PeripheralRestorationAgeOutPolicy {
        return PeripheralRestorationAgeOutPolicy(
            store: store,
            ttlSeconds: ttlSeconds ?? ttl,
            maxRecords: maxRecords,
            persistIntervalSeconds: persistIntervalSeconds
        )
    }

    // MARK: - Empty / cold-boot

    func testEmptyStoreMarksEveryPendingCandidateStale() {
        // Fresh install, iOS restores three pending connects we've never
        // observed. All three must be treated as stale — the persisted map is
        // our record of "this SDK actually saw the peer," and it's empty.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        let partition = policy.partitionRestored(
            candidates: [pending(a), pending(b), pending(c)],
            now: t0
        )

        XCTAssertEqual(partition.fresh, [])
        XCTAssertEqual(Set(partition.stale), Set([a, b, c]))
    }

    func testCandidateListIsEmptyProducesEmptyPartition() {
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        let partition = policy.partitionRestored(candidates: [], now: t0)

        XCTAssertEqual(partition.fresh, [])
        XCTAssertEqual(partition.stale, [])
    }

    // MARK: - A live link is never aged out

    func testConnectedCandidateIsFreshWithNoRecordAtAll() {
        // The migration case, and the one that matters most: the first
        // restoration after this ships finds an empty map. A live GATT link
        // must survive it. Cancelling here would tear down the very link that
        // caused iOS to relaunch us.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        let partition = policy.partitionRestored(candidates: [connected(a)], now: t0)

        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(partition.stale, [])
    }

    func testConnectedCandidateIsFreshDespiteLongExpiredRecord() {
        // Nothing refreshes a timestamp while the app is dead, and background
        // discovery is throttled, so an hours-old record on a live link is
        // the normal reading — not evidence of anything.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600))

        let partition = policy.partitionRestored(candidates: [connected(a)], now: t0)

        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(partition.stale, [])
    }

    func testConnectedCandidateRefreshesItsPersistedRecord() {
        // The link being alive is itself a sighting. Recording it means the
        // next restoration reads the peripheral as fresh even if the link has
        // dropped by then and it comes back as a pending connect.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600))
        _ = policy.partitionRestored(candidates: [connected(a)], now: t0)

        XCTAssertEqual(store.records[a.uuidString], t0.timeIntervalSince1970)

        // Prove the consequence, not just the stored number: a second
        // restoration a few seconds later, this time with the link gone,
        // still restores rather than cancelling.
        let second = policy.partitionRestored(
            candidates: [pending(a)],
            now: t0.addingTimeInterval(5)
        )
        XCTAssertEqual(second.fresh, [a])
    }

    func testConnectedAndPendingCandidatesPartitionIndependently() {
        // a: live link, record long expired  -> fresh (state wins)
        // b: pending connect, record expired -> stale
        // c: pending connect, record fresh   -> fresh
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600))
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(-3600))
        policy.recordSeen(uuid: c, at: t0)

        let partition = policy.partitionRestored(
            candidates: [connected(a), pending(b), pending(c)],
            now: t0.addingTimeInterval(10)
        )

        XCTAssertEqual(Set(partition.fresh), Set([a, c]))
        XCTAssertEqual(partition.stale, [b])
    }

    // MARK: - Fresh vs stale within the TTL window

    func testRecordSeenMakesPendingPeripheralFreshWithinTtl() {
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0)

        // 30 s later — still well inside the 60 s TTL.
        let partition = policy.partitionRestored(
            candidates: [pending(a)],
            now: t0.addingTimeInterval(30)
        )
        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(partition.stale, [])
    }

    func testRecordedButExpiredPendingPeripheralIsStale() {
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0)
        // `b` is the anchor. It is what makes this a real age-out: the app was
        // still awake and watching 61 s after it last saw `a`, so `a` went
        // quiet rather than the clock merely running on while the process was
        // dead.
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(61))

        // 61 s later — just past the boundary.
        let partition = policy.partitionRestored(
            candidates: [pending(a)],
            now: t0.addingTimeInterval(61)
        )
        XCTAssertEqual(partition.fresh, [])
        XCTAssertEqual(partition.stale, [a])
    }

    func testAgeExactlyAtTtlIsStale() {
        // The rule is `anchor - lastSeen < ttl` = fresh — exactly at the
        // boundary (age == ttl) is stale. Nail down the boundary so a future
        // refactor has to argue with the test, not with a hand-wave.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(ttl)) // the anchor

        let partition = policy.partitionRestored(
            candidates: [pending(a)],
            now: t0.addingTimeInterval(ttl)
        )
        XCTAssertEqual(partition.fresh, [])
        XCTAssertEqual(partition.stale, [a])
    }

    func testMixedFreshAndStalePartitionCorrectly() {
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(-120)) // way past TTL
        // c is never recorded — never observed this process.

        let partition = policy.partitionRestored(
            candidates: [pending(a), pending(b), pending(c)],
            now: t0.addingTimeInterval(10)
        )
        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(Set(partition.stale), Set([b, c]))
    }

    // MARK: - The TTL is anchored to the newest sighting, not to `now`

    func testPendingCandidateSurvivesALongDeadPeriodWhenNothingNewerWasSeen() {
        // The app was terminated with `a` in view and iOS relaunched it three
        // hours later. Nothing writes the map while the process is dead, so
        // measuring `a`'s age against the restoration clock would call it
        // stale and cancel its connect request — the only way iOS wakes this
        // app when `a` comes back, since a background scan with no service
        // filter is ignored by the OS. `a` was the last thing this app saw.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0)

        let partition = policy.partitionRestored(
            candidates: [pending(a)],
            now: t0.addingTimeInterval(3 * 3600)
        )
        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(partition.stale, [])
    }

    func testAnchorIsTheNewestRecordNotTheRestorationClock() {
        // Same three-hour dead period, but the app went on to see `b` two
        // minutes after it last saw `a`. That is the evidence the clock alone
        // cannot supply: `a` went quiet while the app was still awake and
        // watching, so it ages out. `b` was the last thing seen, so it stays.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(120))

        let partition = policy.partitionRestored(
            candidates: [pending(a), pending(b)],
            now: t0.addingTimeInterval(3 * 3600)
        )
        XCTAssertEqual(partition.fresh, [b])
        XCTAssertEqual(partition.stale, [a])
    }

    func testDevLoopUuidChurnStillAgesOutTheDeadUuids() {
        // The case the class exists for, under the anchored cutoff. Every
        // relay restart mints a fresh peripheral UUID, and each new UUID
        // becomes the anchor that pushes its predecessors past the cutoff, so
        // the OS-side connect queue stays bounded even though no restoration
        // ever consults the wall clock.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0)                         // relay run 1
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(300)) // run 2
        policy.recordSeen(uuid: c, at: t0.addingTimeInterval(600)) // run 3

        let partition = policy.partitionRestored(
            candidates: [pending(a), pending(b), pending(c)],
            now: t0.addingTimeInterval(3 * 3600)
        )
        XCTAssertEqual(partition.fresh, [c])
        XCTAssertEqual(Set(partition.stale), Set([a, b]))
    }

    func testConnectedRefreshDoesNotTightenTheCutoffForPendingCandidates() {
        // The connected branch writes `now` into the map. Reading the anchor
        // after that instead of before would let a live link drag the cutoff
        // forward to `now - ttl` and age out pending connects the map says
        // were seen alongside it — reinstating the behaviour the anchor exists
        // to avoid, on exactly the restorations that carry a live link.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-100))
        policy.recordSeen(uuid: c, at: t0.addingTimeInterval(-90))

        let partition = policy.partitionRestored(
            candidates: [connected(c), pending(a)],
            now: t0
        )

        XCTAssertEqual(Set(partition.fresh), Set([a, c]))
        XCTAssertEqual(partition.stale, [])
    }

    func testFutureRecordDoesNotAgeOutCurrentEntries() {
        // A record written before a backwards clock step sits in the future.
        // Anchoring to it would put the cutoff ahead of every honest entry and
        // age out the whole map at once, so the anchor is clamped to `now`.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(3600))

        let partition = policy.partitionRestored(candidates: [pending(a)], now: t0)

        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(partition.stale, [])
    }

    // MARK: - A repeated candidate is decided once

    func testDuplicateCandidateIsDecidedOnce() {
        // Neither output list may carry a UUID twice: `stale` drives
        // `cancelPeripheralConnection`, `fresh` drives `connect`.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        let partition = policy.partitionRestored(
            candidates: [pending(a), pending(a)],
            now: t0
        )

        XCTAssertEqual(partition.fresh, [])
        XCTAssertEqual(partition.stale, [a])
    }

    func testDuplicateCandidateKeepsTheFirstDecision() {
        // BleManager resolves a duplicated identifier in favour of the
        // connected instance before it calls in, so first-wins here means the
        // live link wins there.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        let partition = policy.partitionRestored(
            candidates: [connected(a), pending(a)],
            now: t0
        )

        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(partition.stale, [])
    }

    // MARK: - Idempotence and overwrite

    func testRecordSeenOverwritesOlderTimestamp() {
        // A stale record can be revived by a fresh observation.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600)) // stale
        policy.recordSeen(uuid: a, at: t0)                            // fresh now

        let partition = policy.partitionRestored(candidates: [pending(a)], now: t0)
        XCTAssertEqual(partition.fresh, [a])
    }

    // MARK: - Persistence

    func testRecordSeenPersistsToStore() {
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0)

        XCTAssertEqual(store.records[a.uuidString], t0.timeIntervalSince1970)
    }

    func testNewPolicyLoadsPersistedRecords() {
        // A second policy instance built against the same store must inherit
        // everything the first one persisted — this is the whole point of the
        // class: state that survives an app relaunch.
        let store = InMemoryStore()
        let policy1 = makePolicy(store: store)
        policy1.recordSeen(uuid: a, at: t0)

        let policy2 = makePolicy(store: store)
        let partition = policy2.partitionRestored(
            candidates: [pending(a)],
            now: t0.addingTimeInterval(10)
        )
        XCTAssertEqual(partition.fresh, [a])
    }

    // MARK: - Write-through throttle

    func testFirstSightingOfAUuidAlwaysPersistsImmediately() {
        // A new peripheral must reach the store even if another write just
        // happened: losing a first sighting to a termination costs a
        // cancelled reconnect on the next restoration.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, persistIntervalSeconds: 10)

        policy.recordSeen(uuid: a, at: t0)
        XCTAssertEqual(store.saveCount, 1)

        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(1))
        XCTAssertEqual(store.saveCount, 2)
    }

    func testRepeatSightingsWithinTheIntervalDoNotHitTheStore() {
        // `recordSeen` runs from the scan callback on the main queue, roughly
        // once a second per visible peer. Each save serialises the whole map
        // through UserDefaults, so repeats inside the interval are dropped.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, persistIntervalSeconds: 10)

        policy.recordSeen(uuid: a, at: t0)
        XCTAssertEqual(store.saveCount, 1)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(1))
        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(5))
        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(9))
        XCTAssertEqual(store.saveCount, 1)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(10))
        XCTAssertEqual(store.saveCount, 2)
        XCTAssertEqual(store.records[a.uuidString], t0.addingTimeInterval(10).timeIntervalSince1970)
    }

    func testThrottledSightingIsStillCurrentInMemory() {
        // The throttle may delay the write-through; it must never delay the
        // decision. A sighting dropped by the throttle still counts against
        // the TTL on the next restoration.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, persistIntervalSeconds: 10)

        policy.recordSeen(uuid: a, at: t0)
        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(5)) // throttled
        // `b` anchors the cutoff at t0. The first sighting alone sits exactly
        // on it and would age out; the throttled one is 5 s inside it.
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(60))

        let partition = policy.partitionRestored(
            candidates: [pending(a)],
            now: t0.addingTimeInterval(60)
        )
        XCTAssertEqual(partition.fresh, [a])
    }

    func testClockSteppingBackwardsForcesAPersist() {
        // The throttle compares wall-clock readings, so a backwards step must
        // force the write rather than block it — otherwise one clock change
        // could stop the map persisting for as long as the step.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, persistIntervalSeconds: 10)

        policy.recordSeen(uuid: a, at: t0)
        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(1)) // throttled
        XCTAssertEqual(store.saveCount, 1)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-100))
        XCTAssertEqual(store.saveCount, 2)
    }

    func testPartitionAlwaysPersists() {
        // Restoration is rare and its side effects (stale eviction, connected
        // refresh) must be durable immediately: the process may be terminated
        // again at any moment.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, persistIntervalSeconds: 3600)

        policy.recordSeen(uuid: a, at: t0)
        let saves = store.saveCount

        _ = policy.partitionRestored(candidates: [connected(a)], now: t0.addingTimeInterval(1))
        XCTAssertEqual(store.saveCount, saves + 1)
    }

    // MARK: - Pruning stale records from the store

    func testPartitionDropsStaleCandidateFromStore() {
        // A candidate iOS hands back that we age out must be evicted from the
        // persistent map too — otherwise the map grows monotonically.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600))
        policy.recordSeen(uuid: b, at: t0) // the anchor

        _ = policy.partitionRestored(candidates: [pending(a)], now: t0)
        XCTAssertNil(store.records[a.uuidString])
    }

    func testPartitionSweepsExpiredEntriesNotInCandidateList() {
        // A restore event that only lists {b} still needs to age out {a} if
        // {a} has crossed the TTL. Restoration is our natural sweep trigger.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600)) // stale
        policy.recordSeen(uuid: b, at: t0)                            // fresh

        _ = policy.partitionRestored(candidates: [pending(b)], now: t0)

        XCTAssertNil(store.records[a.uuidString])
        XCTAssertEqual(store.records[b.uuidString], t0.timeIntervalSince1970)
    }

    func testPartitionKeepsFreshEntriesNotInCandidateList() {
        // Symmetric to the sweep test: an entry that's still within the TTL
        // must be kept even when iOS didn't hand it back this restoration.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0) // fresh, not in candidate list

        _ = policy.partitionRestored(candidates: [], now: t0.addingTimeInterval(10))

        XCTAssertEqual(store.records[a.uuidString], t0.timeIntervalSince1970)
    }

    // MARK: - Capacity cap

    func testExceedingMaxRecordsEvictsOldestOnRecord() {
        // Pathological dev loop: cycle three peripheral UUIDs with maxRecords
        // = 2. Oldest must be evicted at the third record.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, maxRecords: 2)

        policy.recordSeen(uuid: a, at: t0)                     // oldest
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(1))
        policy.recordSeen(uuid: c, at: t0.addingTimeInterval(2)) // triggers eviction

        XCTAssertNil(store.records[a.uuidString])
        XCTAssertNotNil(store.records[b.uuidString])
        XCTAssertNotNil(store.records[c.uuidString])
        XCTAssertEqual(policy.recordedPeripheralCount(), 2)
    }

    func testEvictionIsWrittenThroughEvenWhenThrottled() {
        // An eviction that stayed in memory would be undone by the next
        // launch's load, so it forces a flush regardless of the interval.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, maxRecords: 2, persistIntervalSeconds: 3600)

        policy.recordSeen(uuid: a, at: t0)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(1))
        policy.recordSeen(uuid: c, at: t0.addingTimeInterval(2))

        XCTAssertNil(store.records[a.uuidString])
        XCTAssertEqual(store.records.count, 2)
    }

    // MARK: - Defaults

    func testDefaultTtlIsSixtySeconds() {
        XCTAssertEqual(PeripheralRestorationAgeOutPolicy.defaultTtlSeconds, 60)
    }

    func testDefaultMaxRecordsIsTwoHundred() {
        XCTAssertEqual(PeripheralRestorationAgeOutPolicy.defaultMaxRecords, 200)
    }

    func testDefaultPersistIntervalIsTenSeconds() {
        XCTAssertEqual(PeripheralRestorationAgeOutPolicy.defaultPersistIntervalSeconds, 10)
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

    func testUserDefaultsStoreSkipsNonNumericValues() {
        // The blob is app-writable and survives upgrades, so a value that is
        // not a number must be dropped rather than crash the loader or land in
        // the map as garbage that then anchors the cutoff.
        let suiteName = "PeripheralRestorationAgeOutPolicyTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defer {
            defaults.removePersistentDomain(forName: suiteName)
        }
        defaults.set(
            [a.uuidString: 1_700_000_000, b.uuidString: "not a timestamp"] as [String: Any],
            forKey: "test-key"
        )

        let store = UserDefaultsPeripheralRestorationStore(
            userDefaults: defaults,
            key: "test-key"
        )

        XCTAssertEqual(store.loadRestorationRecords(), [a.uuidString: 1_700_000_000])
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
