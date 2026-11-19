package com.offlineprotocol

/**
 * Whether the peer-stream layer is up, told to the core once per edge
 * (docs/bridges/kotlin.md K10, stream-framing.md "Position among the
 * transports"). Kept apart from [WifiDirectManager] so the rule is tested
 * with no framework.
 *
 * The layer is up while a peer stream can be established: while Wi-Fi P2P
 * is on, while this device is on a Wi-Fi network (the LAN carrier), or while
 * a WebRTC Direct listener (ADR 0039) is open. The core holds a link usable
 * only while the layer is up, so a listener that a page dials while neither
 * carrier is up (this phone sharing its hotspot, on no Wi-Fi network and
 * with Wi-Fi P2P off) proved the page and counted it nearby, and then no
 * key package went out and no MLS session ever formed. On a Wi-Fi network
 * the LAN carrier held the layer up and hid it.
 *
 * Going down is reported only after the streams of whatever went are ended
 * ([afterEnding]): each announced peer is reported lost while the core
 * still holds it. Reported down first, the core has already dropped the
 * links, and the losses that follow name peers it no longer has.
 *
 * Confined to the transport thread: [report] is an FFI call, and [carriers]
 * is read there.
 */
internal class PeerStreamLayer(
    private val carriers: () -> Carriers,
    private val report: (Boolean) -> Unit,
) {
    /** What can establish a stream now. */
    data class Carriers(val p2p: Boolean, val lan: Boolean, val listener: Boolean) {
        val up: Boolean get() = p2p || lan || listener
    }

    /** What the core was last told. */
    var reported = false
        private set

    /** Tells the core when [carriers] puts the layer on the other side of what it was last told. */
    fun reportEdge() {
        val up = carriers().up
        if (up == reported) return
        reported = up
        report(up)
    }

    /**
     * Runs [endStreams], which ends the streams of a carrier that went (each
     * announced peer reported lost), and only then reports the layer.
     */
    fun <T> afterEnding(endStreams: () -> T): T {
        try {
            return endStreams()
        } finally {
            reportEdge()
        }
    }

    /** Stop told the core the layer is down itself; the next start reports from there. */
    fun stopped() {
        reported = false
    }
}
