//
// SupersededLatchPolicyTests.swift
// Mirrors android/src/test/java/com/offlineprotocol/SupersededLatchPolicyTest.kt
//

import XCTest
@testable import OfflineProtocol

final class SupersededLatchPolicyTests: XCTestCase {

    func testClose4000LatchesWhenSocketIsCurrent() {
        let policy = SupersededLatchPolicy()
        // handleConnectionClosed reaches the decision only after the task was
        // detached, so hasNewerSuccessor is false there.
        XCTAssertTrue(policy.shouldMark(closeCode: 4000, hasNewerSuccessor: false))
    }

    func testNonSupersedeCloseDoesNotLatch() {
        let policy = SupersededLatchPolicy()
        XCTAssertFalse(policy.shouldMark(closeCode: 1000, hasNewerSuccessor: false))
        XCTAssertFalse(policy.shouldMark(closeCode: -1, hasNewerSuccessor: false))
        XCTAssertFalse(policy.shouldMark(closeCode: nil, hasNewerSuccessor: false))
    }

    func testNewerSuccessorSocketIsNeverLatchedByAStale4000() {
        // The cd9fa39 regression: old socket displaced → app re-enabled via
        // start() → new socket B up → a LATE 4000 for the bygone generation
        // must not re-latch and stop B.
        let policy = SupersededLatchPolicy()
        XCTAssertFalse(policy.shouldMark(closeCode: 4000, hasNewerSuccessor: true))
    }

    func testSuccessorGuardWinsEvenWhenAlreadyLatched() {
        let policy = SupersededLatchPolicy()
        policy.mark()
        // A stale latch bit must still never stop a live successor socket.
        XCTAssertFalse(policy.shouldMark(closeCode: 1000, hasNewerSuccessor: true))
    }

    func testOnceLatchedAnyCloseKeepsLatching() {
        // A non-4000 close arriving after a SessionSuperseded notice already
        // latched (on the live socket) must still stop, not reconnect.
        let policy = SupersededLatchPolicy()
        policy.mark()
        XCTAssertTrue(policy.shouldMark(closeCode: 1000, hasNewerSuccessor: false))
        XCTAssertTrue(policy.shouldMark(closeCode: nil, hasNewerSuccessor: false))
    }

    func testMarkIsIdempotentAndReportsOnlyTheFirstTransition() {
        // The relay emits a notice AND close 4000 (each fanning into several
        // terminal signals); the one-shot event must fire exactly once.
        let policy = SupersededLatchPolicy()
        XCTAssertTrue(policy.mark())   // false -> true: fire the event
        XCTAssertFalse(policy.mark())  // already latched: no re-fire
        XCTAssertFalse(policy.mark())
        XCTAssertTrue(policy.isSuperseded)
    }

    func testStartClearsTheLatchAndReArmsMark() {
        let policy = SupersededLatchPolicy()
        policy.mark()
        XCTAssertTrue(policy.isSuperseded)

        // A fresh start() clears it; a subsequent displacement fires again.
        policy.clear()
        XCTAssertFalse(policy.isSuperseded)
        XCTAssertFalse(policy.shouldMark(closeCode: 1000, hasNewerSuccessor: false))
        XCTAssertTrue(policy.mark())
    }

    func testCloseCodeConstantMatchesRelayContract() {
        XCTAssertEqual(SupersededLatchPolicy.SUPERSEDED_CLOSE_CODE, 4000)
    }

    // MARK: - Event tag, payload and restatement

    func testEventTypeMatchesTheTagAppsMatchOn() {
        // Pinned literally. The two sites that emit this tag live in
        // OfflineProtocolModule.swift, which nothing in CI compiles, and apps
        // switch on the string (src/types.ts InternetSessionSupersededEvent).
        // A drift here is an event nobody receives — with nothing to restate it.
        XCTAssertEqual(SupersededLatchPolicy.EVENT_TYPE, "internet_session_superseded")
    }

