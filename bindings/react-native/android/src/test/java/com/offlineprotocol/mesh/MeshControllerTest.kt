package com.offlineprotocol.mesh

import org.junit.Assert.assertEquals
import org.junit.Test

class MeshControllerTest {

    @Test
    fun `should evict low score peer when candidate better`() {
        val controller = MeshController("self")
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
        val controller = MeshController("self")
        controller.updateSelfMetrics(
            MeshController.PeerMetrics(
                rssi = -45,
                batteryPercent = 90,
                stability = 0.9,
                loadPercent = 10,
                uptimeSeconds = 900
            )
        )

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

        controller.registerConnection("weak", MeshController.MeshRole.MEMBER)
        controller.updatePeerMetrics(
            "weak",
            MeshController.PeerMetrics(
                rssi = -40,
                batteryPercent = 95,
                stability = 0.95,
                loadPercent = 5,
                uptimeSeconds = 1_800
            )
        )

        val metadata = MeshAdvertisementData(
            degree = 1,
            freeSlotEstimate = 4,
            nodeScore = 0.45,
            uptimeSeconds = 600,
            batteryPercent = 80,
            loadPercent = 25,
            rssiToYou = -30,
            nodeIdHash = 84L
        )

        val decision = controller.shouldInitiateOutbound(metadata, rssi = -55)

        assertEquals(MeshController.ConnectionIntent.INTRA_CLUSTER, decision.intent)
        assertEquals("weak", decision.evictPeerId)
        assertEquals("swap_bridge_capacity", decision.reason)
    }
}

