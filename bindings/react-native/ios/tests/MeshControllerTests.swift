import XCTest
@testable import OfflineProtocol

final class MeshControllerTests: XCTestCase {

    func testEvictsLowScorePeerForBetterCandidate() {
        // Two connections fill a mesh of two, so an outbound decision has to
        // weigh a swap. The default of four left free slots, the decision came
        // back `capacity_available`, and the eviction path was never reached.
        // The Kotlin twin was repaired in #120; this suite ran nowhere, so
        // nobody saw it fail.
        let controller = MeshController(selfId: "self", config: .init(maxConnections: 2))
        controller.updateSelfMetrics(
            .init(
                rssi: -50,
                batteryPercent: 85,
                signalQuality: 80,
                stability: 0.8,
                uptimeSeconds: 300,
                loadPercent: 15
            )
        )

        controller.registerConnection(peerId: "anchor", role: .member)
        controller.updatePeerMetrics(
            peerId: "anchor",
            metrics: .init(
                rssi: -60,
                batteryPercent: 75,
                signalQuality: 70,
                stability: 0.7,
                uptimeSeconds: 600,
                loadPercent: 25
            )
        )

        controller.registerConnection(peerId: "weak", role: .member)
        controller.updatePeerMetrics(
            peerId: "weak",
            metrics: .init(
                rssi: -95,
                batteryPercent: 10,
                signalQuality: 20,
                stability: 0.1,
                uptimeSeconds: 5,
                loadPercent: 95
            )
        )

        let metadata = MeshAdvertisementData(
            degree: 1,
            freeSlotEstimate: 3,
            nodeScore: 0.92,
            uptimeSeconds: 600,
            batteryPercent: 95,
            loadPercent: 10,
            rssiToYou: -25,
            nodeIdHash: 84
        )

        let decision = controller.shouldInitiateOutbound(metadata: metadata, rssi: -35)

        XCTAssertEqual(decision.intent, .intraCluster)
        XCTAssertEqual(decision.evictPeerId, "weak")
        // Won on score. Without this the test passes on a bridge swap too.
        XCTAssertEqual(decision.reason, "swap_low_score_peer")
    }

    func testBridgeSwapWhenAvailabilityImproves() {
        // A mesh of two, so it is full and a swap is weighed.
        let controller = MeshController(selfId: "self", config: .init(maxConnections: 2))
        controller.updateSelfMetrics(
            .init(
                rssi: -45,
                batteryPercent: 90,
                signalQuality: 85,
                stability: 0.9,
                uptimeSeconds: 900,
                loadPercent: 10
            )
        )

        // A strong incumbent, which must not be the one evicted.
        controller.registerConnection(peerId: "anchor", role: .member)
        controller.updatePeerMetrics(
            peerId: "anchor",
            metrics: .init(
                rssi: -50,
                batteryPercent: 85,
                signalQuality: 80,
                stability: 0.85,
                uptimeSeconds: 1_200,
                loadPercent: 20
            )
        )

        // The weakest peer, the one a bridge swap should evict. The test used
        // to give "weak" the best metrics of the three, so nothing could have
        // evicted it.
        controller.registerConnection(peerId: "weak", role: .member)
        controller.updatePeerMetrics(
            peerId: "weak",
            metrics: .init(
                rssi: -80,
                batteryPercent: 45,
                signalQuality: 40,
                stability: 0.45,
                uptimeSeconds: 500,
                loadPercent: 55
            )
        )

        // A candidate that scores about the same as "weak", so it does not win
        // on score (that path answers `swap_low_score_peer`), but advertises
        // capacity the full incumbent lacks, so it wins as a bridge.
        let metadata = MeshAdvertisementData(
            degree: 2,
            freeSlotEstimate: 1,
            nodeScore: 0.30,
            uptimeSeconds: 600,
            batteryPercent: 50,
            loadPercent: 50,
            rssiToYou: -65,
            nodeIdHash: 84
        )

        let decision = controller.shouldInitiateOutbound(metadata: metadata, rssi: -70)

        XCTAssertEqual(decision.intent, .intraCluster)
        XCTAssertEqual(decision.evictPeerId, "weak")
        XCTAssertEqual(decision.reason, "swap_bridge_capacity")
    }

    func testDroppingEveryLinkFreesEverySlot() {
        // Bluetooth off drops every link with no disconnect callback per link.
        // A mesh still counting them comes back full and refuses its peers.
        let controller = MeshController(selfId: "self", config: .init(maxConnections: 2))
        controller.registerConnection(peerId: "a", role: .member)
        controller.registerConnection(peerId: "b", role: .member)
        XCTAssertFalse(controller.connectionBudgetAvailable())

        controller.registerAllDisconnected()

        XCTAssertTrue(controller.connectionBudgetAvailable())
        XCTAssertEqual(controller.advertisement().freeSlotEstimate, 2)
    }
}

