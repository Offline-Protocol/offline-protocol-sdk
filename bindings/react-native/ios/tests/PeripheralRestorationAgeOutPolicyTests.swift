//
// PeripheralRestorationAgeOutPolicyTests.swift
//
// Pins the age-out gate BleManager applies to peripherals iOS hands back
// via `centralManager(_:willRestoreState:)`. See
// PeripheralRestorationAgeOutPolicy.swift for the rule the fixtures below
// exercise.
//
// Two axes run through nearly every fixture:
//
//   1. `.connected` vs pending. A live link is never judged on a timestamp.
//   2. `.advertisement` vs `.linkActivity`. Only an advertisement moves the
//      age-out cutoff, because only an advertisement proves the app was
//      scanning. Fixtures that need to move the cutoff say `.advertisement`
//      deliberately; fixtures about a live link say `.linkActivity`.
//

import XCTest
@testable import OfflineProtocol

final class PeripheralRestorationAgeOutPolicyTests: XCTestCase {

    /// In-memory store for the age-out policy so the tests stay hermetic —
    /// we never touch `UserDefaults.standard`.
    private final class InMemoryStore: PeripheralRestorationStore {
        private(set) var state = PeripheralRestorationState()
        private(set) var saveCount = 0

        var records: [String: TimeInterval] { return state.records }
        var lastScanSightingSeconds: TimeInterval? { return state.lastScanSightingSeconds }

        func loadRestorationState() -> PeripheralRestorationState {
            return state
        }

