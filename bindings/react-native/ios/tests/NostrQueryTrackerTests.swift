import XCTest
@testable import OfflineProtocol

/// The completion rule for a broadcast resolution query.
///
/// Mirrors android/.../NostrQueryTrackerTest.kt case for case. The behaviour
/// under test is a correctness property rather than a latency one: a username
/// claim is meant to need only one honest relay to survive, so a query that
/// completed on the first end-of-stored-events would hand the whole answer to
/// whichever relay was fastest, and a relay holding nothing wins that race by
/// having nothing to send.
final class NostrQueryTrackerTests: XCTestCase {

    private let relayA = "wss://a.example"
    private let relayB = "wss://b.example"
    private let relayC = "wss://c.example"

    /// **The finding this class exists for.**
    ///
    /// The first relay's EOSE must not complete the query. If it did, every
    /// record the slower relays are still sending would be discarded, which for
    /// a username resolution is the answer itself.
    func testFirstEndOfStoredEventsDoesNotCompleteABroadcastQuery() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA, relayB, relayC], nowMs: 0)

        XCTAssertFalse(
            tracker.noteEndOfStoredEvents("q1", from: relayA),
            "one relay answering is not the whole answer"
        )
        XCTAssertFalse(tracker.noteEndOfStoredEvents("q1", from: relayB))
        XCTAssertTrue(
            tracker.noteEndOfStoredEvents("q1", from: relayC),
            "the last relay owed completes it"
        )
        XCTAssertFalse(tracker.isActive("q1"), "and it is no longer in flight")
    }

    /// A single-relay query completes on that relay's EOSE.
    func testAQuerySentToOneRelayCompletesOnItsAnswer() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA], nowMs: 0)
        XCTAssertTrue(tracker.noteEndOfStoredEvents("q1", from: relayA))
    }

    /// Events under an active subscription are resolution records and go to a
    /// different entry point than inbound messages.
    func testOnlyIssuedQueriesAreActive() {
        let tracker = NostrQueryTracker()
        XCTAssertFalse(tracker.isActive("q1"))
        tracker.issue("q1", relays: [relayA], nowMs: 0)
        XCTAssertTrue(tracker.isActive("q1"))
    }

    /// A relay that went away will never send its EOSE, so it must stop being
    /// waited on or the query sits until its deadline for an answer that cannot
    /// arrive.
    func testADisconnectingRelayStopsBeingAwaitedAndCanCompleteAQuery() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA, relayB], nowMs: 0)
        _ = tracker.noteEndOfStoredEvents("q1", from: relayA)

        XCTAssertEqual(
            tracker.dropRelay(relayB), ["q1"],
            "dropping the last relay owed finishes the query"
        )
        XCTAssertFalse(tracker.isActive("q1"))
    }

    /// A disconnect that still leaves relays owed must not complete anything.
    func testADisconnectingRelayLeavesAQueryThatOthersStillOwe() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA, relayB], nowMs: 0)

        XCTAssertTrue(tracker.dropRelay(relayA).isEmpty)
        XCTAssertTrue(tracker.isActive("q1"), "still waiting on the other relay")
        XCTAssertTrue(tracker.noteEndOfStoredEvents("q1", from: relayB))
    }

    /// One disconnect settles every query that relay owed, and only those.
    func testADisconnectingRelayIsDroppedFromEveryQueryItOwed() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA], nowMs: 0)
        tracker.issue("q2", relays: [relayA], nowMs: 0)
        tracker.issue("q3", relays: [relayB], nowMs: 0)

        XCTAssertEqual(Set(tracker.dropRelay(relayA)), ["q1", "q2"])
        XCTAssertTrue(tracker.isActive("q3"), "a query that never asked this relay is untouched")
    }

    /// A relay is free never to send EOSE, so a query with no deadline holds its
    /// subscription for the life of the connection while its caller waits on the
    /// engine's much later sweep.
    func testASilentRelayExpiresTheQueryAtTheDeadline() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA], nowMs: 1_000)

        XCTAssertTrue(
            tracker.staleQueries(nowMs: 1_000 + NostrQueryTracker.COMPLETION_TIMEOUT_MS).isEmpty,
            "not stale before the deadline"
        )
        XCTAssertEqual(
            tracker.staleQueries(nowMs: 1_000 + NostrQueryTracker.COMPLETION_TIMEOUT_MS + 1),
            ["q1"]
        )
    }

    /// Expiry is non-destructive: the caller still has to finish the query, which
    /// is what sends CLOSE to the relays that never answered.
    func testExpiryReportsWithoutRemovingSoFinishingStaysOnePath() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA, relayB], nowMs: 0)
        _ = tracker.noteEndOfStoredEvents("q1", from: relayA)

        XCTAssertEqual(
            tracker.staleQueries(nowMs: NostrQueryTracker.COMPLETION_TIMEOUT_MS + 1),
            ["q1"]
        )
        XCTAssertTrue(tracker.isActive("q1"), "still in flight until finished")

        XCTAssertEqual(
            tracker.finish("q1"), Set([relayB]),
            "only the relay that never answered is still owed a CLOSE"
        )
        XCTAssertFalse(tracker.isActive("q1"))
    }

    /// Finishing an unknown or already-finished query is a no-op.
    func testFinishingAQueryTwiceReleasesItOnce() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA], nowMs: 0)

        XCTAssertEqual(tracker.finish("q1"), Set([relayA]))
        XCTAssertNil(tracker.finish("q1"), "a second finish must not release it again")
        XCTAssertNil(tracker.finish("never-issued"))
    }

    /// A duplicate or stray EOSE must not complete a query. Reporting completion
    /// twice would hand the transport the same query id twice, and the second
    /// release can land after a later query reused the entry.
    func testADuplicateOrUnknownEndOfStoredEventsCompletesNothing() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA, relayB], nowMs: 0)

        XCTAssertFalse(tracker.noteEndOfStoredEvents("q1", from: relayA))
        XCTAssertFalse(
            tracker.noteEndOfStoredEvents("q1", from: relayA),
            "the same relay answering twice adds nothing"
        )
        XCTAssertFalse(
            tracker.noteEndOfStoredEvents("q1", from: relayC),
            "a relay never asked adds nothing"
        )
        XCTAssertTrue(tracker.isActive("q1"), "still owed by the relay that has not answered")

        XCTAssertTrue(tracker.noteEndOfStoredEvents("q1", from: relayB))
        XCTAssertFalse(
            tracker.noteEndOfStoredEvents("q1", from: relayA),
            "an EOSE for a finished query completes nothing"
        )
    }

    /// When the relays are gone entirely nothing will ever answer, so every
    /// query is released rather than pinning subscription state.
    func testClearReleasesEveryQuery() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA], nowMs: 0)
        tracker.issue("q2", relays: [relayB], nowMs: 0)

        XCTAssertEqual(Set(tracker.clear()), ["q1", "q2"])
        XCTAssertFalse(tracker.isActive("q1"))
        XCTAssertFalse(tracker.isActive("q2"))
        XCTAssertTrue(tracker.clear().isEmpty, "a second clear has nothing to release")
    }

    /// A query is recorded against the relays its REQ actually went to, so a
    /// relay that connects later is never waited on for an answer it was never
    /// asked for.
    func testARelayThatWasNeverAskedCannotHoldAQueryOpen() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA], nowMs: 0)

        XCTAssertTrue(
            tracker.dropRelay(relayC).isEmpty,
            "a relay outside the query completes nothing by disconnecting"
        )
        XCTAssertTrue(tracker.noteEndOfStoredEvents("q1", from: relayA))
    }

    /// Re-issuing under the same id restarts the query rather than merging.
    func testReIssuingAQueryIdReplacesItsProgress() {
        let tracker = NostrQueryTracker()
        tracker.issue("q1", relays: [relayA], nowMs: 0)
        tracker.issue("q1", relays: [relayB], nowMs: 0)

        XCTAssertFalse(
            tracker.noteEndOfStoredEvents("q1", from: relayA),
            "the replaced query's relay is no longer owed"
        )
        XCTAssertTrue(tracker.noteEndOfStoredEvents("q1", from: relayB))
    }
}
