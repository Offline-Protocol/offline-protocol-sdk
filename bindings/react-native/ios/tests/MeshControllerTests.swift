import XCTest
@testable import OfflineProtocol

final class MeshControllerTests: XCTestCase {

    func testEvictsLowScorePeerForBetterCandidate() {
        let controller = MeshController(selfId: "self")
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
    }

    func testBridgeSwapWhenAvailabilityImproves() {
        let controller = MeshController(selfId: "self")
        controller.updateSelfMetrics(
            .init(
                rssi: -45,
                batteryPercent: 90,
                signalQuality: 85,
                stability: 0.9,
                uptimeSeconds: 900,
                loadPercent: 15
            )
        )

        controller.registerConnection(peerId: "anchor", role: .member)
        controller.updatePeerMetrics(
            peerId: "anchor",
            metrics: .init(
                rssi: -55,
                batteryPercent: 80,
                signalQuality: 75,
                stability: 0.85,
                uptimeSeconds: 1_200,
                loadPercent: 20
            )
        )

        controller.registerConnection(peerId: "weak", role: .member)
        controller.updatePeerMetrics(
            peerId: "weak",
            metrics: .init(
                rssi: -40,
                batteryPercent: 95,
                signalQuality: 90,
                stability: 0.95,
                uptimeSeconds: 1_800,
                loadPercent: 10
            )
        )

        let metadata = MeshAdvertisementData(
            degree: 1,
            freeSlotEstimate: 4,
            nodeScore: 0.48,
            uptimeSeconds: 600,
            batteryPercent: 80,
            loadPercent: 25,
            rssiToYou: -25,
            nodeIdHash: 128
        )

        let decision = controller.shouldInitiateOutbound(metadata: metadata, rssi: -55)

        XCTAssertEqual(decision.intent, .intraCluster)
        XCTAssertEqual(decision.evictPeerId, "weak")
        XCTAssertEqual(decision.reason, "swap_bridge_capacity")
    }
}

