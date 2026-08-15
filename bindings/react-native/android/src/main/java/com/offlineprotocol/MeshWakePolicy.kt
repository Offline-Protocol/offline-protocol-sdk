package com.offlineprotocol

import android.os.Bundle

/** What [MeshForegroundService] should do with a null-intent sticky restart. */
enum class MeshRestartAction {
    /** A host in this process already has a mesh up; re-promote and stay. */
    KEEP_ALIVE,

    /** No host, but the app opted in: wake JavaScript and arm the watchdog. */
    WAKE,

    /** No host and no sound way to get one; stop rather than outlive the mesh. */
    STOP,
}

/**
 * The opt-in and the arithmetic behind the Headless-JS mesh wake — everything
 * about the wake that can be decided without touching React or the Android
 * framework, so that it can be tested.
 *
 * This is the built form of the direction issue #307 asked for and #297
 * endorsed without building. The refusal #297 recorded is unchanged and still
 * load-bearing: **nothing native may rebuild the protocol.** A protocol running
 * with no React context does not fail to deliver, it destroys messages and tells
 * their senders they arrived — the receive path ACKs before it emits, that ACK
 * retires the sender's outbox entry, and the event is then dropped by
 * `canEmitToJs()` with nothing persisting inbound content behind it. So the wake
 * does not re-create anything. It starts JavaScript, and JavaScript — holding
 * the app's own config, its own credentials, and its own durable store for
 * `message_received` — decides whether to bring the mesh back and does it
 * through the ordinary `start()` path. The receiver exists before the protocol
 * does, which is the whole point.
 *
 * ## Why the opt-in is manifest meta-data
 *
 * It has to be readable at the moment a restarted service asks the question,
 * which is before any JavaScript has run and possibly before the app's own
 * `Application` has done anything. Meta-data is available from
 * `PackageManager` at that instant, costs no new storage, and therefore needs no
 * `wipePersistedState` integration and cannot go stale against an account that
 * has been wiped. A persisted flag would have all three problems, and a runtime
 * flag would not exist yet.
 *
 * It is also deliberately the app's decision rather than the SDK's default:
 * waking JavaScript in the background is only correct for an app whose wake task
 * durably stores what it receives, and only the app knows that.
 *
 * ## Why the promotion gate is not optional
 *
 * [decideRestart] refuses to wake when the foreground promotion did not take.
 * Two independent reasons, and either alone is sufficient. A process holding a
 * running foreground service counts as foreground for service-start purposes, so
 * `startService()` for the wake service is legal — but *only* while that
 * promotion actually happened; without it the same call is a background service
 * start and throws `BackgroundServiceStartNotAllowedException` on API 31+. And a
 * service that stays up without a notification is the empty-process squatting
 * failure #294 removed, which is exactly what the wake must not reintroduce.
 *
 * A system restart of a sticky *foreground* service is exempt from the API 31+
 * background-start restriction (`Service.START_STICKY` reference: "the
 * restriction doesn't impact restarts of a sticky foreground service"), so the
 * promotion normally succeeds and this gate is not the common path. It is here
 * for the cases that remain — a revoked Nearby-Devices permission taking the
 * `connectedDevice` promotion down with a `SecurityException`, or OEM variance —
 * where failing closed to the pre-#307 behaviour is the honest outcome.
 *
 * Not internally synchronized and holds no state: every function is pure and the
 * caller confines its use to the service's main thread.
 */
object MeshWakePolicy {

    /**
     * Opt-in flag, read from the app's `<application>` meta-data.
     *
     * Accepts `android:value="true"` (a boolean to AAPT) and a string `"true"`,
     * because an app routing the value through a build variant or a string
     * resource gets the latter and the difference is invisible until the wake
     * silently never happens.
     */
    const val META_DATA_ENABLED = "com.offlineprotocol.MESH_WAKE_ENABLED"

    /** Optional wake budget override, in whole seconds. See [DEFAULT_TIMEOUT_MS]. */
    const val META_DATA_TIMEOUT_SECONDS = "com.offlineprotocol.MESH_WAKE_TIMEOUT_SECONDS"

    /**
     * How long the wake task gets before React Native terminates it.
     *
     * Sized for the slow path this exists to serve: a cold React Native boot
     * plus the app's own storage read plus MLS initialization on a mid-range
     * device, which is seconds rather than milliseconds. It must never be 0 —
     * that is React Native's "no timeout" sentinel, and an untimed task on a
     * release where `notifyTaskFinished` does not reach native (RN 0.84/0.85,
     * facebook/react-native#56263) leaves the wake service and its partial wake
     * lock held for the process lifetime.
     */
    const val DEFAULT_TIMEOUT_MS = 60_000L

    /** Floor for the override: below this a cold RN boot cannot finish. */
    const val MIN_TIMEOUT_MS = 10_000L

    /** Ceiling for the override, bounding how long a lying notification can stand. */
    const val MAX_TIMEOUT_MS = 300_000L

    /**
     * Slack between the task deadline and the watchdog, so the watchdog is
     * strictly the later of the two and cannot reap a wake that is still inside
     * its own budget.
     */
    const val WATCHDOG_GRACE_MS = 15_000L

    /**
     * The JavaScript task key, which must match `MESH_WAKE_TASK_KEY` in
     * `src/constants.ts`.
     *
     * A drift here fails silently in the worst way: React Native logs "No task
     * registered for key" to the device log and resolves nothing, the app sees
     * an opt-in that does nothing, and both sides still compile. Pinned by
     * `react_native_mesh_wake_wiring_is_present` in the uniffi crate.
     */
    const val TASK_KEY = "OfflineProtocolMeshWake"

