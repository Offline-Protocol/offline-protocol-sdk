package com.offlineprotocol

import android.content.Intent
import android.util.Log
import com.facebook.react.HeadlessJsTaskService
import com.facebook.react.ReactApplication
import com.facebook.react.bridge.Arguments
import com.facebook.react.jstasks.HeadlessJsTaskConfig

/**
 * Starts JavaScript after a process kill so that a receiver exists before a
 * protocol does.
 *
 * This service does not touch the protocol, the transports, or any SDK state.
 * All it does is boot React Native and hand control to the app's registered
 * wake task, which owns the decision and runs the ordinary
 * `new OfflineProtocol(config)` → `on(...)` → `start()` sequence with its own
 * configuration and credentials. That ordering is the entire justification for
 * the feature — see [MeshWakePolicy] for why anything native rebuilding the
 * protocol itself is refused rather than merely unimplemented.
 *
 * Dispatched only by [MeshForegroundService.handleStickyRestart], and only when
 * the app opted in and the keep-alive is genuinely in the foreground. It is
 * started with a plain `startService()`, which is legal precisely because that
 * promotion succeeded: a process hosting a running foreground service counts as
 * foreground for service starts. It deliberately does **not** promote itself —
 * the keep-alive already holds the process, and `HeadlessJsTaskService` never
 * calls `startForeground()`, so a `startForegroundService()` start here would
 * arm the five-second promotion deadline with nothing to satisfy it
 * (facebook/react-native#36816, open since 2023).
 *
 * ## Untestable here, and kept thin because of it
 *
 * `react-android` is `compileOnly` in the standalone/CI Gradle path, so this
 * class compiles but cannot be loaded by the Robolectric suite. Every decision
 * it would otherwise make lives in [MeshWakePolicy], which is plain Kotlin and
 * is tested; what is left here is dispatch. Keep it that way — logic added to
 * this file is logic no test in this repository can reach.
 *
 * For the same reason [MeshForegroundService] refers to this class by name
 * rather than by class reference: see [MeshWakePolicy.WAKE_SERVICE_CLASS].
 */
class MeshHeadlessWakeService : HeadlessJsTaskService() {

    private companion object {
        const val TAG = "MeshHeadlessWake"
    }

    /**
     * Dispatches the wake task directly rather than going through
     * `getTaskConfig`, which is the shape React Native's own documentation
     * blesses for callers that need to decide whether to run at all
     * ("override onStartCommand and call startTask depending on your custom
     * logic").
     *
     * Returns START_NOT_STICKY unconditionally. Stickiness belongs to the
     * keep-alive, which is the service that represents a running mesh; a wake
     * that the system re-delivered on its own would arrive with no gate having
     * decided it was wanted, and with no watchdog armed to clean up after it.
     */
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (application !is ReactApplication) {
            // Nothing to boot. Not an error worth crashing a restarted process
            // over: the app simply cannot host headless JavaScript, and the
            // watchdog will take the keep-alive down on schedule.
            Log.w(TAG, "Application does not implement ReactApplication; cannot wake JavaScript")
            stopSelf()
            return START_NOT_STICKY
        }

        val timeoutMs = intent?.getLongExtra(MeshWakePolicy.EXTRA_TIMEOUT_MS, 0L)
            ?.takeIf { it > 0L }
            ?: MeshWakePolicy.DEFAULT_TIMEOUT_MS
        val reason = intent?.getStringExtra(MeshWakePolicy.EXTRA_REASON)
            ?: MeshWakePolicy.REASON_STICKY_RESTART

        return try {
            startTask(
                HeadlessJsTaskConfig(
                    MeshWakePolicy.TASK_KEY,
                    Arguments.createMap().apply { putString("reason", reason) },
                    timeoutMs,
                    // Allowed in the foreground, and this is not a convenience.
                    // React Native asserts on starting a task while the React
                    // context is RESUMED, and that assertion runs inside a
                    // Runnable posted to the main thread with nothing catching
                    // it — an uncaught IllegalStateException, i.e. a process
                    // crash. The race is real and unavoidable here: the user can
                    // open the app in the window between the restart landing and
                    // the task starting. So the task is allowed to run in the
                    // foreground and is instead required to be idempotent, which
                    // it must be regardless — an app that is already running has
                    // a protocol and the task returns early.
                    true,
                )
            )
            START_NOT_STICKY
        } catch (e: Exception) {
            // A wake that cannot start is the watchdog's problem, not a reason
            // to take down a process that has just been restarted.
            Log.w(TAG, "Failed to start the mesh wake task: ${e.message}", e)
            stopSelf()
            START_NOT_STICKY
        }
    }
}
