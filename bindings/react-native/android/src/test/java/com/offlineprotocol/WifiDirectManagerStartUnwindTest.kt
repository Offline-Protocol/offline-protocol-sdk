package com.offlineprotocol

import android.content.Context
import android.content.pm.PackageManager
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import uniffi.offline_protocol.NoHandle
import uniffi.offline_protocol.OfflineProtocol
import java.io.IOException
import java.net.ServerSocket
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * [WifiDirectManager.start] unwinds a start that throws after the peer-stream
 * listener is bound. Before, a throw between the bind and the accept task left
 * the socket out of reach of `stop()`, holding the group port for the life of
 * the process, so every later start in it got an ephemeral port and the device
 * never formed a group again.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class WifiDirectManagerStartUnwindTest {

    /**
     * No Rust object behind it: only the calls a failed start makes, which
     * are `wifiDirectStatusChanged` (from `stopUnsafe`) and `localAddress`.
     * Any other method reaches the FFI with no handle and takes the test JVM
     * down rather than failing the test, so a new protocol call on the start
     * or stop path must be overridden here.
     */
    private class FakeProtocol : OfflineProtocol(NoHandle) {
        override fun wifiDirectStatusChanged(isConnected: Boolean) {}
        override fun localAddress(): String? = null
    }

    private class Boom : RuntimeException("listener refused the state")

    /**
     * Throws when the manager enters [failOn]. Before it throws on RUNNING it
     * waits until the accept task has reported its socket and has had time to
     * reach `accept()`, so the unwind closes a socket that task is blocked on.
     *
     * After that throw it takes its time over STOPPING, as a host listener
     * that forwards the state across a bridge may. Without the wait the race
     * the RUNNING case pins is real but narrow: an unwind that closes the
     * socket before it stops clears the running flag a few microseconds
     * later, usually before the accept thread wakes, so the test passed
     * against the wrong order. The wait holds the flag up for as long as
     * that listener runs, which is when the accept thread reports.
     */
    private class ThrowsOn(private val failOn: TransportState) : TransportManagerListener {
        val seen = CopyOnWriteArrayList<TransportState>()
        val diagnostics = CopyOnWriteArrayList<Pair<String, String>>()
        private val acceptTaskStarted = CountDownLatch(1)

        override fun onTransportStateChanged(manager: TransportManager, state: TransportState) {
            seen += state
            if (state == TransportState.STOPPING && failOn == TransportState.RUNNING) {
                Thread.sleep(300)
            }
            if (state != failOn) return
            if (state == TransportState.RUNNING) {
                assertTrue(
                    "the accept task never started",
                    acceptTaskStarted.await(5, TimeUnit.SECONDS),
                )
                // From its report to accept() is one loop condition.
                Thread.sleep(200)
            }
            throw Boom()
        }

        override fun onTransportError(manager: TransportManager, error: Throwable) {}

        override fun onTransportDiagnostic(
            manager: TransportManager,
            level: String,
            message: String,
            context: Map<String, Any?>,
        ) {
            diagnostics += level to message
            if (message.startsWith("Server socket started")) acceptTaskStarted.countDown()
        }
    }

    @After
    fun tearDown() {
        // The manager's looper is process-wide; leave none behind for the next class.
        TransportConfinement.releaseForTests("offline-wifidirect")
    }

    /**
     * Runs [action] off the main looper and returns what it threw. On the main
     * looper `runSync` waits a bounded time and then gives up, which would
     * turn a slow machine into a false failure.
     */
    private fun thrownOffMain(action: () -> Unit): Throwable? {
        var thrown: Throwable? = null
        val thread = Thread {
            try {
                action()
            } catch (t: Throwable) {
                thrown = t
            }
        }
        thread.start()
        thread.join(10_000)
        assertTrue("start() did not return", !thread.isAlive)
        return thrown
    }

    private fun groupPortIsFree(): Boolean = try {
        ServerSocket(PeerStreamListener.GROUP_OWNER_PORT).close()
        true
    } catch (_: IOException) {
        false
    }

    /** Starts a LAN-only manager whose listener throws on [failOn], and checks the unwind. */
    private fun startThrowingOn(failOn: TransportState): ThrowsOn {
        // Asserted, not assumed: nothing else in this JVM may hold the group
        // port, and an assumption here would skip the test silently when a
        // sibling test leaked it.
        assertTrue("the group port is already bound before the start", groupPortIsFree())
        val context = RuntimeEnvironment.getApplication() as Context
        // Wi-Fi present, P2P not permitted: a LAN-only start, so the throw is
        // the only thing in the way.
        shadowOf(context.packageManager).setSystemFeature(PackageManager.FEATURE_WIFI, true)

        val manager = WifiDirectManager(context, FakeProtocol(), "unwind-test")
        val listener = ThrowsOn(failOn)
        manager.listener = listener

        val thrown = thrownOffMain { manager.start() }

        assertNotNull("the start failure must reach the caller", thrown)
        assertTrue("the original failure, not the cleanup's: $thrown", thrown is Boom)
        assertEquals(TransportState.STOPPED, manager.state)
        // The stranded socket would still hold it, and the next start would
        // have bound an ephemeral port and given up the group.
        assertTrue("the failed start left the group port bound", groupPortIsFree())
        val again = PeerStreamListener.bind()!!
        again.socket.use { assertTrue(again.ownsGroupPort) }
        return listener
    }

    @Test
    fun `a start that throws after the bind releases the group port and stops`() {
        val listener = startThrowingOn(TransportState.STARTING)
        assertEquals(
            listOf(TransportState.STARTING, TransportState.STOPPING, TransportState.STOPPED),
            listener.seen.toList(),
        )
    }

    @Test
    fun `a start that throws after the accept task runs unwinds without a false accept error`() {
        val listener = startThrowingOn(TransportState.RUNNING)
        assertEquals(
            listOf(
                TransportState.STARTING,
                TransportState.RUNNING,
                TransportState.STOPPING,
                TransportState.STOPPED,
            ),
            listener.seen.toList(),
        )
        // The accept task reports on its own thread; give a false report time to land.
        Thread.sleep(500)
        val errors = listener.diagnostics.filter { it.first == "error" }
        assertTrue(
            "closing the listener before the stop made the accept task report an error: $errors",
            errors.isEmpty(),
        )
    }
}
