package com.offlineprotocol

import java.io.IOException
import java.net.ServerSocket

/**
 * The one listener that takes every inbound peer stream, Wi-Fi P2P and LAN.
 *
 * A group client knows its owner's address and nothing else, so it dials
 * [GROUP_OWNER_PORT]; a LAN peer resolves the port from the NSD record. A
 * second SDK app on the same phone used to bind the same fixed port, fail,
 * and keep advertising it on the LAN, so every peer that dialled that app
 * reached the first one instead and the second had no inbound streams at
 * all. It now listens wherever it can and advertises that port, and a
 * device that does not hold [GROUP_OWNER_PORT] never creates a group,
 * because the group's clients would reach whichever app does.
 */
internal object PeerStreamListener {
    const val GROUP_OWNER_PORT = 8988

    class Bound(val socket: ServerSocket, val ownsGroupPort: Boolean)

    /**
     * Binds [groupOwnerPort], or any free port when another listener holds
     * it. Null when no port can be bound, which leaves the carrier with no
     * inbound streams: the caller refuses to start rather than advertise one.
     */
    fun bind(groupOwnerPort: Int = GROUP_OWNER_PORT): Bound? {
        try {
            return Bound(ServerSocket(groupOwnerPort), ownsGroupPort = true)
        } catch (_: IOException) {
        }
        return try {
            Bound(ServerSocket(0), ownsGroupPort = false)
        } catch (_: IOException) {
            null
        }
    }
}
