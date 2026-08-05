package com.offlineprotocol

import android.content.Context
import android.content.Intent
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.android.controller.ServiceController
import org.robolectric.annotation.Config

/**
 * Pins the two invariants that keep [MeshForegroundService] from taking the
 * process down with it.
 *
 * The first is the reason this class exists at all: Android gives an app five
 * seconds after `startForegroundService()` to call `startForeground()`, and
 * missing it is a fatal `RemoteServiceException`, not a degraded service. The
 * promotion therefore has to happen in `onCreate` — before any `onStartCommand`
 * dispatch, and so before whatever main-thread work is queued ahead of it. That
 * is invisible in the source once written; only a test that creates the service
 * *without* delivering a start command can tell the two placements apart.
 *
 * The second is the notification's Stop action, which must reach the host
 * module rather than stopping this service directly. Dropping the keep-alive
 * on its own leaves the transports and the process scheduler running with no
 * foreground protection and nothing told to JS.
 *
 * Action strings are written out verbatim rather than read from the companion:
 * they cross a process boundary inside a PendingIntent that can outlive the
 * process that built it, so a rename is a compatibility question, not a
 * refactor, and the test should notice.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class MeshForegroundServiceTest {

    private companion object {
        const val NOTIFICATION_ID = 9001
        const val ACTION_STOP = "com.offlineprotocol.action.STOP_MESH"
        const val ACTION_STOP_FROM_NOTIFICATION =
            "com.offlineprotocol.action.STOP_MESH_FROM_NOTIFICATION"
        const val ACTION_START = "com.offlineprotocol.action.START_MESH"
    }

    private val context: Context
        get() = RuntimeEnvironment.getApplication()

    private var controller: ServiceController<MeshForegroundService>? = null

    /**
     * `isRunning` and the internal start-request flag are companion state, so
     * they outlive an individual test. Destroying the instance clears both
     * through `onDestroy`, which keeps the no-op assertions in
     * [stop_does_not_create_a_service_when_none_was_started] honest.
     *
     * A test that calls `start()` without ever creating an instance leaves the
     * start-request flag set with no `onDestroy` to clear it, so `stop()` runs
     * unconditionally here too — it no-ops once both flags are already down.
     */
    @After
    fun tearDown() {
        MeshForegroundService.onStopRequestedByUser = null
        MeshForegroundService.onServiceRestarted = null
        controller?.destroy()
        controller = null
        MeshForegroundService.stop(context)
        drainStartedServices()
    }

    private fun createService(): MeshForegroundService {
        val created = Robolectric.buildService(MeshForegroundService::class.java)
        controller = created
        return created.create().get()
    }

    private fun deliver(action: String?): MeshForegroundService {
        val started = controller ?: error("createService() first")
        val intent = action?.let {
            Intent(context, MeshForegroundService::class.java).apply { this.action = it }
        }
        return started.withIntent(intent).startCommand(0, 1).get()
    }

    private fun drainStartedServices() {
        while (shadowOf(RuntimeEnvironment.getApplication()).nextStartedService != null) {
            // discard
        }
    }

    @Test
    fun `service is in the foreground by the end of onCreate`() {
        val service = createService()

        // Deliberately no startCommand: this is the whole point. If the
        // promotion ever moves back into onStartCommand, the notification is
        // still absent here and the 5-second deadline becomes reachable again.
        val shadow = shadowOf(service)
        assertNotNull(
            "startForeground must run in onCreate, before any onStartCommand dispatch",
            shadow.lastForegroundNotification
        )
        assertEquals(NOTIFICATION_ID, shadow.lastForegroundNotificationId)
        assertTrue(MeshForegroundService.isRunning)
    }

    @Test
    fun `notification carries a stop action pointing back at this service`() {
        val service = createService()

        val notification = shadowOf(service).lastForegroundNotification
        val actions = notification.actions
        assertEquals("expected exactly one notification action", 1, actions.size)

        val pending = shadowOf(actions[0].actionIntent)
        assertTrue(
            "O+ must use getForegroundService so the tap is legal while backgrounded",
            pending.isForegroundServiceIntent
        )

        val routed = pending.savedIntent
        assertEquals(ACTION_STOP_FROM_NOTIFICATION, routed.action)
        assertEquals(MeshForegroundService::class.java.name, routed.component?.className)
    }

    @Test
    fun `stop action defers to the host and leaves the keep-alive up`() {
        var invoked = 0
        MeshForegroundService.onStopRequestedByUser = { invoked += 1 }

        createService()
        val service = deliver(ACTION_STOP_FROM_NOTIFICATION)

        assertEquals("the host owns transport teardown and must be called", 1, invoked)
        // The host's own teardown ends in MeshForegroundService.stop(), which
        // comes back through ACTION_STOP. Clearing the notification here would
        // report "mesh off" while the radios are still running.
        assertFalse(
            "service must stay up until the host has actually stopped the mesh",
            shadowOf(service).isStoppedBySelf
        )
    }

    @Test
    fun `stop action still drops the keep-alive when no host is registered`() {
        MeshForegroundService.onStopRequestedByUser = null

        createService()
        val service = deliver(ACTION_STOP_FROM_NOTIFICATION)

        // Nothing to defer to (process restarted, module never re-registered).
        // The button must not be dead.
        assertTrue(shadowOf(service).isStoppedBySelf)
        assertTrue(shadowOf(service).isForegroundStopped)
        assertFalse(MeshForegroundService.isRunning)
    }

    @Test
    fun `stop action falls back to stopping itself when the host callback throws`() {
        MeshForegroundService.onStopRequestedByUser = { throw IllegalStateException("host is gone") }

        createService()
        val service = deliver(ACTION_STOP_FROM_NOTIFICATION)

        assertTrue(shadowOf(service).isStoppedBySelf)
        assertFalse(MeshForegroundService.isRunning)
    }

    @Test
    fun `host-initiated stop tears down foreground and self`() {
        createService()
        val service = deliver(ACTION_STOP)

        assertTrue(shadowOf(service).isForegroundStopped)
        assertTrue(shadowOf(service).isStoppedBySelf)
        assertFalse(MeshForegroundService.isRunning)
    }

    @Test
    fun `sticky restart re-promotes and notifies the host`() {
        var restarted = 0
        MeshForegroundService.onServiceRestarted = { restarted += 1 }

        createService()
        // A null intent is how the system re-delivers after a process kill.
        val service = deliver(null)

        assertEquals(1, restarted)
        assertEquals(NOTIFICATION_ID, shadowOf(service).lastForegroundNotificationId)
        assertTrue(MeshForegroundService.isRunning)
    }

    @Test
    fun `stop does not create a service when none was started`() {
        // Establish the precondition explicitly: onDestroy clears both the
        // running flag and the start-request flag that survive between tests.
        createService()
        controller?.destroy()
        controller = null
        drainStartedServices()

        MeshForegroundService.stop(context)

        // Without the guard this would start the service just to stop it —
        // and since onCreate now promotes, that is a notification flash on a
        // teardown path the app runs twice (stop() and invalidate()).
        assertNull(
            "stop() must not create a service instance when nothing is running",
            shadowOf(RuntimeEnvironment.getApplication()).nextStartedService
        )
    }

    @Test
    fun `stop reaches a service that is still coming up`() {
        // start() sets the request flag synchronously; the service itself is
        // created asynchronously, so isRunning is still false at this point.
        // Gating stop() on isRunning alone would strand the instance.
        MeshForegroundService.start(context)
        drainStartedServices()

        MeshForegroundService.stop(context)

        val stopIntent = shadowOf(RuntimeEnvironment.getApplication()).nextStartedService
        assertNotNull("stop() must reach a service that start() already requested", stopIntent)
        assertEquals(ACTION_STOP, stopIntent.action)
    }

    @Test
    fun `start requests the service with the start action`() {
        MeshForegroundService.start(context)

        val intent = shadowOf(RuntimeEnvironment.getApplication()).nextStartedService
        assertNotNull(intent)
        assertEquals(ACTION_START, intent.action)
        assertEquals(MeshForegroundService::class.java.name, intent.component?.className)
    }
}
