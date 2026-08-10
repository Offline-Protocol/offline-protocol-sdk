//
// RelayAnswerPrefixesTests.swift
//
// Pins the "a relay answer reaches the core unattributed" rule against the
// core's RELAY_ANSWER_PREFIXES. Mirrors android's RelayAnswerPrefixesTest —
// keep in sync.
//
// Regression pin: __GROUP_MEMBER_ADDED__ and __GROUP_MEMBER_REMOVED__ were
// injected with the relay-reported actor (added_by / removed_by) as a
// reachability assertion. Once control traffic became unconditionally
// signature-gated that attribution started failing the core's exemption — it
// requires no transport peer identity — so every legitimate relay membership
// notification was dropped as unsigned and raised UNSIGNED_CONTROL_REJECTED.
//

import XCTest
@testable import OfflineProtocol

final class RelayAnswerPrefixesTests: XCTestCase {

    /// The list must match `RELAY_ANSWER_PREFIXES` in
    /// `crates/offline-protocol/src/protocol/prefixes.rs` exactly. A prefix
    /// here that the core does not exempt is dropped as unsigned; a prefix the
    /// core exempts that is missing here gets attributed and dropped the same
    /// way.
    func testListMatchesTheCoreConstant() {
        XCTAssertEqual(
            RelayAnswerPrefixes.all,
            [
                "__GROUP_CREATED__",
                "__GROUP_MEMBER_ADDED__",
                "__GROUP_MEMBER_REMOVED__",
                "__GROUP_INFO__",
                "__USER_GROUPS__",
                "__GROUP_ERROR__"
            ]
        )
    }

    /// The regression itself: these two carried an actor and must not.
    func testMembershipAnswersAreNeverAttributed() {
        for prefix in ["__GROUP_MEMBER_ADDED__", "__GROUP_MEMBER_REMOVED__"] {
            XCTAssertNil(
                RelayAnswerPrefixes.attributableActor(prefix: prefix, actorId: "alice"),
                "\(prefix) must reach the core unattributed or it is dropped as unsigned"
            )
        }
    }

    func testEveryRelayAnswerDropsItsActor() {
        for prefix in RelayAnswerPrefixes.all {
            XCTAssertNil(RelayAnswerPrefixes.attributableActor(prefix: prefix, actorId: "alice"))
            XCTAssertNil(RelayAnswerPrefixes.attributableActor(prefix: prefix, actorId: nil))
        }
    }

    /// The rule is scoped, not blanket. `__GROUP_MSG__` is a data-plane prefix
    /// — never signature-gated, because MLS authenticates it afterwards — so it
    /// keeps its attribution and stays the reachability signal for a relayed
    /// sender. Nulling it here would be a silent regression of that seam.
    func testDataPlaneAndPeerPrefixesKeepTheirActor() {
        for prefix in ["__GROUP_MSG__", "__CONN_REQ__", "__MLS_ENC__"] {
            XCTAssertEqual(
                RelayAnswerPrefixes.attributableActor(prefix: prefix, actorId: "alice"),
                "alice",
                "\(prefix) is not a relay answer and must keep its attribution"
            )
        }
    }

    func testIsRelayAnswerDiscriminates() {
        XCTAssertTrue(RelayAnswerPrefixes.isRelayAnswer("__GROUP_CREATED__"))
        XCTAssertFalse(RelayAnswerPrefixes.isRelayAnswer("__GROUP_MSG__"))
        // Matched whole, not by prefix-of-prefix: a crafted content string
        // starting with an exempt prefix is a different question, decided in
        // the core against the frame's transport and attribution.
        XCTAssertFalse(RelayAnswerPrefixes.isRelayAnswer("__GROUP_CREATED__extra"))
    }
}