    /**
     * The wake service, named as a string rather than a class reference **on
     * purpose**.
     *
     * `MeshHeadlessWakeService` extends React Native's `HeadlessJsTaskService`,
     * and `react-android` is `compileOnly` in the standalone/CI Gradle path — it
     * compiles, but it is not on the test runtime classpath. Resolving a class
     * constant for it would load its superclass and throw `NoClassDefFoundError`
     * inside the Robolectric suite, taking [MeshForegroundService]'s coverage
     * down with it. Naming it as a string keeps the foreground service — the one
     * with tests — free of any React type, and the guard test in the uniffi
     * crate pins the string against the class that has to exist.
     */
    const val WAKE_SERVICE_CLASS = "com.offlineprotocol.MeshHeadlessWakeService"

    /** Extra carrying [MeshWakeSettings.timeoutMs] to the wake service. */
    const val EXTRA_TIMEOUT_MS = "com.offlineprotocol.extra.WAKE_TIMEOUT_MS"

    /** Extra naming why JavaScript was woken, surfaced to the task as `reason`. */
    const val EXTRA_REASON = "com.offlineprotocol.extra.WAKE_REASON"

    /** The only reason there is today: the system handed the service back. */
    const val REASON_STICKY_RESTART = "sticky_restart"

    /**
     * Reads the app's opt-in. Absent, malformed, or explicitly off all yield
     * [MeshWakeSettings.DISABLED] — an app that has not asked for this gets
     * exactly the pre-#307 behaviour.
     */
    fun settingsFrom(metaData: Bundle?): MeshWakeSettings {
        if (metaData == null) return MeshWakeSettings.DISABLED
        if (!readFlag(metaData, META_DATA_ENABLED)) return MeshWakeSettings.DISABLED
        return MeshWakeSettings(enabled = true, timeoutMs = readTimeoutMs(metaData))
    }

    /**
     * Decides what a null-intent restart does.
     *
     * [hostPresent] is `MeshForegroundService.onStopRequestedByUser != null` —
     * the only signal in the process that a live host believes mesh is running,
     * and the reason no new state is needed here. It is scoped to the mesh
     * rather than to the module (registered as mesh comes up, surrendered as it
     * goes down), so it answers both questions this feature asks: whether a wake
     * is needed at all, and whether one that was already sent has landed.
     */
    fun decideRestart(
        hostPresent: Boolean,
        wakeEnabled: Boolean,
        foregroundPromoted: Boolean,
    ): MeshRestartAction = when {
        // A host beat the re-delivered intent — an app booting React Native from
        // Application.onCreate does win this race. The mesh it started is the
        // one this notification now belongs to; waking a second one would be
        // redundant at best.
        hostPresent -> MeshRestartAction.KEEP_ALIVE
        !wakeEnabled -> MeshRestartAction.STOP
        !foregroundPromoted -> MeshRestartAction.STOP
        else -> MeshRestartAction.WAKE
    }

    /**
     * How long after dispatching a wake the watchdog should check on it.
     *
     * Strictly later than the task's own deadline (see [WATCHDOG_GRACE_MS]), so
     * a task still inside its budget is never reaped, and a task React Native
     * has already terminated is never waited on indefinitely.
     */
    fun watchdogDelayMs(settings: MeshWakeSettings): Long =
        settings.timeoutMs + WATCHDOG_GRACE_MS

    /**
     * Whether the watchdog should bring the keep-alive down.
     *
     * The wake succeeded exactly when a host registered, because the app's
     * `start()` registers the stop callback before this service is asked to come
     * up. Anything else — the app declined to restore the mesh, the task was
     * never registered, JavaScript failed to boot, the task threw — leaves the
     * slot empty and must end in a stop, or the "Mesh Active" notification over
     * a dead protocol that #294 removed comes straight back.
     */
    fun shouldStopOnWatchdog(hostPresent: Boolean): Boolean = !hostPresent

    /**
     * Reads a meta-data flag as either a real boolean or the string `"true"`.
     *
     * `Bundle.getBoolean` returns the default for a value stored as a String
     * rather than coercing, so the string form has to be checked separately.
     */
    private fun readFlag(metaData: Bundle, key: String): Boolean {
        if (metaData.getBoolean(key, false)) return true
        return metaData.getString(key)?.trim().equals("true", ignoreCase = true)
    }

    /** Reads the timeout override in seconds, clamped, falling back to the default. */
    private fun readTimeoutMs(metaData: Bundle): Long {
        val seconds = metaData.getInt(META_DATA_TIMEOUT_SECONDS, 0)
            .takeIf { it > 0 }
            ?: metaData.getString(META_DATA_TIMEOUT_SECONDS)?.trim()?.toIntOrNull()
            ?: return DEFAULT_TIMEOUT_MS
        if (seconds <= 0) return DEFAULT_TIMEOUT_MS
        return (seconds * 1_000L).coerceIn(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)
    }
}

/**
 * The app's wake opt-in, resolved. Built by [MeshWakePolicy.settingsFrom].
 */
data class MeshWakeSettings(
    /** Whether the app asked for the mesh to be restored after a process kill. */
    val enabled: Boolean,
    /** The wake budget handed to React Native as the headless task timeout. */
    val timeoutMs: Long,
) {
    companion object {
        /** What every app that has not opted in gets. */
        val DISABLED = MeshWakeSettings(enabled = false, timeoutMs = MeshWakePolicy.DEFAULT_TIMEOUT_MS)
    }
}
