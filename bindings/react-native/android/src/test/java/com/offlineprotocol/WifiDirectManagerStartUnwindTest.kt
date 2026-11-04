package com.offlineprotocol

import android.content.Context
import android.content.pm.PackageManager
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
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

    /** No Rust object behind it: only the calls a failed start makes. */
    private class FakeProtocol : OfflineProtocol(NoHandle) {
        override fun wifiDirectStatusChanged(isConnected: Boolean) {}
        override fun localAddress(): String? = null
    }

    private class Boom : RuntimeException("listener refused STARTING")

    /** Throws on STARTING, the first thing after the bind. */
    private class ThrowsOnStarting : TransportManagerListener {
        val seen = mutableListOf<TransportState>()
        override fun onTransportStateChanged(manager: TransportManager, state: TransportState) {
            seen += state
            if (state == TransportState.STARTING) throw Boom()
        }
        override fun onTransportError(manager: TransportManager, error: Throwable) {}
        override fun onTransportDiagnostic(
            manager: TransportManager,
            level: String,
            message: String,
            context: Map<String, Any?>,
        ) {}
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

    @Test
    fun `a start that throws after the bind releases the group port and stops`() {
        assumeTrue("another process holds the group port", groupPortIsFree())
        val context = RuntimeEnvironment.getApplication() as Context
        // Wi-Fi present, P2P not permitted: a LAN-only start, so the throw is
        // the only thing in the way.
        shadowOf(context.packageManager).setSystemFeature(PackageManager.FEATURE_WIFI, true)

        val manager = WifiDirectManager(context, FakeProtocol(), "unwind-test")
        val listener = ThrowsOnStarting()
        manager.listener = listener

        val thrown = thrownOffMain { manager.start() }

        assertNotNull("the start failure must reach the caller", thrown)
        assertTrue("the original failure, not the cleanup's: $thrown", thrown is Boom)
        assertEquals(TransportState.STOPPED, manager.state)
        assertEquals(
            listOf(TransportState.STARTING, TransportState.STOPPING, TransportState.STOPPED),
            listener.seen,
        )
        // The stranded socket would still hold it, and the next start would
        // have bound an ephemeral port and given up the group.
        assertTrue("the failed start left the group port bound", groupPortIsFree())
        val again = PeerStreamListener.bind()!!
        again.socket.use { assertTrue(again.ownsGroupPort) }
    }
}
