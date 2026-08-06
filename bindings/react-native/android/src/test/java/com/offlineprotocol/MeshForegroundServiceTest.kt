package com.offlineprotocol

import android.app.Service
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
 * Pins the three invariants that keep [MeshForegroundService] from taking the
 * process down with it, or from outliving the mesh it advertises.
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
 * foreground protection and nothing told to JS. That includes reaching the
 * *right* host: the callback slot is process-global while hosts are
 * per-ReactContext, so clearing it is by identity and a departing host must
 * leave its replacement armed.
 *
 * The third is what a START_STICKY restart may assert. The system hands the
 * service back after a process kill, but the protocol, the transports and the
 * module died with the old process, and rebuilding them from here is refused —
 * a protocol with no React context behind it ACKs the messages it decrypts and
 * then drops them, which retires the sender's retry ladder for messages that
 * reached nobody. So the restart has to check whether a host has brought a mesh
 * back up before it re-posts a notification claiming one is running. That check reads the same
 * stop-callback slot, which is why the slot's lifetime is pinned here too: it
 * has to be surrendered when the mesh stops, not when the module dies, or the
 * gate reads "host present" over a mesh that is already down.
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

    /** Every stop callback this test registered, so tearDown can clear them. */
    private val registeredStopCallbacks = mutableListOf<() -> Unit>()

    /**
     * `isRunning` and the internal start-request flag are companion state, so
     * they outlive an individual test. Destroying the instance clears both
     * through `onDestroy`, which keeps the no-op assertions in
     * [stop_does_not_create_a_service_when_none_was_started] honest.
     *
     * A test that calls `start()` without ever creating an instance leaves the
     * start-request flag set with no `onDestroy` to clear it, so `stop()` runs
     * unconditionally here too — it no-ops once both flags are already down.
     *
     * The stop slot is cleared by identity like production does, which means
     * clearing every callback registered here rather than nulling the field.
     */
    @After
    fun tearDown() {
        registeredStopCallbacks.forEach { MeshForegroundService.clearStopRequestCallback(it) }
        registeredStopCallbacks.clear()
        controller?.destroy()
        controller = null
        MeshForegroundService.stop(context, null)
        drainStartedServices()
    }

    private fun registerHost(callback: () -> Unit): () -> Unit {
        registeredStopCallbacks.add(callback)
        MeshForegroundService.registerStopRequestCallback(callback)
        return callback
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
        registerHost { invoked += 1 }

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
        assertNull(MeshForegroundService.onStopRequestedByUser)

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
        registerHost { throw IllegalStateException("host is gone") }

        createService()
        val service = deliver(ACTION_STOP_FROM_NOTIFICATION)

        assertTrue(shadowOf(service).isStoppedBySelf)
        assertFalse(MeshForegroundService.isRunning)
    }

    @Test
    fun `a host clearing its own registration disarms the stop action`() {
        var invoked = 0
        val callback = registerHost { invoked += 1 }

        MeshForegroundService.clearStopRequestCallback(callback)

        createService()
        val service = deliver(ACTION_STOP_FROM_NOTIFICATION)

        assertEquals("a cleared host must not be called", 0, invoked)
        assertNull(MeshForegroundService.onStopRequestedByUser)
        // Nothing left to defer to, so the button falls back to dropping the
        // keep-alive rather than doing nothing.
        assertTrue(shadowOf(service).isStoppedBySelf)
    }

    @Test
    fun `a stale host clearing after a newer one registered leaves the newer one armed`() {
        var staleInvoked = 0
        var currentInvoked = 0
        // The slot is process-global while modules are per-ReactContext: during
        // a React reload the replacement registers before the outgoing module
        // tears down. An unconditional null there would disarm the live host
        // and the Stop button would drop the keep-alive over a running mesh.
        val stale = registerHost { staleInvoked += 1 }
        registerHost { currentInvoked += 1 }

        MeshForegroundService.clearStopRequestCallback(stale)

        createService()
        val service = deliver(ACTION_STOP_FROM_NOTIFICATION)

        assertEquals("a stale clear must not disarm the current host", 1, currentInvoked)
        assertEquals(0, staleInvoked)
        assertFalse(
            "the current host owns teardown, so the keep-alive stays up",
            shadowOf(service).isStoppedBySelf
        )
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
    fun `sticky restart re-promotes for a host that is already back up`() {
        // A registered stop callback is what "a host in this process has a mesh
        // running" looks like — the module registers it immediately before
        // starting the service. Reachable on a restart because an app that
        // boots React Native from Application.onCreate can beat the
        // re-delivered intent.
        registerHost { }

        createService()
        // A null intent is how the system re-delivers after a process kill.
        val service = deliver(null)

        // Re-promotion is all this branch does. It deliberately does not try to
        // rebuild anything: the mesh it is keeping alive is the one that host
        // already brought up, and a protocol started with no React context
        // behind it would ACK incoming messages into a void rather than deliver
        // them — see handleStickyRestart.
        assertEquals(NOTIFICATION_ID, shadowOf(service).lastForegroundNotificationId)
        assertTrue(MeshForegroundService.isRunning)
        assertFalse(shadowOf(service).isStoppedBySelf)
    }

    @Test
    fun `sticky restart with no host stops instead of outliving the mesh`() {
        assertNull("no host registered is the whole precondition", MeshForegroundService.onStopRequestedByUser)

        createService()
        val service = deliver(null)

        // The protocol died with the old process and nothing here may rebuild
        // it, so "Mesh Active" would be a lie that outlives the mesh. On 12+ it
        // is worse: the promotion is refused from the background, leaving an
        // empty process squatting with no notification at all.
        //
        // "May not", not "cannot": rebuilding natively is reachable and refused.
        // Without a React context the rebuilt protocol ACKs every message it
        // decrypts and then drops the event, so senders retire messages that
        // were never delivered anywhere. Stopping keeps the outage recoverable.
        assertTrue(shadowOf(service).isForegroundStopped)
        assertTrue(shadowOf(service).isStoppedBySelf)
        assertFalse(MeshForegroundService.isRunning)
    }

    @Test
    fun `sticky restart return value tracks host presence`() {
        // Pins the return values the ServiceController API hides. Both branches
        // run against one instance: this is about what onStartCommand returns,
        // not about the lifecycle around it.
        val service = createService()

        // START_NOT_STICKY, or the system hands the same restart straight back
        // and the stop above becomes a loop rather than an exit.
        assertEquals(Service.START_NOT_STICKY, service.onStartCommand(null, 0, 1))

        registerHost { }
        assertEquals(Service.START_STICKY, service.onStartCommand(null, 0, 2))
    }

    @Test
    fun `stopping the mesh surrenders the host registration`() {
        val host = registerHost { }

        // Nothing is up, so stop() takes its nothing-to-stop early return. The
        // registration must go anyway: it tracks the mesh rather than the
        // module, and the mesh is down either way. Clearing after that guard
        // instead of before it would strand the slot on exactly the path the
        // app runs twice (stop() and invalidate()).
        MeshForegroundService.stop(context, host)

        assertNull(
            "the slot is the sticky-restart liveness signal and must fall with the mesh",
            MeshForegroundService.onStopRequestedByUser
        )
    }

    @Test
    fun `a stale host stopping the mesh leaves the current one armed`() {
        var staleInvoked = 0
        var currentInvoked = 0
        // The same reload overlap the clear path guards against, reached
        // through a stop: the replacement registers before the outgoing module
        // tears its own mesh down. Surrendering by identity is what keeps the
        // live host's Stop action armed.
        val stale = registerHost { staleInvoked += 1 }
        registerHost { currentInvoked += 1 }

        MeshForegroundService.stop(context, stale)
        drainStartedServices()

        createService()
        deliver(ACTION_STOP_FROM_NOTIFICATION)

        assertEquals("a stale host's stop must not disarm the current one", 1, currentInvoked)
        assertEquals(0, staleInvoked)
    }

    @Test
    fun `sticky restart after the host stopped the mesh stops instead of re-promoting`() {
        // The window the host-present branch exists for, with a mesh that went
        // down inside it: the kill leaves a restart pending, the new process
        // boots React Native, brings mesh up and takes it back down — all
        // before the re-delivered intent lands. A slot scoped to the module
        // rather than the mesh would still read "host present" here and
        // re-post "Mesh Active" over a protocol that is gone.
        val host = registerHost { }
        MeshForegroundService.start(context)
        drainStartedServices()
        MeshForegroundService.stop(context, host)
        drainStartedServices()

        createService()
        val service = deliver(null)

        assertNull(
            "a host that stopped its mesh is not a host the gate may keep us up for",
            MeshForegroundService.onStopRequestedByUser
        )
        assertTrue(shadowOf(service).isForegroundStopped)
        assertTrue(shadowOf(service).isStoppedBySelf)
        assertFalse(MeshForegroundService.isRunning)
    }

    @Test
    fun `stop does not create a service when none was started`() {
        // Establish the precondition explicitly: onDestroy clears both the
        // running flag and the start-request flag that survive between tests.
        createService()
        controller?.destroy()
        controller = null
        drainStartedServices()

        MeshForegroundService.stop(context, null)

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

        MeshForegroundService.stop(context, null)

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
