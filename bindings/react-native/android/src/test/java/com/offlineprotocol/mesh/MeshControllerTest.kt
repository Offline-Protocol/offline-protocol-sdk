package com.offlineprotocol.mesh

import org.junit.Assert.assertEquals
import org.junit.Test

class MeshControllerTest {

    @Test
    fun `should evict low score peer when candidate better`() {
        // maxConnections = 2 saturates the mesh with the two registered peers so
        // shouldInitiateOutbound evaluates a swap instead of returning
        // capacity_available. The default max of 4 leaves free slots for two
        // peers, so this test — added in 542e547 alongside that default and never
        // run in CI until now — never actually exercised the eviction path.
        val controller = MeshController("self", MeshController.MeshConfig(maxConnections = 2))
        controller.updateSelfMetrics(
            MeshController.PeerMetrics(
                rssi = -55,
                batteryPercent = 80,
                stability = 0.8,
                loadPercent = 10,
                uptimeSeconds = 120
            )
        )

        controller.registerConnection("anchor", MeshController.MeshRole.MEMBER)
        controller.updatePeerMetrics(
            "anchor",
            MeshController.PeerMetrics(
                rssi = -60,
                batteryPercent = 70,
                stability = 0.7,
                loadPercent = 30,
                uptimeSeconds = 400
            )
        )

        controller.registerConnection("weak", MeshController.MeshRole.MEMBER)
        controller.updatePeerMetrics(
            "weak",
            MeshController.PeerMetrics(
                rssi = -95,
                batteryPercent = 15,
                stability = 0.1,
                loadPercent = 90,
                uptimeSeconds = 10
            )
        )

        val metadata = MeshAdvertisementData(
            degree = 1,
            freeSlotEstimate = 3,
            nodeScore = 0.9,
            uptimeSeconds = 500,
            batteryPercent = 90,
            loadPercent = 15,
            rssiToYou = -30,
            nodeIdHash = 42L
        )

        val decision = controller.shouldInitiateOutbound(metadata, rssi = -40)

        assertEquals(MeshController.ConnectionIntent.INTRA_CLUSTER, decision.intent)
        assertEquals("weak", decision.evictPeerId)
    }

    @Test
    fun `should bridge when candidate has availability even if score similar`() {
        // maxConnections = 2 so the mesh is saturated and a swap is evaluated.
        val controller = MeshController("self", MeshController.MeshConfig(maxConnections = 2))
        controller.updateSelfMetrics(
            MeshController.PeerMetrics(
                rssi = -45,
                batteryPercent = 90,
                stability = 0.9,
                loadPercent = 10,
                uptimeSeconds = 900
            )
        )

        // A strong incumbent that must NOT be the eviction target.
        controller.registerConnection("anchor", MeshController.MeshRole.MEMBER)
        controller.updatePeerMetrics(
            "anchor",
            MeshController.PeerMetrics(
                rssi = -50,
                batteryPercent = 85,
                stability = 0.85,
                loadPercent = 20,
                uptimeSeconds = 1_200
            )
        )

        // The genuinely weakest peer — the one a bridge swap should evict. The
        // original test mislabelled the BEST-metric peer as "weak", so it could
        // never have evicted it; these metrics make "weak" actually the
        // lowest-scored active peer.
        controller.registerConnection("weak", MeshController.MeshRole.MEMBER)
        controller.updatePeerMetrics(
            "weak",
            MeshController.PeerMetrics(
                rssi = -80,
                batteryPercent = 45,
                stability = 0.45,
                loadPercent = 55,
                uptimeSeconds = 500
            )
        )

        // Candidate whose overall score is ~equal to "weak" (so it does NOT win
        // on score alone — that path is swap_low_score_peer) but which advertises
        // spare capacity the saturated incumbent lacks, so it wins as a bridge.
        val metadata = MeshAdvertisementData(
            degree = 2,
            freeSlotEstimate = 1,
            nodeScore = 0.30,
            uptimeSeconds = 600,
            batteryPercent = 50,
            loadPercent = 50,
            rssiToYou = -65,
            nodeIdHash = 84L
        )

        val decision = controller.shouldInitiateOutbound(metadata, rssi = -70)

        assertEquals(MeshController.ConnectionIntent.INTRA_CLUSTER, decision.intent)
        assertEquals("weak", decision.evictPeerId)
        assertEquals("swap_bridge_capacity", decision.reason)
    }
}

