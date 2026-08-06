package com.offlineprotocol

import org.json.JSONObject

/**
 * The relay-displacement ("session superseded") latch, extracted as a pure
 * state machine so it can be unit-tested without a live WebSocket (see
 * SupersededLatchPolicyTest). InternetManager owns the threading (every call
 * is made on main, the single writer); this class owns only the decision and
 * the boolean.
 *
 * The relay closes a displaced connection with WebSocket close code 4000 (a
 * newer registration for the same identity took the slot) and, redundantly,
 * sends an application-level SessionSuperseded notice on the live socket
 * first. Either signal must latch the transport stopped: a blind reconnect
 * would just re-displace the peer socket in a ~1s loop — the fleet-wide
 * eviction storm the relay-displacement rollout guards against. The latch is
 * cleared only by an explicit start() (the app resolved "connected
 * elsewhere" and re-enabled the transport).
 *
 * **This class is also the source of truth for the event that reports the
 * latch**, not just the boolean behind it — the tag, the payload shape and
 * the reason. `internet_session_superseded` is strictly one-shot: nothing
 * restates it and every reconnect route refuses while latched, so an emit JS
 * could not take leaves the app showing a relay connection that is never
 * coming back. Android buffers that emit ([StickyEventBuffer]); iOS re-derives
 * the report from this state on app foreground instead, which needs the
 * *reason* to outlive the emit rather than to be held only as its argument.
 * The two platforms share this class so both spellings of the tag come from
 * one place and one test.
 *
 * Mirrors ios/SupersededLatchPolicy.swift — keep in sync.
 */
class SupersededLatchPolicy {
    companion object {
        /** WebSocket close code the relay uses to signal displacement. */
        const val SUPERSEDED_CLOSE_CODE = 4000

        /**
         * The bridge event tag reporting a relay displacement.
         *
         * Apps match on this string (`src/types.ts`
         * InternetSessionSupersededEvent) and it is also the sticky buffer's
         * collapsing key, so a drift is an event nobody receives with nothing
         * to restate it. Declared here — and asserted literally in the tests
         * on both platforms — because the iOS emit sites live in a file
         * nothing in CI compiles.
         */
        const val EVENT_TYPE = "internet_session_superseded"

        /**
         * The `internet_session_superseded` payload for [reason], as event JSON.
         *
         * The `reason` key is omitted rather than null when absent, matching
         * what both bridges have emitted since 0.16.2.
         */
        fun eventJson(reason: String?): String {
            val payload = JSONObject().put("type", EVENT_TYPE)
            if (reason != null) payload.put("reason", reason)
            return payload.toString()
        }
    }

    // Main-owned like the InternetManager flags it replaces (autoReconnect):
    // written on main, read best-effort off-main (getMetrics). @Volatile for
    // the same defensive cross-thread visibility the field it replaced had.
    @Volatile
    private var latched = false

    // The reason carried by the displacement that latched, kept for as long as
    // the latch so a report can be re-derived from state rather than replayed.
    // First-wins with [latched] — see [mark].
    @Volatile
    private var latchedReason: String? = null

    val isSuperseded: Boolean get() = latched

    /** The reason the current latch was taken, if the displacement carried one. */
    val supersedeReason: String? get() = latchedReason

    /**
     * Pure decision: does a close on some socket displace the transport?
     *
     * @param closeCode the WebSocket close code, or null for a local / error
     *   terminal (ping/auth/send-failure teardown) that carries none.
     * @param hasNewerSuccessor a *different*, live socket has already replaced
     *   the one this close belongs to. A late 4000 for a bygone generation
     *   (old socket displaced → app re-enabled via start() → new socket B up)
     *   must NOT re-latch and stop B — the sibling-race the cd9fa39 fix
     *   targets. The Android close funnel's identity guard runs before the
     *   decision, so a successor never reaches it: callers there pass false.
     *   (The parameter exists so the iOS bridge, which keys on the close code
     *   before its identity guard, shares this exact decision.)
     *
     * An already-latched connection stays latched regardless of close code (a
     * non-4000 close after a SessionSuperseded notice still stops).
     */
    fun shouldMark(closeCode: Int?, hasNewerSuccessor: Boolean): Boolean {
        if (hasNewerSuccessor) return false
        return latched || closeCode == SUPERSEDED_CLOSE_CODE
    }

    /**
     * Latches superseded, retaining [reason] for later restatement. Returns
     * true only on the false→true transition, so the caller fires the one-shot
     * event exactly once even though the relay emits both a notice and a close
     * (each of which fans into several terminal signals) for a single
     * displacement.
     *
     * The reason is stored on that same transition and not after it: the later
     * signals for one displacement are the close-code paths, which carry no
     * relay explanation, so a last-wins store would overwrite the notice's
     * reason with nothing.
     */
    fun mark(reason: String? = null): Boolean {
        if (latched) return false
        latched = true
        latchedReason = reason
        return true
    }

    /**
     * Cleared by an explicit start() — the deliberate re-enable.
     *
     * Drops the reason with the latch. They are one fact: a reason outliving
     * its latch could only ever be attached to a *different* displacement, and
     * [mark] would refuse to overwrite it.
     */
    fun clear() {
        latched = false
        latchedReason = null
    }

    /**
     * The event restating a *currently* superseded transport, or null if it is
     * not superseded.
     *
     * The iOS bridge calls this on app foreground to re-derive the one-shot
     * report from state instead of buffering the emit: a non-null result is
     * true at the moment it is delivered rather than a replay of a past edge,
     * so there is no staleness window to bound and no discard site to remember
     * at either path that clears the latch — after both, this returns null.
     * Android keeps its buffer (it serves a second event with no such backing
     * state) and does not call this today; it lives here so the mirrored
     * classes stay identical and the behaviour is pinned by both test suites.
     */
    fun restatementEventJson(): String? {
        if (!latched) return null
        return eventJson(latchedReason)
    }
}
