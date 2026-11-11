package com.offlineprotocol

/**
 * Development-only ordering hints for hosts that drive [ProtocolEngineHost]
 * directly (the Flutter plugin, a hand-written Kotlin host). React Native
 * turns them off, because its TypeScript layer already gives the same hints
 * (`lifecycleHints.ts`).
 *
 * A hint that fires on correct code teaches developers to ignore the channel,
 * so every hint here must be one a correctly ordered app never sees. That is
 * why the data hint reacts to the engine refusing a call for want of storage,
 * rather than predicting it from engine state: the engine gates data calls on
 * storage, not on running, so any prediction from state is wrong on some
 * valid order (after MLS init but before start, while stopped, while paused).
 *
 * The semantics mirror `bindings/react-native/src/lifecycleHints.ts`; keep the
 * two in step. One instance per host: the dedupe set and the applied interest
 * belong to that host, and are read from whatever thread calls it.
 */
class IntegratorLifecycleHints(
    val enabled: Boolean,
    private val sink: (String) -> Unit,
) {
    private val lock = Any()
    private val warnedKeys = HashSet<String>()

    /** The interest last applied, by space; an absent space has the default `["*"]`. */
    private val appliedInterest = HashMap<String, List<String>>()

    /** Forgets what was warned and applied, for the next engine on this host. */
    fun reset() {
        synchronized(lock) {
            warnedKeys.clear()
            appliedInterest.clear()
        }
    }

    private fun warnOnce(key: String, message: String) {
        if (!enabled) return
        val first = synchronized(lock) { warnedKeys.add(key) }
        if (first) sink(message)
    }

    fun callBeforeEngineStart(context: String, engineRunning: Boolean) {
        if (engineRunning) return
        warnOnce(
            "engine:$context",
            "[OfflineProtocol] $context: call start() before this API. " +
                "Install the event callback and declare data interest before start(). " +
                "See $DOC.",
        )
    }

    fun startWithoutMls(encryptionEnabled: Boolean, mlsReady: Boolean) {
        if (!encryptionEnabled || mlsReady) return
        warnOnce(
            "start:mls",
            "[OfflineProtocol] start() with encryption enabled before " +
                "initializeMlsWithSecureStorage(). Secure send and handshake fail until it runs, " +
                "or set encryptionEnabled to false for cleartext-only tests. See $DOC.",
        )
    }

    fun sendWithoutMls(
        context: String,
        encryptionEnabled: Boolean,
        mlsReady: Boolean,
        engineRunning: Boolean,
    ) {
        if (!engineRunning || !encryptionEnabled || mlsReady) return
        warnOnce(
            "mls:$context",
            "[OfflineProtocol] $context: encryption is enabled but MLS is not initialized. " +
                "Secure send and handshake may fail until it is. Run " +
                "initializeMlsWithSecureStorage() successfully, or set encryptionEnabled " +
                "to false for cleartext-only tests.",
        )
    }

    /** The engine refused [context] with `DataStorageUnavailable`. */
    fun dataStorageUnavailable(context: String) {
        warnOnce(
            "data:$context",
            "[OfflineProtocol] $context: the data layer has no storage yet. " +
                "Run initializeMlsWithSecureStorage() (or set a data storage) before data calls. " +
                "See docs/data.md.",
        )
    }

    /**
     * Records that the engine accepted [patterns] for [spaceId], warning first
     * when a running engine narrowed the space's interest. A widening is the
     * supported post-start use (it asks peers for what it adds) and never warns.
     */
    fun interestApplied(spaceId: String, patterns: List<String>, engineRunning: Boolean) {
        val previous = synchronized(lock) {
            appliedInterest.put(spaceId, patterns.toList()) ?: listOf("*")
        }
        if (!engineRunning || !isInterestNarrowing(previous, patterns)) return
        warnOnce(
            "interest:narrowed:$spaceId",
            "[OfflineProtocol] setInterest(\"$spaceId\") after start() narrowed the interest. " +
                "The narrower filter refuses documents from now on, but does not delete what " +
                "this device already holds, including what the start-up exchange pulled in. " +
                "Declare interest before start() when you need a tight filter from the first exchange.",
        )
    }

    companion object {
        private const val DOC = "docs/android-integration.md and docs/integrator-map.md"

        /**
         * Whether interest pattern [outer] matches every document [inner]
         * matches. A pattern is a document name, or a prefix ending in `*`.
         */
        private fun patternCovers(outer: String, inner: String): Boolean {
            if (outer == inner) return true
            if (!outer.endsWith("*")) return false
            return inner.startsWith(outer.dropLast(1))
        }

        /**
         * Whether replacing [previous] with [next] drops any document
         * [previous] matched. Conservative, as in `lifecycleHints.ts`: a
         * pattern counts as kept only when one pattern of [next] covers it whole.
         */
        fun isInterestNarrowing(previous: List<String>, next: List<String>): Boolean =
            previous.any { old -> next.none { candidate -> patternCovers(candidate, old) } }
    }
}
