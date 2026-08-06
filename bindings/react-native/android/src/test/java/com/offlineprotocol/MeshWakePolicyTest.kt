package com.offlineprotocol

import android.os.Bundle
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * Pins the two things the mesh wake gets to decide, both of which fail silently
 * in production.
 *
 * The opt-in is read from manifest meta-data, and AAPT's typing of that value is
 * not something an app author controls in one obvious way: `android:value="true"`
 * is a boolean, the same value routed through a string resource or a build
 * variant is a String, and `Bundle.getBoolean` returns its default for the
 * latter rather than coercing. An app that opted in and got silence would have
 * no way to tell that from the feature not working.
 *
 * The restart decision is here rather than in the service for a harness reason:
 * `react-android` is `compileOnly` in the standalone Gradle path, so anything
 * that touches React — including [MeshHeadlessWakeService] — cannot be loaded by
 * this suite at all. Keeping the decisions in plain Kotlin is what makes them
 * testable, and the refused-promotion branch in particular is reachable *only*
 * from here: nothing in Robolectric makes a real `startForeground` fail.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class MeshWakePolicyTest {

    private fun metaData(build: Bundle.() -> Unit): Bundle = Bundle().apply(build)

    // --- The opt-in --------------------------------------------------------

    @Test
    fun `absent meta-data is not opted in`() {
        assertEquals(MeshWakeSettings.DISABLED, MeshWakePolicy.settingsFrom(null))
    }

    @Test
    fun `meta-data without the flag is not opted in`() {
        val settings = MeshWakePolicy.settingsFrom(metaData { putString("unrelated", "value") })

        assertFalse(settings.enabled)
    }

    @Test
    fun `a boolean flag opts in`() {
        val settings = MeshWakePolicy.settingsFrom(
            metaData { putBoolean(MeshWakePolicy.META_DATA_ENABLED, true) }
        )

        assertTrue(settings.enabled)
        assertEquals(MeshWakePolicy.DEFAULT_TIMEOUT_MS, settings.timeoutMs)
    }

    @Test
    fun `a string flag opts in, whatever its case`() {
        // The form an app gets from a string resource or a manifest placeholder.
        // getBoolean would report false for both of these and the opt-in would
        // look like it simply did not work.
        listOf("true", "TRUE", " true ").forEach { raw ->
            val settings = MeshWakePolicy.settingsFrom(
                metaData { putString(MeshWakePolicy.META_DATA_ENABLED, raw) }
            )

            assertTrue("expected $raw to opt in", settings.enabled)
        }
    }

    @Test
    fun `an explicit false does not opt in`() {
        listOf<Bundle>(
            metaData { putBoolean(MeshWakePolicy.META_DATA_ENABLED, false) },
            metaData { putString(MeshWakePolicy.META_DATA_ENABLED, "false") },
            metaData { putString(MeshWakePolicy.META_DATA_ENABLED, "yes") },
        ).forEach { bundle ->
            assertFalse(MeshWakePolicy.settingsFrom(bundle).enabled)
        }
    }

    // --- The wake budget ---------------------------------------------------

    @Test
    fun `an integer override is taken in seconds`() {
        val settings = MeshWakePolicy.settingsFrom(
            metaData {
                putBoolean(MeshWakePolicy.META_DATA_ENABLED, true)
                putInt(MeshWakePolicy.META_DATA_TIMEOUT_SECONDS, 90)
            }
        )

        assertEquals(90_000L, settings.timeoutMs)
    }

    @Test
    fun `a string override is taken in seconds`() {
        val settings = MeshWakePolicy.settingsFrom(
            metaData {
                putBoolean(MeshWakePolicy.META_DATA_ENABLED, true)
                putString(MeshWakePolicy.META_DATA_TIMEOUT_SECONDS, "90")
            }
        )

        assertEquals(90_000L, settings.timeoutMs)
    }

    @Test
    fun `an override outside the supported range is clamped`() {
        val floor = MeshWakePolicy.settingsFrom(
            metaData {
                putBoolean(MeshWakePolicy.META_DATA_ENABLED, true)
                putInt(MeshWakePolicy.META_DATA_TIMEOUT_SECONDS, 1)
            }
        )
        val ceiling = MeshWakePolicy.settingsFrom(
            metaData {
                putBoolean(MeshWakePolicy.META_DATA_ENABLED, true)
                putInt(MeshWakePolicy.META_DATA_TIMEOUT_SECONDS, 86_400)
            }
        )

        assertEquals(MeshWakePolicy.MIN_TIMEOUT_MS, floor.timeoutMs)
        assertEquals(MeshWakePolicy.MAX_TIMEOUT_MS, ceiling.timeoutMs)
    }

    /**
     * 0 is React Native's "run without a timeout" sentinel, and an untimed task
     * on a release where `notifyTaskFinished` never reaches native leaves the
     * wake service and its partial wake lock held for the process lifetime. No
     * meta-data an app can write may produce it.
     */
    @Test
    fun `a malformed or zero override falls back to the default, never to zero`() {
        listOf<Bundle.() -> Unit>(
            { putInt(MeshWakePolicy.META_DATA_TIMEOUT_SECONDS, 0) },
            { putInt(MeshWakePolicy.META_DATA_TIMEOUT_SECONDS, -30) },
            { putString(MeshWakePolicy.META_DATA_TIMEOUT_SECONDS, "soon") },
            { putString(MeshWakePolicy.META_DATA_TIMEOUT_SECONDS, "") },
            { putBoolean(MeshWakePolicy.META_DATA_TIMEOUT_SECONDS, true) },
        ).forEach { override ->
            val settings = MeshWakePolicy.settingsFrom(
                metaData {
                    putBoolean(MeshWakePolicy.META_DATA_ENABLED, true)
                    override()
                }
            )

            assertEquals(MeshWakePolicy.DEFAULT_TIMEOUT_MS, settings.timeoutMs)
        }
    }

    @Test
    fun `the default budget is neither zero nor outside the supported range`() {
        assertTrue(MeshWakePolicy.DEFAULT_TIMEOUT_MS >= MeshWakePolicy.MIN_TIMEOUT_MS)
        assertTrue(MeshWakePolicy.DEFAULT_TIMEOUT_MS <= MeshWakePolicy.MAX_TIMEOUT_MS)
        assertTrue(MeshWakeSettings.DISABLED.timeoutMs > 0L)
    }

    // --- The restart decision ----------------------------------------------

    @Test
    fun `a host already up keeps the keep-alive, opted in or not`() {
        listOf(true, false).forEach { wakeEnabled ->
            assertEquals(
                MeshRestartAction.KEEP_ALIVE,
                MeshWakePolicy.decideRestart(
                    hostPresent = true,
                    wakeEnabled = wakeEnabled,
                    foregroundPromoted = true,
                ),
            )
        }
    }

    @Test
    fun `no host and no opt-in stops, exactly as before this feature`() {
        assertEquals(
            MeshRestartAction.STOP,
            MeshWakePolicy.decideRestart(
                hostPresent = false,
                wakeEnabled = false,
                foregroundPromoted = true,
            ),
        )
    }

    @Test
    fun `no host with an opt-in wakes`() {
        assertEquals(
            MeshRestartAction.WAKE,
            MeshWakePolicy.decideRestart(
                hostPresent = false,
                wakeEnabled = true,
                foregroundPromoted = true,
            ),
        )
    }

    /**
     * Without a promotion the process is not foreground, so starting the wake
     * service would be a background service start and throw on API 31+; and a
     * service kept alive with no notification is the empty-process squat #294
     * removed. Either alone is enough to fail closed.
     */
    @Test
    fun `an opt-in does not wake when the foreground promotion was refused`() {
        assertEquals(
            MeshRestartAction.STOP,
            MeshWakePolicy.decideRestart(
                hostPresent = false,
                wakeEnabled = true,
                foregroundPromoted = false,
            ),
        )
    }

    // --- The watchdog ------------------------------------------------------

    @Test
    fun `the watchdog fires strictly after the task's own deadline`() {
        val settings = MeshWakeSettings(enabled = true, timeoutMs = 30_000L)

        assertTrue(MeshWakePolicy.watchdogDelayMs(settings) > settings.timeoutMs)
        assertEquals(
            settings.timeoutMs + MeshWakePolicy.WATCHDOG_GRACE_MS,
            MeshWakePolicy.watchdogDelayMs(settings),
        )
    }

    @Test
    fun `the watchdog stops only when no host registered`() {
        assertTrue(MeshWakePolicy.shouldStopOnWatchdog(hostPresent = false))
        assertFalse(MeshWakePolicy.shouldStopOnWatchdog(hostPresent = true))
    }
}