    func testEventJsonCarriesTypeAndReason() throws {
        let json = SupersededLatchPolicy.eventJson(reason: "connected elsewhere")
        let parsed = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
        )
        XCTAssertEqual(parsed["type"] as? String, "internet_session_superseded")
        XCTAssertEqual(parsed["reason"] as? String, "connected elsewhere")
    }

    func testEventJsonOmitsReasonWhenAbsent() throws {
        // Omitted, not null: this is the shape both bridges have emitted since
        // 0.16.2, and `reason` is declared optional on the TS event.
        let json = SupersededLatchPolicy.eventJson(reason: nil)
        let parsed = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
        )
        XCTAssertEqual(parsed["type"] as? String, "internet_session_superseded")
        XCTAssertFalse(parsed.keys.contains("reason"))
    }

    func testEventJsonEscapesRelaySuppliedReason() throws {
        // The reason is relay-supplied and reaches JS as a JSON string inside
        // an event envelope. Hand-built JSON would break on these; this pins
        // that it is built by a serializer.
        let hostile = "he said \"hi\"\n\\ tab\there"
        let json = SupersededLatchPolicy.eventJson(reason: hostile)
        let parsed = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
        )
        XCTAssertEqual(parsed["reason"] as? String, hostile)
    }

    func testRestatementIsNilUntilSuperseded() {
        let policy = SupersededLatchPolicy()
        XCTAssertNil(policy.restatementEventJson())
    }

    func testRestatementReportsTheLatchedReason() throws {
        let policy = SupersededLatchPolicy()
        policy.mark(reason: "newer session took the slot")

        let json = try XCTUnwrap(policy.restatementEventJson())
        let parsed = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
        )
        XCTAssertEqual(parsed["type"] as? String, "internet_session_superseded")
        XCTAssertEqual(parsed["reason"] as? String, "newer session took the slot")
    }

    func testRestatementRepeatsForAsLongAsTheTransportIsSuperseded() {
        // The whole point of deriving from state instead of buffering the emit:
        // every foreground while latched restates, so a drop heals on the next
        // one rather than being a single chance that was already spent.
        let policy = SupersededLatchPolicy()
        policy.mark(reason: "displaced")
        XCTAssertNotNil(policy.restatementEventJson())
        XCTAssertNotNil(policy.restatementEventJson())
        XCTAssertNotNil(policy.restatementEventJson())
    }

    func testReasonIsFirstWinsAcrossTheSignalsOfOneDisplacement() {
        // The relay sends a SessionSuperseded notice (which carries the
        // explanation) and then closes 4000 (which does not). Last-wins would
        // overwrite the reason with nothing.
        let policy = SupersededLatchPolicy()
        XCTAssertTrue(policy.mark(reason: "session superseded by newer login"))
        XCTAssertFalse(policy.mark(reason: nil))
        XCTAssertFalse(policy.mark(reason: "a later, different story"))
        XCTAssertEqual(policy.supersedeReason, "session superseded by newer login")
    }

    func testClearDropsTheReasonWithTheLatch() {
        // A reason outliving its latch could only be attached to a *different*
        // displacement, and mark() would refuse to overwrite it.
        let policy = SupersededLatchPolicy()
        policy.mark(reason: "displaced")
        policy.clear()

        XCTAssertNil(policy.supersedeReason)
        XCTAssertNil(policy.restatementEventJson())

        policy.mark(reason: "displaced again")
        XCTAssertEqual(policy.supersedeReason, "displaced again")
    }

    func testReEnableStopsRestatementWithoutAnyDiscardBookkeeping() {
        // Both paths that clear the latch — module start() and
        // enableTransport('internet') — reach InternetManager.start() ->
        // clear(). This is why re-deriving needs no discard site: after either,
        // there is simply nothing to restate.
        let policy = SupersededLatchPolicy()
        policy.mark(reason: "displaced")
        XCTAssertNotNil(policy.restatementEventJson())

        policy.clear()

        XCTAssertNil(policy.restatementEventJson())
    }

    func testMarkWithoutAReasonStillRestates() {
        // The close-4000 path latches with no relay explanation. The report is
        // still the point; only the reason is missing.
        let policy = SupersededLatchPolicy()
        policy.mark()
        XCTAssertEqual(policy.restatementEventJson(), "{\"type\":\"internet_session_superseded\"}")
    }
}
