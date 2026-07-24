package com.offlineprotocol

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
 * Mirrors ios/SupersededLatchPolicy.swift — keep in sync.
 */
class SupersededLatchPolicy {
    companion object {
        /** WebSocket close code the relay uses to signal displacement. */
        const val SUPERSEDED_CLOSE_CODE = 4000
    }

    // Main-owned like the InternetManager flags it replaces (autoReconnect):
    // written on main, read best-effort off-main (getMetrics). @Volatile for
    // the same defensive cross-thread visibility the field it replaced had.
    @Volatile
    private var latched = false

    val isSuperseded: Boolean get() = latched

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
     * Latches superseded. Returns true only on the false→true transition, so
     * the caller fires the one-shot event exactly once even though the relay
     * emits both a notice and a close (each of which fans into several
     * terminal signals) for a single displacement.
     */
    fun mark(): Boolean {
        if (latched) return false
        latched = true
        return true
    }

    /** Cleared by an explicit start() — the deliberate re-enable. */
    fun clear() {
        latched = false
    }
}