        func saveRestorationState(_ state: PeripheralRestorationState) {
            self.state = state
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

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600), source: .advertisement)

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

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600), source: .advertisement)
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

    func testConnectedRefreshDoesNotMoveTheAnchor() {
        // The connected branch records a sighting for its own peripheral, but
        // a live link says nothing about whether the app could see anything
        // else. If it moved the anchor, the restoration that carries a live
        // link would age out every pending connect alongside it.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        _ = policy.partitionRestored(candidates: [connected(a)], now: t0.addingTimeInterval(3600))

        XCTAssertEqual(store.lastScanSightingSeconds, t0.timeIntervalSince1970)
    }

    func testConnectedAndPendingCandidatesPartitionIndependently() {
        // a: live link, record long expired  -> fresh (state wins)
        // b: pending connect, record expired -> stale
        // c: pending connect, record fresh   -> fresh
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600), source: .advertisement)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(-3600), source: .advertisement)
        policy.recordSeen(uuid: c, at: t0, source: .advertisement)

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

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)

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

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        // `b`'s advertisement is the anchor. It is what makes this a real
        // age-out: the app was still scanning 61 s after it last saw `a`, so
        // `a` went quiet rather than the scan merely having stopped.
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(61), source: .advertisement)

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

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(ttl), source: .advertisement) // the anchor

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

        // Chronological, because the anchor takes the last advertisement
        // written rather than the newest timestamp: the scan callback delivers
        // sightings in order, and a fixture that writes them backwards is
        // asserting against a clock step, not against an age-out.
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(-120), source: .advertisement) // way past TTL
        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        // c is never recorded — never observed this process.

        let partition = policy.partitionRestored(
            candidates: [pending(a), pending(b), pending(c)],
            now: t0.addingTimeInterval(10)
        )
        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(Set(partition.stale), Set([b, c]))
    }

    // MARK: - The cutoff is anchored to the last advertisement

    func testPendingCandidateSurvivesALongDeadPeriodWhenNothingNewerWasSeen() {
        // The app was terminated with `a` in view and iOS relaunched it three
        // hours later. Nothing writes the map while the process is dead, so
        // measuring `a`'s age against the restoration clock would call it
        // stale and cancel its connect request — the only way iOS wakes this
        // app when `a` comes back, since a background scan with no service
        // filter is ignored by the OS. `a` was the last thing this app saw.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)

        let partition = policy.partitionRestored(
            candidates: [pending(a)],
            now: t0.addingTimeInterval(3 * 3600)
        )
        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(partition.stale, [])
    }

    func testAnchorIsTheNewestAdvertisementNotTheRestorationClock() {
        // Same three-hour dead period, but the app went on to see `b`
        // advertise two minutes after it last saw `a`. That is the evidence
        // the clock alone cannot supply: `a` went quiet while the app was
        // still scanning, so it ages out. `b` was the last thing seen, so it
        // stays.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(120), source: .advertisement)

        let partition = policy.partitionRestored(
            candidates: [pending(a), pending(b)],
            now: t0.addingTimeInterval(3 * 3600)
        )
        XCTAssertEqual(partition.fresh, [b])
        XCTAssertEqual(partition.stale, [a])
    }

    func testLinkActivityDoesNotAgeOutAPeerTheAppCouldNotHaveSeen() {
        // The regression this distinction exists for, and the shape of an
        // ordinary backgrounded session.
        //
        // `b` is discovered five seconds in and its connect request is left
        // pending — it walked out of range before the connect completed. The
        // app then goes to the background: the scan stops, and a background
        // scan with no service filter is ignored by the OS anyway, so nothing
        // can be re-sighted from here on. `a`'s link stays up and keeps
        // delivering traffic for an hour. iOS terminates the app and relaunches
        // it later on an event from `a`.
        //
        // If link traffic moved the cutoff, `a` alone would push it an hour
        // past `b`'s sighting and cancel `b`'s connect request — which is the
        // only way iOS would ever wake this app when `b` reappears. One chatty
        // peer would silently disable the wake-up path for every absent one.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(5), source: .advertisement)
        // Backgrounded from ~t0+30. Only link traffic from here.
        for offset in stride(from: 60.0, through: 3600.0, by: 60.0) {
            policy.recordSeen(uuid: a, at: t0.addingTimeInterval(offset), source: .linkActivity)
        }

        let partition = policy.partitionRestored(
            candidates: [pending(a), pending(b)],
            now: t0.addingTimeInterval(2 * 3600)
        )

        XCTAssertEqual(Set(partition.fresh), Set([a, b]))
        XCTAssertEqual(partition.stale, [])
    }

    func testLinkActivityStillRefreshesItsOwnRecord() {
        // The other half of the same rule: link traffic must keep the chatty
        // peer's own record current, so it survives a cutoff that other peers'
        // advertisements have pushed forward. `a` connects at t0 and never
        // advertises again; `c` advertises 10 minutes later and moves the
        // cutoff to t0+540. Only the traffic on `a`'s link keeps it alive.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(590), source: .linkActivity)
        policy.recordSeen(uuid: c, at: t0.addingTimeInterval(600), source: .advertisement)

        let partition = policy.partitionRestored(
            candidates: [pending(a)],
            now: t0.addingTimeInterval(3 * 3600)
        )
        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(partition.stale, [])
    }

    func testLinkActivityAloneNeverEstablishesAnAnchor() {
        // A store whose only sightings are link activity has no anchor, so the
        // cutoff falls back to the newest record. That fallback must not be
        // reached by way of `.linkActivity` pretending to be a sighting: the
        // anchor field stays nil.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0, source: .linkActivity)

        XCTAssertNil(store.lastScanSightingSeconds)
        XCTAssertEqual(store.records[a.uuidString], t0.timeIntervalSince1970)
    }

    func testDevLoopUuidChurnStillAgesOutTheDeadUuids() {
        // The case the class exists for, under the anchored cutoff. Every
        // relay restart mints a fresh peripheral UUID that is DISCOVERED —
        // an advertisement, so it becomes the new anchor and pushes its
        // predecessors past the cutoff. The OS-side connect queue stays
        // bounded even though no restoration ever consults the wall clock.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)                         // relay run 1
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(300), source: .advertisement) // run 2
        policy.recordSeen(uuid: c, at: t0.addingTimeInterval(600), source: .advertisement) // run 3

        let partition = policy.partitionRestored(
            candidates: [pending(a), pending(b), pending(c)],
            now: t0.addingTimeInterval(3 * 3600)
        )
        XCTAssertEqual(partition.fresh, [c])
        XCTAssertEqual(Set(partition.stale), Set([a, b]))
    }

    func testAnchorFallsBackToTheNewestRecordWhenNoneWasEverPersisted() {
        // Migration: a map written by a build that predates the anchor loads
        // with no anchor at all. The cutoff falls back to the newest record,
        // which reproduces the previous behaviour for that one restoration
        // rather than defaulting to `now` and cancelling everything.
        let store = InMemoryStore()
        store.saveRestorationState(
            PeripheralRestorationState(
                records: [
                    a.uuidString: t0.timeIntervalSince1970,
                    b.uuidString: t0.addingTimeInterval(120).timeIntervalSince1970
                ],
                lastScanSightingSeconds: nil
            )
        )
        let policy = makePolicy(store: store)

        let partition = policy.partitionRestored(
            candidates: [pending(a), pending(b)],
            now: t0.addingTimeInterval(3 * 3600)
        )
        XCTAssertEqual(partition.fresh, [b])
        XCTAssertEqual(partition.stale, [a])
    }

    func testConnectedRefreshDoesNotTightenTheCutoffForPendingCandidates() {
        // In the no-anchor fallback the cutoff is derived from `records`, and
        // the connected branch writes `now` into `records`. Reading the anchor
        // after that instead of before would let a live link drag the cutoff
        // forward to `now - ttl` and age out pending connects the map says
        // were seen alongside it.
        let store = InMemoryStore()
        store.saveRestorationState(
            PeripheralRestorationState(
                records: [
                    a.uuidString: t0.addingTimeInterval(-100).timeIntervalSince1970,
                    c.uuidString: t0.addingTimeInterval(-90).timeIntervalSince1970
                ],
                lastScanSightingSeconds: nil
            )
        )
        let policy = makePolicy(store: store)

        let partition = policy.partitionRestored(
            candidates: [connected(c), pending(a)],
            now: t0
        )

        XCTAssertEqual(Set(partition.fresh), Set([a, c]))
        XCTAssertEqual(partition.stale, [])
    }

    func testFutureAnchorDoesNotAgeOutCurrentEntries() {
        // An anchor written before a backwards clock step sits in the future.
        // Anchoring to it would put the cutoff ahead of every honest entry and
        // age out the whole map at once, so the anchor is clamped to `now`.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(3600), source: .advertisement)

        let partition = policy.partitionRestored(candidates: [pending(a)], now: t0)

        XCTAssertEqual(partition.fresh, [a])
        XCTAssertEqual(partition.stale, [])
    }

    func testBackwardsClockStepLowersTheAnchorRatherThanPinningIt() {
        // Last-write-wins, not `max`. After a backwards step the anchor moves
        // back with the clock, which moves the cutoff EARLIER and lets more
        // records survive. Taking the maximum would leave a future-dated
        // reading pinning the cutoff until the clock caught up.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(3600), source: .advertisement)
        policy.recordSeen(uuid: b, at: t0, source: .advertisement)

        XCTAssertEqual(store.lastScanSightingSeconds, t0.timeIntervalSince1970)
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

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600), source: .advertisement) // stale
        policy.recordSeen(uuid: a, at: t0, source: .advertisement)                            // fresh now

        let partition = policy.partitionRestored(candidates: [pending(a)], now: t0)
        XCTAssertEqual(partition.fresh, [a])
    }

    // MARK: - Persistence

    func testRecordSeenPersistsToStore() {
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)

        XCTAssertEqual(store.records[a.uuidString], t0.timeIntervalSince1970)
        XCTAssertEqual(store.lastScanSightingSeconds, t0.timeIntervalSince1970)
    }

    func testNewPolicyLoadsPersistedRecords() {
        // A second policy instance built against the same store must inherit
        // everything the first one persisted — this is the whole point of the
        // class: state that survives an app relaunch.
        let store = InMemoryStore()
        let policy1 = makePolicy(store: store)
        policy1.recordSeen(uuid: a, at: t0, source: .advertisement)

        let policy2 = makePolicy(store: store)
        let partition = policy2.partitionRestored(
            candidates: [pending(a)],
            now: t0.addingTimeInterval(10)
        )
        XCTAssertEqual(partition.fresh, [a])
    }

    func testNewPolicyInheritsTheAnchorNotJustTheRecords() {
        // The anchor has to survive the relaunch too. A policy that reloaded
        // the records but dropped the anchor would fall back to the newest
        // record, which is the behaviour the anchor exists to replace: here
        // `a`'s link traffic is newer than `b`'s advertisement, and reading it
        // as the anchor would age `b` out.
        let store = InMemoryStore()
        let policy1 = makePolicy(store: store)
        policy1.recordSeen(uuid: b, at: t0, source: .advertisement)
        policy1.recordSeen(uuid: a, at: t0.addingTimeInterval(3600), source: .linkActivity)

        let policy2 = makePolicy(store: store)
        let partition = policy2.partitionRestored(
            candidates: [pending(b)],
            now: t0.addingTimeInterval(4 * 3600)
        )
        XCTAssertEqual(partition.fresh, [b])
        XCTAssertEqual(partition.stale, [])
    }

    // MARK: - Write-through throttle

    func testFirstSightingOfAUuidAlwaysPersistsImmediately() {
        // A new peripheral must reach the store even if another write just
        // happened: losing a first sighting to a termination costs a
        // cancelled reconnect on the next restoration.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, persistIntervalSeconds: 10)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        XCTAssertEqual(store.saveCount, 1)

        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(1), source: .advertisement)
        XCTAssertEqual(store.saveCount, 2)
    }

    func testRepeatSightingsWithinTheIntervalDoNotHitTheStore() {
        // `recordSeen` runs from the scan callback on the main queue, roughly
        // once a second per visible peer. Each save serialises the whole map
        // through UserDefaults, so repeats inside the interval are dropped.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, persistIntervalSeconds: 10)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        XCTAssertEqual(store.saveCount, 1)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(1), source: .advertisement)
        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(5), source: .advertisement)
        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(9), source: .advertisement)
        XCTAssertEqual(store.saveCount, 1)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(10), source: .advertisement)
        XCTAssertEqual(store.saveCount, 2)
        XCTAssertEqual(store.records[a.uuidString], t0.addingTimeInterval(10).timeIntervalSince1970)
    }

    func testThrottledRecordAndAnchorFlushTogether() {
        // The record and the anchor it will be compared against are written in
        // one snapshot, so the staleness the throttle allows applies to both
        // sides of that comparison instead of skewing it.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, persistIntervalSeconds: 10)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(5), source: .advertisement) // throttled
        XCTAssertEqual(store.records[a.uuidString], t0.timeIntervalSince1970)
        XCTAssertEqual(store.lastScanSightingSeconds, t0.timeIntervalSince1970)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(12), source: .advertisement)
        XCTAssertEqual(store.records[a.uuidString], t0.addingTimeInterval(12).timeIntervalSince1970)
        XCTAssertEqual(store.lastScanSightingSeconds, t0.addingTimeInterval(12).timeIntervalSince1970)
    }

    func testThrottledSightingIsStillCurrentInMemory() {
        // The throttle may delay the write-through; it must never delay the
        // decision. A sighting dropped by the throttle still counts against
        // the TTL on the next restoration.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, persistIntervalSeconds: 10)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(5), source: .advertisement) // throttled
        // `b` anchors the cutoff at t0. The first sighting alone sits exactly
        // on it and would age out; the throttled one is 5 s inside it.
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(60), source: .advertisement)

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

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(1), source: .advertisement) // throttled
        XCTAssertEqual(store.saveCount, 1)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-100), source: .advertisement)
        XCTAssertEqual(store.saveCount, 2)
    }

    func testPartitionAlwaysPersists() {
        // Restoration is rare and its side effects (stale eviction, connected
        // refresh) must be durable immediately: the process may be terminated
        // again at any moment.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, persistIntervalSeconds: 3600)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
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

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600), source: .advertisement)
        policy.recordSeen(uuid: b, at: t0, source: .advertisement) // the anchor

        _ = policy.partitionRestored(candidates: [pending(a)], now: t0)
        XCTAssertNil(store.records[a.uuidString])
    }

    func testPartitionSweepsExpiredEntriesNotInCandidateList() {
        // A restore event that only lists {b} still needs to age out {a} if
        // {a} has crossed the TTL. Restoration is our natural sweep trigger.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0.addingTimeInterval(-3600), source: .advertisement) // stale
        policy.recordSeen(uuid: b, at: t0, source: .advertisement)                            // fresh

        _ = policy.partitionRestored(candidates: [pending(b)], now: t0)

        XCTAssertNil(store.records[a.uuidString])
        XCTAssertEqual(store.records[b.uuidString], t0.timeIntervalSince1970)
    }

    func testPartitionKeepsFreshEntriesNotInCandidateList() {
        // Symmetric to the sweep test: an entry that's still within the TTL
        // must be kept even when iOS didn't hand it back this restoration.
        let store = InMemoryStore()
        let policy = makePolicy(store: store)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement) // fresh, not in candidate list

        _ = policy.partitionRestored(candidates: [], now: t0.addingTimeInterval(10))

        XCTAssertEqual(store.records[a.uuidString], t0.timeIntervalSince1970)
    }

    // MARK: - Capacity cap

    func testExceedingMaxRecordsEvictsOldestOnRecord() {
        // Pathological dev loop: cycle three peripheral UUIDs with maxRecords
        // = 2. Oldest must be evicted at the third record.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, maxRecords: 2)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)                     // oldest
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(1), source: .advertisement)
        policy.recordSeen(uuid: c, at: t0.addingTimeInterval(2), source: .advertisement) // triggers eviction

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

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(1), source: .advertisement)
        policy.recordSeen(uuid: c, at: t0.addingTimeInterval(2), source: .advertisement)

        XCTAssertNil(store.records[a.uuidString])
        XCTAssertEqual(store.records.count, 2)
    }

    func testEvictionNeverDropsTheAnchor() {
        // The anchor is not a peripheral record and must not be swept with
        // them: evicting it would drop the cutoff back to the newest surviving
        // record, which is the behaviour it replaces.
        let store = InMemoryStore()
        let policy = makePolicy(store: store, maxRecords: 1)

        policy.recordSeen(uuid: a, at: t0, source: .advertisement)
        policy.recordSeen(uuid: b, at: t0.addingTimeInterval(1), source: .linkActivity)

        XCTAssertEqual(store.records.count, 1)
        XCTAssertEqual(store.lastScanSightingSeconds, t0.timeIntervalSince1970)
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

    func testDefaultStoreKeysAreNamespaced() {
        XCTAssertEqual(
            UserDefaultsPeripheralRestorationStore.defaultRecordsKey,
            "mesh.blemanager.peripheralLastSeen.v1"
        )
        XCTAssertEqual(
            UserDefaultsPeripheralRestorationStore.defaultScanSightingKey,
            "mesh.blemanager.lastScanSighting.v1"
        )
    }

    // MARK: - UserDefaults-backed store roundtrip

    /// A suite-scoped UserDefaults keeps these tests hermetic — they never
    /// touch `.standard`.
    private func withTemporaryDefaults(_ body: (UserDefaults) -> Void) {
        let suiteName = "PeripheralRestorationAgeOutPolicyTests.\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suiteName) else {
            XCTFail("could not open a suite-scoped UserDefaults")
            return
        }
        defer { defaults.removePersistentDomain(forName: suiteName) }
        body(defaults)
    }

    private func makeStore(_ defaults: UserDefaults) -> UserDefaultsPeripheralRestorationStore {
        return UserDefaultsPeripheralRestorationStore(
            userDefaults: defaults,
            recordsKey: "test-records",
            scanSightingKey: "test-anchor"
        )
    }

    func testUserDefaultsStoreRoundtripsState() {
        withTemporaryDefaults { defaults in
            let store = makeStore(defaults)
            let state = PeripheralRestorationState(
                records: [
                    a.uuidString: 1_700_000_000,
                    b.uuidString: 1_700_000_030
                ],
                lastScanSightingSeconds: 1_700_000_030
            )
            store.saveRestorationState(state)

            XCTAssertEqual(store.loadRestorationState(), state)
        }
    }

    func testUserDefaultsStoreSkipsNonNumericValues() {
        // The blob is app-writable and survives upgrades, so a value that is
        // not a number must be dropped rather than crash the loader or land in
        // the map as garbage that then anchors the cutoff.
        withTemporaryDefaults { defaults in
            defaults.set(
                [a.uuidString: 1_700_000_000, b.uuidString: "not a timestamp"] as [String: Any],
                forKey: "test-records"
            )

            let loaded = makeStore(defaults).loadRestorationState()
            XCTAssertEqual(loaded.records, [a.uuidString: 1_700_000_000])
            XCTAssertNil(loaded.lastScanSightingSeconds)
        }
    }

    func testUserDefaultsStoreReadsAMissingAnchorAsNilNotZero() {
        // `double(forKey:)` cannot tell "never written" from a stored 0, and 0
        // is a real (1970) timestamp: anchoring to it would put the cutoff at
        // the epoch and keep every record alive forever.
        withTemporaryDefaults { defaults in
            defaults.set([a.uuidString: 1_700_000_000], forKey: "test-records")

            XCTAssertNil(makeStore(defaults).loadRestorationState().lastScanSightingSeconds)
        }
    }

    func testUserDefaultsStoreEmptySaveClearsBothKeys() {
        // Saving an empty state must clear the keys entirely so the blob
        // doesn't sit as `{}` in UserDefaults forever.
        withTemporaryDefaults { defaults in
            let store = makeStore(defaults)
            store.saveRestorationState(
                PeripheralRestorationState(
                    records: [a.uuidString: 1_700_000_000],
                    lastScanSightingSeconds: 1_700_000_000
                )
            )
            XCTAssertNotNil(defaults.object(forKey: "test-records"))
            XCTAssertNotNil(defaults.object(forKey: "test-anchor"))

            store.saveRestorationState(PeripheralRestorationState())
            XCTAssertNil(defaults.object(forKey: "test-records"))
            XCTAssertNil(defaults.object(forKey: "test-anchor"))
        }
    }
}
