package com.offlineprotocol.ble

/**
 * Decides when a client link that keeps taking our writes has never delivered
 * one.
 *
 * A Message write goes out without response, so the stack reports success
 * whether or not the peripheral can do anything with the bytes. Most frames
 * the core sends ask for an acknowledgement, so a peer that can attribute our
 * writes answers within its acknowledgement timeout: over a notification on
 * this link, or over a write to our server. A link is silent when nothing
 * from the peer has arrived on any link since it came up, and a write at
 * least [ackWaitMs] after the first one (the core's resend, or another
 * message) has itself gone [ackWaitMs] unanswered. That is never sooner than
 * twice [ackWaitMs] after the first write.
 *
 * Both writes are needed. The last frame of an exchange is often an
 * acknowledgement, which nothing answers, so one unanswered write proves
 * nothing; a write the peer has not had [ackWaitMs] to answer proves nothing
 * either.
 *
 * A link the peer has answered once is never judged again. From then on the
 * peer can attribute what this side writes, and a peer that stops answering is
 * the core's business: it holds acknowledgements back for a session it lacks,
 * and on two phones a link torn down for that came back, was answered once,
 * and was torn down again every 30 to 40 seconds.
 *
 * A peer is redialled for silence once until it is heard from again. Some
 * frames ask for no acknowledgement (ephemeral frames, relay hints), so a peer
 * that only receives those, and has nothing of its own to send, never answers
 * a link that works. Judged per link alone, each redial starts unjudged and
 * would be torn down again 25 to 30 seconds later, for as long as the stream
 * lasts. A silent link of a peer already redialled is no longer judged.
 *
 * Keyed by link rather than by address, so a new link at the same address
 * starts unjudged. BLE-thread only; it holds no lock.
 */
internal class SilentLinkPolicy<K : Any, P : Any>(private val ackWaitMs: Long = ACK_WAIT_MS) {
    private class Window(val firstWriteAt: Long, var resendAt: Long? = null)

    private val windows = HashMap<K, Window>()
    private val answered = HashSet<K>()
    private val redialled = HashSet<P>()

    /** The link took a Message write at [now]. */
    fun noteWrite(link: K, now: Long) {
        if (link in answered) return
        val window = windows.getOrPut(link) { Window(now) }
        if (window.resendAt == null && now - window.firstWriteAt >= ackWaitMs) {
            window.resendAt = now
        }
    }

    /** Something from the peer arrived while [link] was up. */
    fun noteHeard(link: K) {
        windows.remove(link)
        answered.add(link)
    }

    /** Something from [peer] arrived, on any link: it may be redialled for silence again. */
    fun notePeerHeard(peer: P) {
        redialled.remove(peer)
    }

    fun forget(link: K) {
        windows.remove(link)
        answered.remove(link)
    }

    /** Forgets everything, redialled peers included. Used by transport stop. */
    fun clear() {
        windows.clear()
        answered.clear()
        redialled.clear()
    }

    /** Drops every link [keep] rejects, so closed links do not accumulate. */
    fun retainAll(keep: (K) -> Boolean) {
        windows.keys.retainAll(keep)
        answered.retainAll(keep)
    }

    /** The links that are silent at [now]. */
    fun silentLinks(now: Long): List<K> =
        windows.filterValues { window ->
            window.resendAt?.let { now - it >= ackWaitMs } ?: false
        }.keys.toList()

    /**
     * The silent links to redial at [now], at most one per peer until that
     * peer is heard from. Each link returned is forgotten; a silent link of a
     * peer already redialled is kept and no longer judged. A link whose peer
     * [peerOf] cannot name is always returned.
     */
    fun linksToRedial(now: Long, peerOf: (K) -> P?): List<K> =
        silentLinks(now).filter { link ->
            val peer = peerOf(link)
            if (peer != null && !redialled.add(peer)) {
                windows.remove(link)
                answered.add(link)
                false
            } else {
                forget(link)
                true
            }
        }

    companion object {
        /** The core's acknowledgement timeout (docs/state-machines/outbox-and-retries.md). */
        const val ACK_WAIT_MS = 10_000L
    }
}
