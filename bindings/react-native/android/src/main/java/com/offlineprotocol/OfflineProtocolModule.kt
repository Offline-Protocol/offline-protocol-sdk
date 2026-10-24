package com.offlineprotocol

import android.app.Activity
import android.app.Application
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.util.Log
import com.facebook.react.bridge.*
import com.facebook.react.modules.core.DeviceEventManagerModule
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.max
import kotlin.math.min
import java.util.concurrent.atomic.AtomicInteger

import com.offlineprotocol.ble.BleTransportFacade

// Import generated UniFFI bindings
import uniffi.offline_protocol.*

/**
 * UniFFI-based React Native module
 */
class OfflineProtocolModule(reactContext: ReactApplicationContext) :
    ReactContextBaseJavaModule(reactContext), LifecycleEventListener {

    /**
     * Gates the foreground relay reconnect on how long the app actually stayed
     * backgrounded, fed [android.os.SystemClock.elapsedRealtime] (monotonic AND
     * sleep-inclusive). Paired with iOS's ForegroundReconnectPolicy so both
     * platforms heal on foreground automatically and identically. Main-thread
     * confined: React Native delivers the host lifecycle callbacks there.
     */
    private val foregroundReconnectPolicy = ForegroundReconnectPolicy()

    /**
     * The protocol handle.
     *
     * Written on the native-modules thread (`create`, [destroy], [invalidate])
     * and read on the main thread by the process lifecycle watcher through
     * [notifyAppStateQuietly]. Volatile for the reason [sawActivityStart] is:
     * without it the main thread can go on reading the null it saw before
     * `create`, skip the background notification, and leave open the
     * telemetry session that notification would have closed.
     */
    internal val engineHost by lazy {
        ProtocolEngineHost(logTag = NAME) { level, message, context ->
            emitDiagnostic(level, message, context)
        }
    }

    /** React context for shared bridge adapters (same module, not subclass-accessible). */
    internal val bridgeContext: Context
        get() = reactApplicationContext

    private var protocol: OfflineProtocol?
        get() = engineHost.protocol
        set(value) {
            engineHost.protocol = value
        }
    private var meshServices: MeshServices?
        get() = engineHost.meshServices
        set(value) {
            engineHost.meshServices = value
        }
    private var dataStore: DataStore?
        get() = engineHost.dataStore
        set(value) {
            engineHost.dataStore = value
        }
    private var bleTransport: BleTransportFacade?
        get() = engineHost.bleTransport
        set(value) {
            engineHost.bleTransport = value
        }
    private var internetManager: InternetManager?
        get() = engineHost.internetManager
        set(value) {
            engineHost.internetManager = value
        }
    private var wifiDirectManager: WifiDirectManager?
        get() = engineHost.wifiDirectManager
        set(value) {
            engineHost.wifiDirectManager = value
        }
    private var reticulumManager: ReticulumManager?
        get() = engineHost.reticulumManager
        set(value) {
            engineHost.reticulumManager = value
        }
    private var nostrManager: NostrManager?
        get() = engineHost.nostrManager
        set(value) {
            engineHost.nostrManager = value
        }

    /**
     * Our registration in [MeshForegroundService]'s process-global stop slot,
     * kept so teardown can clear it by identity rather than unconditionally.
     * See [releaseForegroundStopCallback].
     *
     * Volatile because `start()` writes it from the JS thread while
     * [invalidate] reads it from React Native's teardown, with no lock between
     * them: a stale read there would skip the clear and leave this module — and
     * its ReactContext — pinned by the companion field.
     */
    @Volatile
    private var foregroundStopCallback: (() -> Unit)? = null

    /**
     * How many JS subscriptions React Native believes this module has, as
     * reported by [addListener] / [removeListeners].
     *
     * Atomic because it is written from the thread React Native delivers those
     * calls on and read from wherever an event happens to originate — including
     * the `"mesh-user-stop"` thread [handleUserRequestedMeshStop] spawns, which
     * shares no lock and no happens-before edge with the writer. A plain `Int`
     * there may legally read a stale `0` while JS is fully subscribed, and
     * [sendEvent] would drop the event on it. That is not a theoretical race:
     * the event it drops is the one report of a teardown the app did not
     * initiate. Same reasoning as [foregroundStopCallback] above.
     *
     * A correct read is necessary but not sufficient — see [stickyEvents] for
     * the events that must survive a genuinely shut gate.
     */
    private val listenerCount = AtomicInteger(0)

    /**
     * Redelivers one-shot events that could not be handed to JS, on the next
     * subscribe or foreground. See [sendStickyEvent] for which events qualify
     * and why the others must not, and [StickyEventDispatcher] for the three
     * orderings that make redelivery correct.
     */
    private val stickyEvents = StickyEventDispatcher(
        buffer = StickyEventBuffer(),
        canEmit = { canEmitToJs() },
        emit = { eventJson -> sendEvent(EVENT_NAME, eventParams(eventJson)) },
        schedule = { runnable ->
            try {
                reactApplicationContext.runOnJSQueueThread(runnable)
            } catch (e: Exception) {
                // runOnJSQueueThread asserts the queue threads were
                // initialized; a context torn down under us throws
                // (AssertionException, a RuntimeException) rather than
                // returning false. The events stay held for the next trigger.
                android.util.Log.w(NAME, "Could not schedule sticky event flush", e)
                false
            }
        },
    )

    /**
     * Redelivers per-message events that could not be handed to JS (see
     * [BUFFERED_INBOUND_EVENT_TYPES] for the set and why each qualifies) on
     * the next subscribe or foreground. [holdInboundEventIfBuffered] is the
     * gate.
     *
     * A second [StickyEventDispatcher] rather than more keys in [stickyEvents]
     * because the two hold different things. A one-shot event collapses per
     * type and the buffer is sized for two keys; an inbound event is one
     * message the core has already ACKed and will never restate, so every one
     * must survive on its own key (`type:message_id`) and the cap has to be a
     * real capacity — 256, the oldest dropped past it — rather than a backstop.
     */
    private val inboundEvents = StickyEventDispatcher(
        buffer = StickyEventBuffer(maxEntries = INBOUND_EVENT_BUFFER_CAPACITY),
        canEmit = { canEmitToJs() },
        emit = { eventJson -> sendEvent(EVENT_NAME, eventParams(eventJson)) },
        schedule = { runnable ->
            try {
                reactApplicationContext.runOnJSQueueThread(runnable)
            } catch (e: Exception) {
                android.util.Log.w(NAME, "Could not schedule inbound event flush", e)
                false
            }
        },
    )

    /**
     * The started activities, which is what tells the process apart from an
     * activity, and the watcher that maintains them. Held by identity rather
     * than counted; see [installProcessLifecycleWatcher] for why, and for
     * why the boundary follows these rather than `onHostPause`.
     *
     * Written only from the main thread, where every
     * `ActivityLifecycleCallbacks` method is delivered, but **read from the
     * native-modules thread**: [enableTelemetry] seeds the pipe's starting
     * state from these, and [invalidate] tears the watcher down. Neither is
     * on main, so the set is synchronized and the flag is `@Volatile`;
     * without that a seed can read a stale set under the Java memory model
     * and start a session on the wrong side of the boundary. Declared here
     * because the `init` below assigns them, and a property initializer
     * running after that `init` would overwrite the watcher with null and
     * silently unhook the session boundary.
     */
    private val startedActivities: MutableSet<Activity> = java.util.Collections.synchronizedSet(
        java.util.Collections.newSetFromMap(java.util.IdentityHashMap<Activity, Boolean>())
    )

    /**
     * Whether the watcher has ever seen an activity start.
     *
     * Until it has, an empty [startedActivities] means "nothing observed
     * yet", not "nothing running": this module is constructed around the host
     * activity's `onCreate`, so the first `onStart` can land before the
     * watcher is registered. [seedAppState] uses this to tell the two apart.
     */
    @Volatile
    private var sawActivityStart = false
    private var processLifecycleWatcher: Application.ActivityLifecycleCallbacks? = null

    init {
        // Drive the foreground relay-heal from the host activity's lifecycle so
        // Android matches iOS: both platforms reconnect automatically on
        // foreground after a background stay long enough to have killed the
        // socket. See onHostPause/onHostResume and ForegroundReconnectPolicy.
        //
        // Must stay *below* every field onHostResume touches — today
        // foregroundReconnectPolicy and stickyEvents. `addLifecycleEventListener`
        // replays onHostResume when the host is already RESUMED, and while it
        // does so through `runOnUiQueueThread` (which always posts, so it can
        // never run inline in this constructor), depending on that to keep a
        // `val` from being read before its initializer is a hazard with no
        // upside. Declaration order costs nothing and makes it structural.
        reactContext.addLifecycleEventListener(this)
        // Telemetry session boundaries follow the process, not the activity.
        installProcessLifecycleWatcher()
    }

    private var currentConfig: ProtocolConfig?
        get() = engineHost.currentConfig
        set(value) {
            engineHost.currentConfig = value
        }

    companion object {
        const val NAME = "OfflineProtocolModule"
        const val EVENT_NAME = "OfflineProtocol_Event"
        /** Field on every [EVENT_NAME] payload carrying [eventSeq]. */
        const val EVENT_SEQ_FIELD = "seq"

        /**
         * The two *one-shot* event tags, which double as their
         * [StickyEventBuffer] keys — see [sendStickyEvent] for why only these
         * two qualify.
         *
         * Named rather than spelled out at each site because the tag and the
         * key have to agree across sites that are nowhere near each other:
         * [EVENT_INTERNET_SESSION_SUPERSEDED] is emitted in
         * [emitInternetSupersededEvent] and discarded in [enableTransport],
         * over a thousand lines apart. A typo in either would compile, pass
         * every check in CI, and silently drop half the mechanism — this module
         * cannot be unit-tested (`react-android` is `compileOnly` in the test
         * harness), and `StickyEventDispatcherTest` necessarily uses its own
         * literals, so nothing else is watching. A constant makes the
         * key-equals-tag invariant the compiler's problem.
         */
        private const val EVENT_MESH_STOPPED_BY_USER = "mesh_stopped_by_user"

        /**
         * @see EVENT_MESH_STOPPED_BY_USER
         *
         * Aliased from [SupersededLatchPolicy.EVENT_TYPE] rather than spelled
         * again, so the tag has exactly one definition across both platforms —
         * and, unlike a literal here, one a unit test can pin (this module has
         * no test harness; the policy does, on both platforms).
         */
        private const val EVENT_INTERNET_SESSION_SUPERSEDED = SupersededLatchPolicy.EVENT_TYPE

        /** How long `destroy` waits for an in-flight process tick to finish. */
        private const val PROCESS_SHUTDOWN_TIMEOUT_MS = 2_000L

        /**
         * The inbound event tags [inboundEvents] holds when JS cannot take
         * them. Each reports one message the core has already ACKed,
         * dedup-marked and dropped its queued copy of — the sender will not
         * resend and nothing will restate it — so a drop here is a lost
         * message, and the drop is ordinary: the React instance is down while
         * the app is backgrounded, or a push injection lands before JS has
         * subscribed.
         *
         * `message_delivered` and `message_failed` are the sender's half of
         * the same fact (#514): each settles one message for good, after the
         * core has already removed its pending acknowledgement, retry and
         * outbox entry, so nothing will restate it either, and a dropped one
         * leaves the app showing a message as in flight forever. Must match
         * `BUFFERED_INBOUND_EVENT_TYPES` in
         * `src/constants.ts` and `InboundEventBuffer.bufferedEventTypes` on
         * iOS; pinned by `react_native_buffered_inbound_event_set_matches_native`.
         */
        private val BUFFERED_INBOUND_EVENT_TYPES: Set<String> = setOf(
            "message_received",
            "file_received",
            "message_decryption_failed",
            "message_delivered",
            "message_failed",
        )

        /** Inbound events held at most; the oldest is dropped past it. */
        private const val INBOUND_EVENT_BUFFER_CAPACITY = 256
    }
    
    private object Constants {
        // One definition, in the applier both bridges share.
        const val MIN_BATTERY_LEVEL = InitialRuntimeConfigApplier.Limits.MIN_BATTERY_LEVEL
        const val MAX_BATTERY_LEVEL = InitialRuntimeConfigApplier.Limits.MAX_BATTERY_LEVEL
        const val MIN_HISTORY_WINDOW = InitialRuntimeConfigApplier.Limits.MIN_HISTORY_WINDOW
        const val MAX_HISTORY_WINDOW = InitialRuntimeConfigApplier.Limits.MAX_HISTORY_WINDOW
        const val BLE_RESTART_DELAY_MS = 1000L
        // Matches iOS 100ms tick. Handles retries, ACK timeouts, welcome
        // processing, and DORS. Latency-sensitive work is also event-driven.
        const val PROCESS_INTERVAL_MS = 100L
        const val MAX_RECEIVE_DRAIN_PER_TICK = 100
        const val LOG_INTERVAL_MS = 5000L
        const val LOG_INTERVAL_THRESHOLD_MS = 100L
        const val DEFAULT_RSSI_THRESHOLD: Short = -85
        const val DEFAULT_CONGESTION_QUEUE = 50L
        const val DEFAULT_STABILITY_WINDOW = 8L
        const val DEFAULT_QUEUE_RECOVERY_RATIO = 0.5f
        const val HTTPS_PORT = 443
        const val HTTP_PORT = 80
        const val MILLISECONDS_PER_SECOND = 1000L
    }

    override fun getName(): String = NAME

    /**
     * Deliberately NOT `@Synchronized`, and deliberately inline rather than
     * routed through [stopTransportsAndProtocol]: React Native may call this
     * while holding its own teardown lock, so taking the module monitor here —
     * and holding it across calls that reach back into RN internals — is a
     * lock-order inversion. The residual is a double-stop of a transport racing
     * `stop()`, which every transport tolerates. Please don't "fix" that by
     * adding the annotation.
     *
     * The steps still run through [TeardownSequence] — a plain helper that
     * takes no locks, so it does not reintroduce that inversion — because a
     * throwing transport must not skip the ones after it, the keep-alive
     * service, or the callback clear that unpins this module. Failures only
     * reach logcat: `invalidate` has no error channel, and reporting through
     * [emitDiagnostic] would push events into a ReactContext that is already
     * going down.
     */
    override fun invalidate() {
        super.invalidate()
        val teardown = TeardownSequence()

        teardown.step("lifecycle listener") {
            reactApplicationContext.removeLifecycleEventListener(this)
        }
        teardown.step("process lifecycle watcher") { removeProcessLifecycleWatcher() }
        teardown.step("process scheduler") { engineHost.backgroundProcessor.stopProcessScheduler() }

        // The null-outs sit outside their steps so a transport that throws on
        // stop is still released.
        teardown.step("BLE manager") { bleTransport?.stop() }
        bleTransport = null
        teardown.step("Internet manager") { internetManager?.stop() }
        internetManager = null
        teardown.step("WiFi Direct manager") { wifiDirectManager?.stop() }
        wifiDirectManager = null
        teardown.step("Reticulum manager") { reticulumManager?.stop() }
        reticulumManager = null
        teardown.step("Nostr manager") { nostrManager?.stop() }
        nostrManager = null
        // Same reason as in [destroy]: the handle owns the telemetry pipe's
        // uploader thread, and invalidate is the last moment this module can
        // release it deterministically.
        val protocolHandle = protocol
        protocol = null
        teardown.step("protocol handle") { protocolHandle?.destroy() }

        teardown.step("mesh foreground service") { stopForegroundService() }
        releaseForegroundStopCallback()

        for (failure in teardown.failures) {
            android.util.Log.w(NAME, "Teardown step failed during invalidate: ${failure.step}", failure.cause)
        }
    }

    /**
     * Drops this module's registration from [MeshForegroundService] — a
     * companion field that captures the module, so leaving it set outlives the
     * module and its ReactContext for the process lifetime.
     *
     * Clears by identity: the slot is process-global while modules are
     * per-ReactContext, so during a reload this module may be tearing down
     * *after* its replacement registered, and an unconditional null would
     * disarm the live host's Stop action.
     *
     * A backstop, not the main path, and idempotent so it stays cheap as one:
     * mesh teardown surrenders the registration through [stopForegroundService]
     * instead, because the slot is the service's sticky-restart liveness signal
     * and must fall with the mesh rather than with the module. This covers a
     * teardown that never reached that step.
     */
    private fun releaseForegroundStopCallback() {
        val ours = foregroundStopCallback ?: return
        foregroundStopCallback = null
        MeshForegroundService.clearStopRequestCallback(ours)
    }

    // MARK: - Host lifecycle (foreground relay heal)

    /**
     * App went to background. Record the moment so [onHostResume] can measure
     * the stay. A background long enough to have killed the relay TCP (Doze, OS
     * freeze, network handoff) leaves the cached ready flags stale-true against
     * a dead socket, which only a full reconnect heals.
     */
    override fun onHostPause() {
        foregroundReconnectPolicy.didEnterBackground(nowMs = android.os.SystemClock.elapsedRealtime())
        // No telemetry session boundary here. `onHostPause` is
        // `Activity.onPause`, which fires for a permission dialog or a share
        // sheet as readily as for the app going away — good enough to arm the
        // relay heal above, which only measures a stay, and too coarse to end
        // a session. See [installProcessLifecycleWatcher].
    }

    /**
     * App returned to foreground. Proactively heal a socket the OS likely killed
     * while backgrounded: `isReady()` cannot tell a healthy socket from a zombie
     * (both report connected+authenticated), so gate on background duration
     * instead (see ForegroundReconnectPolicy). forceReconnect() no-ops unless
     * the transport is running/starting and resets the reconnect backoff, so it
     * is safe to call unconditionally within the gate. Mirrors iOS's
     * applicationWillEnterForeground — apps no longer need to call
     * forceInternetReconnect() on foreground themselves.
     */
    override fun onHostResume() {
        if (foregroundReconnectPolicy.shouldReconnectOnForeground(nowMs = android.os.SystemClock.elapsedRealtime())) {
            internetManager?.forceReconnect()
        }
        // No telemetry session boundary here either; its pair lives in
        // installProcessLifecycleWatcher.
        // The other flush trigger besides [addListener]. An app whose listeners
        // never went away still needs one: a sticky event held because the React
        // instance was briefly down would otherwise wait for a resubscribe that
        // is never coming.
        flushStickyEvents()
    }

    override fun onHostDestroy() {
        // No-op: teardown is handled by invalidate(); nothing lifecycle-specific
        // to release here.
    }

    // MARK: - Process lifecycle (telemetry session boundaries)

    /**
     * Draws the telemetry session boundary on the *process* leaving the
     * foreground, not on an activity being paused.
     *
     * [onHostPause] is `Activity.onPause`, which fires for anything that comes
     * in front of the host: a runtime permission dialog — including the
     * Bluetooth one this SDK itself triggers on first launch — the system
     * share sheet, any translucent activity. A boundary there emits a session
     * summary and rotates the session id every time a user grants a
     * permission, so the same user journey would be counted as several
     * sessions on Android and one on iOS, which uses `didEnterBackground` and
     * deliberately ignores the transient `Inactive` state. Every activity
     * being stopped is the signal that actually means "not visible", and is
     * what `androidx.lifecycle`'s `ProcessLifecycleOwner` is built on;
     * counting it here keeps that dependency off every consumer of this
     * package.
     *
     * A configuration change (a rotation) stops and restarts the activity
     * without the app going anywhere, so `isChangingConfigurations` gates the
     * background edge. The matching `ACTIVE` needs no such gate: the pipe
     * debounces its own edges, so an `ACTIVE` with no `BACKGROUND` before it
     * is not an edge and does nothing.
     *
     * The started activities are held as an identity set rather than counted.
     * React Native constructs its native modules after the host activity's
     * `onStart`, so a counter starts at zero while an activity is already
     * visible, and the first stop it sees is unmatched. Clamping that at zero
     * hides it only while there is one activity: open a second in-process
     * activity (a sign-in hub, an image picker, a payment sheet) and the
     * sequence is start B, stop A, which takes the count to zero and fires a
     * background edge with the app fully on screen — then start A on the way
     * back, rotating the session. One journey becomes two sessions on
     * Android and one on iOS, which is the asymmetry this whole function
     * exists to avoid. A set is right whether or not the first activity was
     * ever tracked: an untracked A stopping while B is tracked leaves the set
     * non-empty, and the edge waits for B.
     */
    private fun installProcessLifecycleWatcher() {
        val application = reactApplicationContext.applicationContext as? Application
        if (application == null) {
            Log.w(
                NAME,
                "No Application context: telemetry sessions will not follow the process lifecycle"
            )
            return
        }
        val watcher = object : Application.ActivityLifecycleCallbacks {
            override fun onActivityStarted(activity: Activity) {
                sawActivityStart = true
                startedActivities.add(activity)
                notifyAppStateQuietly(AppState.ACTIVE)
            }

            override fun onActivityStopped(activity: Activity) {
                startedActivities.remove(activity)
                if (startedActivities.isEmpty() && !activity.isChangingConfigurations) {
                    // The pipe emits its summary and wakes the uploader
                    // thread; nothing here blocks.
                    notifyAppStateQuietly(AppState.BACKGROUND)
                }
            }

            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}
            override fun onActivityResumed(activity: Activity) {}
            override fun onActivityPaused(activity: Activity) {}
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}

            override fun onActivityDestroyed(activity: Activity) {
                // An activity destroyed without a matching stop would sit in
                // the set forever and hold the background edge off.
                startedActivities.remove(activity)
            }
        }
        application.registerActivityLifecycleCallbacks(watcher)
        processLifecycleWatcher = watcher
    }

    private fun removeProcessLifecycleWatcher() {
        val watcher = processLifecycleWatcher ?: return
        processLifecycleWatcher = null
        // Unregister first, then clear. The other order lets a callback
        // already in flight on the main thread mutate the set this thread is
        // clearing, and leaves an entry behind in the set it just emptied.
        (reactApplicationContext.applicationContext as? Application)
            ?.unregisterActivityLifecycleCallbacks(watcher)
        startedActivities.clear()
    }

    /**
     * Reports a lifecycle transition to the pipe, absorbing the throw a
     * concurrently destroyed handle raises.
     *
     * This runs on the main thread, from `ActivityLifecycleCallbacks`, while
     * [destroy] and [invalidate] run on the native-modules thread. UniFFI
     * throws `IllegalStateException` once a handle's call counter has closed,
     * and an exception escaping an `ActivityLifecycleCallbacks` method takes
     * the process down. Logging out while the app is being backgrounded is
     * exactly that race. Losing the boundary is the correct outcome there:
     * the pipe it would have reported to is already stopped, and its final
     * flush ran on the teardown path.
     *
     * Every exception, not just that one. The reason for catching is the
     * consequence of not catching, and taking the process down is the same
     * consequence whatever the type: UniFFI surfaces a panic in the core as
     * `InternalException`, which is not an `IllegalStateException`, and a
     * telemetry lifecycle hint is never worth a crash.
     */
    private fun notifyAppStateQuietly(state: AppState) {
        try {
            protocol?.notifyAppState(state)
        } catch (e: Exception) {
            Log.d(NAME, "Telemetry lifecycle transition skipped: the protocol handle is gone", e)
        }
    }

    /**
     * The application state to seed a new telemetry pipe with.
     *
     * This has to be measured the way the watcher measures its edges, not the
     * way React Native reports its own lifecycle. `lifecycleState` is
     * `RESUMED` only between `onResume` and `onPause`, so it reads
     * `BEFORE_RESUME` while a permission dialog or share sheet sits over a
     * fully visible app. A pipe seeded `BACKGROUND` there has its boundary
     * armed with nothing to disarm it, because dismissing a dialog fires
     * `onActivityResumed` and no `onActivityStarted`: the next real
     * background is then read as a non-edge and reports nothing at all. The
     * demo app calls `enableTelemetry` immediately after `start()`, which is
     * exactly when a first-run permission prompt is on screen.
     *
     * Three sources, in falling order of authority:
     *
     * 1. The watcher's own set, once it has seen a start. This is the same
     *    signal the edges use, so a seed from it can never disagree with them.
     * 2. A live current activity. React Native constructs its native modules
     *    around the host activity's `onCreate`, so the watcher can miss that
     *    activity's `onStart` and never see one; `currentActivity` survives a
     *    pause and is cleared only on host destroy, so a non-null value still
     *    means the process has a foreground activity.
     * 3. Otherwise `BACKGROUND`: no activity has ever resumed, which is what
     *    a headless mesh wake or a service-only launch looks like.
     *
     * The order also puts the cheaper error first. Seeding `ACTIVE` while
     * actually backgrounded costs one missed rotation and no data. Seeding
     * `BACKGROUND` while actually foregrounded arms the boundary and loses
     * the next real session summary, which is the bug this replaced.
     */
    private fun seedAppState(): AppState = when {
        sawActivityStart -> if (startedActivities.isEmpty()) AppState.BACKGROUND else AppState.ACTIVE
        reactApplicationContext.currentActivity != null -> AppState.ACTIVE
        else -> AppState.BACKGROUND
    }

    @ReactMethod
    fun addListener(eventName: String) {
        listenerCount.incrementAndGet()
        // A subscription is the moment a held one-shot event becomes
        // deliverable. The flush defers itself onto the JS queue rather than
        // emitting here — see [flushStickyEvents] for why that is required
        // rather than tidy.
        flushStickyEvents()
    }

    @ReactMethod
    fun removeListeners(count: Double) {
        listenerCount.updateAndGet { (it - count.toInt()).coerceAtLeast(0) }
    }

    /**
     * Accepts both the current vocabulary and the pre-0.22 `low`/`medium`/`high`
     * spelling of the same three values, so an app that has not migrated its
     * config keeps working.
     */
    private fun normalizeRelayPriority(priority: String?): RelayPriority? =
        InitialRuntimeConfigApplier.normalizeRelayPriority(priority)

    private fun relayPriorityName(priority: RelayPriority?): String =
        InitialRuntimeConfigApplier.relayPriorityName(priority)

    private fun applyInitialRuntimeConfig(proto: OfflineProtocol, json: JSONObject) {
        InitialRuntimeConfigApplier.apply(proto, json) { level, message, context ->
            emitDiagnostic(level, message, context ?: emptyMap())
        }
    }

    /**
     * Rejects a shared-bridge [promise]. With [mapProtocolErrors] the error
     * goes through [rejectWithProtocolError], so a mapped error rejects with
     * its stable typed code (see ProtocolErrorBridge.kt); without it the
     * method keeps the fixed fallback code it has always rejected with.
     *
     * Per method, from `tools/rn_engine_bridge_error_manifest.json`, and true
     * exactly for the methods that mapped before the shared bridge existed.
     * Mapping every method would change the code an app sees for the others;
     * mapping none drops `NoKeyPackage` and `SessionNotReady` from the sends.
     * iOS makes the same choice from the same file.
     */
    internal fun rejectBridgePromise(
        promise: Promise,
        error: Throwable,
        fallbackCode: String,
        fallbackMessage: String,
        mapProtocolErrors: Boolean,
    ) {
        if (mapProtocolErrors) {
            rejectWithProtocolError(promise, error, fallbackCode, fallbackMessage)
        } else {
            promise.reject(fallbackCode, "$fallbackMessage: ${error.message}", error)
        }
    }

    private fun rejectWithProtocolError(
        promise: Promise,
        error: Throwable,
        fallbackCode: String,
        fallbackMessage: String
    ) {
        val mapped = mapProtocolBridgeError(error)
        if (mapped != null) {
            promise.reject(mapped.code, mapped.message, error)
        } else {
            promise.reject(fallbackCode, "$fallbackMessage: ${error.message}", error)
        }
    }

    @ReactMethod
    fun create(configJson: String, promise: Promise) {
        try {
            val internetHandlers = ProtocolEngineHost.InternetTransportEventHandlers(
                serverMessageEmitter = { rawJson -> emitServerMessageEvent(rawJson) },
                connectionStatusEmitter = { connected, authenticated ->
                    emitInternetStatusEvent(connected, authenticated)
                },
                supersededEmitter = { reason -> emitInternetSupersededEvent(reason) },
            )
            engineHost.create(
                reactApplicationContext,
                configJson,
                object : EventCallback {
                    override fun onEvent(eventJson: String) {
                        try {
                            if (!sendEvent(EVENT_NAME, eventParams(eventJson))) {
                                holdInboundEventIfBuffered(eventJson)
                            }
                        } catch (e: Exception) {
                            android.util.Log.e(NAME, "Core event not handed to JS: ${eventTypeOf(eventJson)}", e)
                            try {
                                holdInboundEventIfBuffered(eventJson)
                            } catch (held: Exception) {
                                android.util.Log.e(NAME, "Core event could not be held either", held)
                            }
                        }
                    }
                },
                internetHandlers,
            )
            promise.resolve(null)
        } catch (e: Exception) {
            emitDiagnostic("error", "Failed to create protocol", mapOf(
                "message" to (e.message ?: "unknown"),
                "exception" to e.javaClass.simpleName
            ))
            promise.reject("ERROR_CREATE", "Failed to create protocol: ${e.message}", e)
        }
    }
    
    /**
     * Whether an emit has any chance of reaching JS: something is subscribed,
     * and there is a live React instance to carry it.
     *
     * Shared by [sendEvent] and the paths that want to know the answer *before*
     * building a `WritableMap` they may not use. Never a delivery guarantee —
     * see [sendEvent] for why nothing at this layer can offer one.
     */
    private fun canEmitToJs(): Boolean =
        listenerCount.get() > 0 && reactApplicationContext.hasActiveReactInstance()

    /** The one-field payload every event on [EVENT_NAME] travels in. */
    private fun eventParams(eventJson: String): WritableMap =
        Arguments.createMap().apply { putString("eventJson", eventJson) }

    /**
     * Hands an event to JavaScript, reporting whether it got that far.
     *
     * Returns false when the gate is shut — no JS subscription, or no live
     * React instance — and when the emit itself throws. Most callers ignore the
     * result: their events are periodic, re-derivable, or followed by another
     * carrying the same state, so a drop costs nothing. [sendStickyEvent] is
     * for the ones where it costs everything.
     *
     * The [ReactContext.hasActiveReactInstance] check is a *precondition*, not
     * belt-and-braces around the catch, because on the New Architecture there
     * is nothing to catch: `getJSModule` returns a proxy that forwards to
     * `ReactHost.callFunctionOnModule`, which reports failure by rejecting a
     * Task rather than throwing. "Did not throw" is therefore not evidence of
     * delivery on bridgeless, and this guard is the only signal available.
     *
     * Even a true return is not a delivery receipt — nothing here observes JS
     * receiving the event. It means the event was handed over with the instance
     * alive and a subscription registered, which is as much as this layer can
     * know. [flushStickyEvents] is built around that limit.
     *
     * It is not even a receipt for *this* event name. React Native's
     * [addListener] reports the name but [removeListeners] reports only a count,
     * so the subscription tally cannot be kept per name and [listenerCount] is
     * shared across [EVENT_NAME] and [TELEMETRY_EVENT_NAME]. A subscription to
     * one therefore opens the gate for the other. In-tree that cannot bite —
     * the SDK's constructor subscribes to both together — but it is a real
     * limit on what a `true` means, and it is the sticky path that leans on
     * the answer.
     */
    private fun sendEvent(eventName: String, params: Any?): Boolean {
        if (!canEmitToJs()) {
            return false
        }
        // Numbered under the lock and handed over inside it, so the numbers
        // reach JS in the order they were given out whichever thread drove
        // the core. See [eventSeq].
        // A number is spent only on an emit that returned: one that threw
        // handed nothing over, and the one-shot hold redelivers it later, so
        // consuming its number would report a gap for an event never lost.
        return synchronized(eventSeqLock) {
            val numbered = eventName == EVENT_NAME && params is WritableMap
            // Read before the emit: the map is handed over by transfer, and a
            // consumed native map cannot be read back afterwards.
            var replayJson: String? = null
            if (numbered) {
                val map = params as WritableMap
                replayJson = if (map.hasKey("eventJson")) map.getString("eventJson") else null
                map.putInt(EVENT_SEQ_FIELD, eventSeq)
            }
            try {
                reactApplicationContext
                    .getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
                    .emit(eventName, params)
                if (numbered) {
                    replayJson?.let { eventReplay.record(eventSeq, it) }
                    eventSeq += 1
                }
                true
            } catch (e: Exception) {
                android.util.Log.w(NAME, "Event emit failed for $eventName", e)
                false
            }
        }
    }

    /**
     * The number the next event handed to JS on [EVENT_NAME] carries, so the
     * TypeScript layer can see one go missing (#514).
     *
     * Nothing else here can: a bridgeless emit that fails inside React Native
     * returns normally (see [sendEvent]), and the core's own reasons for
     * acknowledging a message without surfacing it are `tracing` lines that
     * never reach a device log. A gap in these numbers at JS says the event
     * left this module and was lost in React Native; no gap, and no event,
     * says it never reached this module. Detection, not recovery: a missing
     * event is reported, never refetched. Only events handed over are
     * numbered, so a refused gate (an event held, or dropped by design
     * before JS subscribes) leaves no gap.
     */
    private var eventSeq = 0
    private val eventSeqLock = Any()

    /**
     * Every event recently handed over on [EVENT_NAME], by its [eventSeq], so
     * the TypeScript layer can fetch one React Native lost instead of only
     * reporting it. Recorded under [eventSeqLock], in the same block that
     * spends the number. See [EventReplayRing] and [replayEvents].
     */
    private val eventReplay = EventReplayRing()

    /**
     * Hands back the events numbered [fromSeq] to [toSeq] that are still kept,
     * as `{seq, eventJson}` in order (#514). The TypeScript layer calls this
     * when a number it expected never arrived, and reports as lost only what
     * this cannot serve. A pull, so not gated on [canEmitToJs]: whoever asks
     * is JavaScript, and it is live.
     */
    @ReactMethod
    fun replayEvents(fromSeq: Double, toSeq: Double, promise: Promise) {
        try {
            val replayed = Arguments.createArray()
            eventReplay.replay(fromSeq.toInt(), toSeq.toInt()).forEach { entry ->
                replayed.pushMap(Arguments.createMap().apply {
                    putInt(EVENT_SEQ_FIELD, entry.seq)
                    putString("eventJson", entry.eventJson)
                })
            }
            promise.resolve(replayed)
        } catch (e: Exception) {
            promise.reject("ERROR_EVENT_REPLAY", "Failed to replay events", e)
        }
    }

    /**
     * Emits a *one-shot* event, holding it for redelivery if JS could not take
     * it. For everything else, [sendEvent] and its silent drop are correct.
     *
     * The bar for enrolling an event here is narrow: nothing else must ever
     * restate it. `mesh_stopped_by_user` is the terminal event of the mesh
     * lifecycle — after it, every transport, the scheduler and the core are
     * down, so there is no later event to carry the same news, and no periodic
     * signal to re-derive it from. `internet_session_superseded` is the same
     * shape for the relay: InternetManager latches the transport stopped and
     * refuses every reconnect path until an explicit start(), so a dropped emit
     * means the app never learns it is connected elsewhere.
     *
     * Sticking a *periodic* event would be actively wrong — a held
     * `internet_status_changed` replayed minutes later reports a link state
     * that has since changed, which is worse than the drop it replaced. That is
     * why the sticky set is a decision at the call site rather than a filter
     * over event JSON: a filter would have to parse every event that crosses
     * this bridge, including the core callback's, to answer a question only
     * two call sites ever ask.
     *
     * [key] identifies the event for last-wins collapsing, so a repeated stop
     * cannot accumulate copies. Callers pass the event's `type` tag.
     *
     * The orderings that make redelivery correct — generation read before the
     * emit, a successful hold re-running the flush, and the hop onto the JS
     * queue — live in [StickyEventDispatcher], where they are unit-tested.
     */
    private fun sendStickyEvent(key: String, eventJson: String) {
        stickyEvents.send(key, eventJson)
    }

    /**
     * Redelivers held one-shot and inbound events, if JS looks able to take
     * them now.
     */
    private fun flushStickyEvents() {
        stickyEvents.flush()
        inboundEvents.flush()
    }

    /**
     * Holds an inbound message event for redelivery when JS could not take
     * it, keyed `type:message_id` so every message survives on its own.
     * Returns whether the event was one of [BUFFERED_INBOUND_EVENT_TYPES] and
     * was handed to [inboundEvents]; anything else stays dropped, as before.
     *
     * Only called after [sendEvent] refused the event, so the parse here is
     * off the hot path. An event of a buffered type that carries no id at
     * all is keyed by arrival order rather than dropped: losing a message to
     * a missing field would be the failure this buffer exists to prevent.
     */
    private fun holdInboundEventIfBuffered(eventJson: String): Boolean {
        val json = try {
            JSONObject(eventJson)
        } catch (e: Exception) {
            return false
        }
        val type = json.optString("type", "")
        if (type !in BUFFERED_INBOUND_EVENT_TYPES) return false
        val id = json.optString("message_id", "")
            .ifEmpty { json.optString("file_id", "") }
            .ifEmpty { "seq-${inboundEventSequence.incrementAndGet()}" }
        inboundEvents.send("$type:$id", eventJson)
        return true
    }

    /** The event's `type` tag, for a log line that must not carry its fields. */
    private fun eventTypeOf(eventJson: String): String = try {
        JSONObject(eventJson).optString("type", "unknown")
    } catch (e: Exception) {
        "unparseable"
    }

    /** Fallback key source for a buffered inbound event that carries no id. */
    private val inboundEventSequence = AtomicInteger(0)

    private fun emitDiagnostic(level: String, message: String, context: Map<String, Any?> = emptyMap()) {
        try {
            val json = JSONObject()
            json.put("type", "diagnostic")
            json.put("level", level)
            json.put("message", message)

            if (context.isNotEmpty()) {
                val contextJson = JSONObject()
                context.forEach { (key, value) ->
                    when (value) {
                        null -> contextJson.put(key, JSONObject.NULL)
                        else -> contextJson.put(key, value)
                    }
                }
                json.put("context", contextJson)
            }

            val params = Arguments.createMap().apply {
                putString("eventJson", json.toString())
            }
            sendEvent(EVENT_NAME, params)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to emit diagnostic event", e)
        }
    }

    /**
     * Forwards an internet-transport readiness transition as the
     * `internet_status_changed` event. `authenticated: true` is the
     * positive gate for raw server commands (replaces app-side
     * `relayStatus === 'authenticated'` tracking).
     */
    private fun emitInternetStatusEvent(connected: Boolean, authenticated: Boolean) {
        try {
            val json = JSONObject()
            json.put("type", "internet_status_changed")
            json.put("connected", connected)
            json.put("authenticated", authenticated)
            val params = Arguments.createMap().apply {
                putString("eventJson", json.toString())
            }
            sendEvent(EVENT_NAME, params)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to emit internet status event", e)
        }
    }

    /**
     * Forwards a relay session displacement ("superseded") as the
     * `internet_session_superseded` event. The relay closed this socket with
     * code 4000 because a newer registration for the same identity took over;
     * the SDK will NOT auto-reconnect. The app surfaces "connected elsewhere"
     * and reconnects only on explicit user action (re-enabling the transport).
     *
     * Sticky: InternetManager latches the transport stopped on displacement and
     * refuses auto- and force-reconnect until an explicit start(), so nothing
     * restates this. Dropped, the app is left showing a relay connection that
     * will never come back.
     */
    private fun emitInternetSupersededEvent(reason: String?) {
        try {
            sendStickyEvent(
                EVENT_INTERNET_SESSION_SUPERSEDED,
                SupersededLatchPolicy.eventJson(reason)
            )
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to emit internet superseded event", e)
        }
    }

    /**
     * Forwards a raw relay frame apps need outside or in addition to
     * SDK-owned processing (group snapshot extensions, invite links, role
     * changes, rate limiting, unknown types) as the
     * `internet_server_message` event.
     */
    private fun emitServerMessageEvent(rawJson: String) {
        try {
            val json = JSONObject()
            json.put("type", "internet_server_message")
            json.put("json", rawJson)
            val params = Arguments.createMap().apply {
                putString("eventJson", json.toString())
            }
            sendEvent(EVENT_NAME, params)
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to emit server message event", e)
        }
    }

    @ReactMethod
    fun start(promise: Promise) {
        try {
            // A one-shot event held from the *previous* mesh session is moot the
            // moment a new one comes up, and redelivering it would be the bug
            // this whole mechanism exists to prevent, inverted: an app told
            // "mesh stopped" or "relay superseded" just after starting has
            // nothing to restate it.
            //
            // Not redundant with destroy()'s endSession(): destroy() is *not*
            // the only way the gate shuts — canEmitToJs() also refuses while
            // there is no live React instance — so an event can be held across
            // a window the app tore nothing down for, and still be waiting
            // here. Which means this can discard a genuinely pending event: the
            // flush onHostResume posts runs on the JS queue, so an app calling
            // start() from its own foreground handler can bump the generation
            // first. That is the right trade — the app is bringing the mesh
            // back up, which is what a held stop would have told it to
            // reconcile against — and it is why the getState() reconcile in
            // docs/react-native-integration.md §6.1 is documented as the
            // belt-and-braces rather than as an optional extra.
            //
            // That trade is stated for a start() that *succeeds*, and this runs
            // before the one that can throw, so a failed start() discards a
            // held stop as well. Still right, but for a different reason: the
            // caller learns the mesh is not up from its own rejected promise,
            // which is strictly more than the held event would have told it.
            // Kept first rather than moved after protocol.start() so there is
            // no window where a start() is underway with the buffer still
            // closed by the previous destroy().
            //
            // beginSession(), not endSession(): this is a session *starting*,
            // so the buffer must take what follows. destroy() is the transition
            // that closes it. Collapsing the two is how an event emitted by the
            // session destroy() just ended ends up held for the next one.
            stickyEvents.beginSession()
            // The inbound buffer follows the same session edges: nothing
            // buffered can be produced before this start(), and a message held
            // for the previous session belongs to whatever identity ran it.
            inboundEvents.beginSession()
            emitDiagnostic("info", "Starting protocol")
            protocol?.start()
            emitDiagnostic("info", "Protocol core started")

            // Start foreground service to protect the process from being killed
            startForegroundService()
            
            //  Start BLE manager if available - BLE should work independently
            // BLE peer discovery and messaging must work even when Internet/WiFi are disabled
            bleTransport?.let { manager ->
                try {
                    android.util.Log.i(NAME, "Starting BLE manager (BLE should work independently of other transports)...")
                    emitDiagnostic("info", "Starting BLE manager", mapOf(
                        "internetEnabled" to (currentConfig?.internetEnabled ?: false),
                        "wifiDirectEnabled" to (currentConfig?.wifiDirectEnabled ?: false)
                    ))
                    manager.start()
                    android.util.Log.i(NAME, "BLE Manager started successfully - scanning and advertising should be active")
                    emitDiagnostic("info", "BLE manager started - peer discovery active", mapOf(
                        "scanning" to true,
                        "advertising" to true
                    ))
                    
                    // bleStatusChanged(true) is called inside BleTransportFacade.start()
                    // after advertising and scanning are both active — no backup timer needed.
                } catch (e: Exception) {
                    android.util.Log.e(NAME, "FAILED to start BLE Manager!", e)
                    android.util.Log.e(NAME, "Error type: ${e.javaClass.simpleName}")
                    android.util.Log.e(NAME, "Error message: ${e.message}")
                    android.util.Log.e(NAME, "Stack trace: ", e)
                    emitDiagnostic("error", "Failed to start BLE manager", mapOf(
                        "message" to (e.message ?: "unknown"),
                        "exception" to e.javaClass.simpleName,
                        "stackTrace" to e.stackTraceToString()
                    ))
                    // Don't fail the entire start if BLE fails, but log the error clearly
                    android.util.Log.w(NAME, "Protocol will continue without BLE, but peer discovery and BLE messaging will not work")
                }
            } ?: run {
                android.util.Log.w(NAME, "BLE manager is null - BLE was not initialized. Check if bleEnabled=true in config.")
                emitDiagnostic("warning", "BLE manager is null - BLE not initialized", mapOf(
                    "bleEnabled" to (currentConfig?.bleEnabled ?: false)
                ))
            }
            
            promise.resolve(null)
        } catch (e: Exception) {
            emitDiagnostic("error", "Failed to start protocol", mapOf(
                "message" to (e.message ?: "unknown"),
                "exception" to e.javaClass.simpleName
            ))
            promise.reject("ERROR_START", "Failed to start protocol: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun emitTestEvent(promise: Promise) {
        try {
            protocol?.emitTestEvent()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_TEST_EVENT", "Failed to emit test event: ${e.message}", e)
        }
    }

    @ReactMethod
    fun enableTelemetry(configMap: ReadableMap?, promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.reject("NOT_STARTED", "Protocol not created", null)
            return
        }
        try {
            engineHost.enableTelemetry(BridgeArgumentMaps.readableMapToMap(configMap), seedAppState())
            promise.resolve(null)
        } catch (e: Exception) {
            val mapped = mapProtocolBridgeError(e)
            if (mapped != null) {
                promise.reject(mapped.code, mapped.message, e)
            } else {
                promise.reject("TELEMETRY_ENABLE", "Failed to enable telemetry: ${e.message}", e)
            }
        }
    }

    @ReactMethod
    fun disableTelemetry(promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.reject("NOT_STARTED", "Protocol not created", null)
            return
        }
        try {
            engineHost.disableTelemetry()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("TELEMETRY_DISABLE", "Failed to disable telemetry: ${e.message}", e)
        }
    }

    @ReactMethod
    fun flushTelemetry(promise: Promise) {
        try {
            engineHost.flushTelemetry()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("TELEMETRY_FLUSH_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun telemetryStats(promise: Promise) {
        try {
            val stats = engineHost.telemetryStats()
            promise.resolve(stats?.let { encodeTelemetryStats(it) })
        } catch (e: Exception) {
            promise.reject("TELEMETRY_STATS_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun endTelemetrySession(promise: Promise) {
        try {
            // Blocks for the flush (at most three seconds), off the UI
            // thread, but on the single native-modules thread: every other
            // native call queues behind it for that long.
            engineHost.endTelemetrySession()
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("TELEMETRY_END_SESSION_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun setTelemetryEnabled(enabled: Boolean, promise: Promise) {
        try {
            engineHost.setTelemetryEnabled(enabled)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("TELEMETRY_SET_ENABLED_FAILED", e.message, e)
        }
    }

    @ReactMethod
    fun telemetryInstallId(promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.reject("NOT_STARTED", "Protocol not created", null)
            return
        }
        try {
            promise.resolve(engineHost.telemetryInstallId())
        } catch (e: Exception) {
            promise.reject("TELEMETRY_INSTALL_ID", "Failed to read telemetry install id: ${e.message}", e)
        }
    }

    // The TS `TelemetryConfig` type (bindings/react-native/src/types.ts)
    // only emits camelCase keys — this parser matches that contract.
    //
    // The two required strings are forwarded as given, an absent one as "":
    // the core validates and names the refused field, which reaches JS as
    // the `TelemetryConfigInvalid` code. Restating the check here would be
    // a second copy of the rule.
    //
    // The platform fields are filled here, never read from the map: the
    // ingest keys its per-platform aggregates on them, and an app cannot
    // set them wrong if it cannot set them.
    //
    // Unrecognised `mlsVerbosity` strings fall back to null (Rust default)
    // with a `Log.w` — silent fallback would have hid integrator typos
    // behind a config that "just works" at Lifecycle.
    private fun parseTelemetryConfig(map: ReadableMap?): TelemetryConfig {
        val m: ReadableMap = map ?: Arguments.createMap()
        val verbosity = readOptionalString(m, "mlsVerbosity")?.let { raw ->
            when (raw.lowercase()) {
                "off" -> MlsVerbosity.OFF
                "diagnostic" -> MlsVerbosity.DIAGNOSTIC
                "lifecycle" -> MlsVerbosity.LIFECYCLE
                else -> {
                    Log.w(
                        NAME,
                        "telemetry: unknown mlsVerbosity '$raw' — expected 'off', 'lifecycle', or 'diagnostic'. Falling back to the Rust default (lifecycle)."
                    )
                    null
                }
            }
        }
        return TelemetryConfig(
            apiKey = readOptionalString(m, "apiKey") ?: "",
            appId = readOptionalString(m, "appId") ?: "",
            os = TelemetryOs.ANDROID,
            // The API level, which is what the earlier client reported too.
            osMajor = Build.VERSION.SDK_INT.toUShort(),
            appVersion = readOptionalString(m, "appVersion"),
            debug = readOptionalBoolean(m, "debug"),
            flushIntervalMs = readOptionalNonNegativeLong(m, "flushIntervalMs")?.toULong(),
            maxBatchBytes = readOptionalNonNegativeLong(m, "maxBatchBytes")?.toULong(),
            // Clamped, not truncated: `toUInt()` on a Long wraps, so a
            // JavaScript number just past `UInt.MAX_VALUE` would silently
            // become a one-slot ring buffer rather than a large one. Swift
            // clamps the same field; a value this large is nonsense either
            // way, and the two bindings should be nonsense in the same
            // direction.
            maxBufferedRecords = readOptionalNonNegativeLong(m, "maxBufferedRecords")
                ?.coerceAtMost(UInt.MAX_VALUE.toLong())
                ?.toUInt(),
            includeDeviceId = readOptionalBoolean(m, "includeDeviceId"),
            scrubIds = readOptionalBoolean(m, "scrubIds"),
            mlsVerbosity = verbosity,
            metricsCadenceMs = readOptionalNonNegativeLong(m, "metricsCadenceMs")?.toULong(),
            routingDiagnostic = readOptionalBoolean(m, "routingDiagnostic"),
            mlsSamplingBypass = readOptionalBoolean(m, "mlsSamplingBypass"),
        )
    }

    private fun readOptionalBoolean(map: ReadableMap, key: String): Boolean? =
        if (map.hasKey(key) && !map.isNull(key)) map.getBoolean(key) else null

    private fun readOptionalString(map: ReadableMap, key: String): String? =
        if (map.hasKey(key) && !map.isNull(key)) map.getString(key) else null

    // Non-negative u64-compatible parse: JS numbers are f64, so values above
    // 2^53 lose precision — fine for config-sized fields (cadences in ms
    // fit comfortably). Negative values would wrap to a huge u64 after
    // `.toULong()`, so we explicitly warn-and-reject (returning null keeps
    // the Rust-side default). Do NOT reuse this helper for counter fields
    // that can exceed 2^53.
    private fun readOptionalNonNegativeLong(map: ReadableMap, key: String): Long? {
        if (!map.hasKey(key) || map.isNull(key)) {
            return null
        }
        val value = map.getDouble(key).toLong()
        if (value < 0L) {
            Log.w(
                NAME,
                "telemetry: ignoring negative value for '$key' ($value) — expected a non-negative number. Falling back to the Rust default."
            )
            return null
        }
        return value
    }

    // Counters go through putDouble: they are u64 on the Rust side and a
    // signed Int would wrap at 2^31 on a long-running device.
    private fun encodeTelemetryStats(s: TelemetryStats): WritableMap = Arguments.createMap().apply {
        putDouble("buffered", s.buffered.toLong().toDouble())
        putDouble("sentEvents", s.sentEvents.toLong().toDouble())
        putDouble("acceptedEvents", s.acceptedEvents.toLong().toDouble())
        putDouble("dropped", s.dropped.toLong().toDouble())
        putString("sessionId", s.sessionId)
        s.lastError?.let { putString("lastError", it) }
        s.lastFlushAtMs?.let { putDouble("lastFlushAtMs", it.toDouble()) }
    }

    @ReactMethod
    fun stop(promise: Promise) {
        try {
            stopTransportsAndProtocol()
            promise.resolve(null)
        } catch (e: Exception) {
            emitDiagnostic("error", "Failed to stop protocol", mapOf(
                "message" to (e.message ?: "unknown"),
                "exception" to e.javaClass.simpleName
            ))
            promise.reject("ERROR_STOP", "Failed to stop protocol: ${e.message}", e)
        }
    }

    /**
     * Stop every transport, the process scheduler, the keep-alive service and
     * the protocol core. Shared by the JS-facing [stop] and the mesh
     * notification's Stop action so both tear down identically: the foreground
     * service is only a keep-alive, and dropping it on its own would leave the
     * transports and the scheduler running with no process protection.
     *
     * Synchronized because those two callers run on different threads and can
     * overlap — a notification Stop while the app foregrounds and calls `stop()`.
     * Interleaved passes double-stop the transports mid-teardown. Serialized,
     * the second entrant re-runs the stops after a completed first pass, which
     * is their idempotent no-op path.
     *
     * Every step runs through [TeardownSequence], so a transport that throws
     * cannot skip the ones after it, strand the notification advertising an
     * active mesh, or leave the core ticking its outbox against stopped
     * transports. A BLE stop reaches `stopScan` on an adapter the user may have
     * just switched off, which throws `IllegalStateException` back across
     * `runOnBleThreadSync` — and BLE is the first transport in the sequence. The
     * first failure is rethrown once the rest are down, so [stop] still rejects
     * with the same cause it did before.
     */
    @Synchronized
    private fun stopTransportsAndProtocol() {
        engineHost.stopTransportsAndProtocol { stopForegroundService() }
    }

    /**
     * The user tapped "Stop" on the mesh notification. Runs the same teardown
     * as the JS-facing [stop], then tells JS — without the event, the app would
     * keep reporting mesh as active against transports that are now down.
     *
     * The service invokes this from its main-thread `onStartCommand`, so the
     * work moves off that thread: [stopProcessScheduler] blocks for up to
     * [PROCESS_SHUTDOWN_TIMEOUT_MS] waiting for an in-flight process tick, and
     * it waits again for any overlapping teardown to release the lock.
     *
     * The event fires in `finally` so a throwing transport cannot leave JS
     * believing the mesh is still up; [stopTransportsAndProtocol] already
     * guarantees the keep-alive came down on that path.
     */
    private fun handleUserRequestedMeshStop() {
        Thread({
            try {
                stopTransportsAndProtocol()
            } catch (e: Exception) {
                android.util.Log.e(NAME, "User-requested mesh stop failed", e)
                emitDiagnostic("error", "User-requested mesh stop failed", mapOf(
                    "message" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName
                ))
            } finally {
                emitMeshStoppedByUserEvent()
            }
        }, "mesh-user-stop").start()
    }

    /**
     * Reports that the mesh was stopped from the notification's Stop action
     * rather than through a JS `stop()` call, so the app can reconcile its own
     * "mesh active" state with a teardown it did not initiate.
     *
     * Sticky, and the reason [sendStickyEvent] exists. This is the terminal
     * event of the mesh lifecycle — by the time it fires the transports, the
     * scheduler, the keep-alive service and the core are down, so no later
     * event carries the same news. The window where it matters most is the one
     * where JS is least likely to be listening: the user reaches the
     * notification shade precisely when the app is not in front of them.
     */
    private fun emitMeshStoppedByUserEvent() {
        try {
            val json = JSONObject()
            json.put("type", EVENT_MESH_STOPPED_BY_USER)
            sendStickyEvent(EVENT_MESH_STOPPED_BY_USER, json.toString())
        } catch (e: Exception) {
            android.util.Log.e(NAME, "Failed to emit mesh stopped event", e)
        }
    }

    /**
     * Pauses the transports and then the core, in that order.
     *
     * The order is the contract, not a preference: pausing the transports first
     * is what bounds the window in which one of them can still re-enter UniFFI
     * behind a paused core, and it is why [InternetManager.pause] can describe
     * its final drain as "bounded to sends already in flight".
     *
     * Which is exactly why the fan-out runs through [TeardownSequence], like the
     * two stop paths already do. Each of these is now a synchronous wait on that
     * transport's own confinement thread, so each is a place a real exception
     * can surface; written as a plain statement list the first throw skips the
     * four transports after it *and* the core pause, leaving the core running
     * against a half-paused transport set — the opposite of what the ordering is
     * for. Failures are collected, everything still gets paused, and the first
     * one is rethrown so this method's promise rejects with the same cause it
     * would have before.
     */
    @ReactMethod
    fun pause(promise: Promise) {
        try {
            // Stop the process scheduler so process() doesn't tick while paused
            engineHost.backgroundProcessor.stopProcessScheduler()

            // Pause all transports consistently
            val teardown = TeardownSequence()
            teardown.step("BLE manager") { bleTransport?.pause() }
            teardown.step("Internet manager") { internetManager?.pause() }
            teardown.step("WiFi Direct manager") { wifiDirectManager?.pause() }
            teardown.step("Reticulum manager") { reticulumManager?.pause() }
            teardown.step("Nostr manager") { nostrManager?.pause() }
            teardown.step("protocol") { protocol?.pause() }
            teardown.firstFailureOrNull()?.let { throw it }

            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_PAUSE", "Failed to pause protocol: ${e.message}", e)
        }
    }

    /**
     * The mirror of [pause], core first — a transport must not hand work to a
     * still-paused core — and through [TeardownSequence] for the same reason.
     *
     * A resume that gives up halfway is the worse half of the pair: [pause] at
     * least fails safe (things stay stopped), whereas a transport left paused
     * here is one DORS can still select and nothing will restart until the next
     * background/foreground cycle. The scheduler restart is the step that most
     * needs to survive a transport throwing, since without it the core stops
     * ticking its outbox entirely.
     */
    @ReactMethod
    fun resume(promise: Promise) {
        try {
            val teardown = TeardownSequence()
            teardown.step("protocol") { protocol?.resume() }

            // Resume all transports consistently
            teardown.step("BLE manager") { bleTransport?.resume() }
            teardown.step("Internet manager") { internetManager?.resume() }
            teardown.step("WiFi Direct manager") { wifiDirectManager?.resume() }
            teardown.step("Reticulum manager") { reticulumManager?.resume() }
            teardown.step("Nostr manager") { nostrManager?.resume() }

            // Restart the process scheduler
            teardown.step("process scheduler") {
                if (protocol?.getState() == ProtocolState.RUNNING) {
                    engineHost.backgroundProcessor.startProcessScheduler()
                }
            }
            teardown.firstFailureOrNull()?.let { throw it }

            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_RESUME", "Failed to resume protocol: ${e.message}", e)
        }
    }

    @ReactMethod
    fun sendMessage(recipient: String, content: String, priority: Int, replyToMsg: String?, promise: Promise) {
        try {
            val messageId = engineHost.sendMessage(recipient, content, priority, replyToMsg)
            promise.resolve(messageId)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_SEND", "Failed to send message")
        }
    }

    /**
     * Rich send surface: reply context, rich media metadata, and forward
     * attribution — sealed inside the MLS ciphertext for capable recipients,
     * silently dropped for everyone else (never cleartext).
     */
    @ReactMethod
    fun sendMessageRich(recipient: String, content: String, priority: Int, replyToMsg: String?, options: ReadableMap?, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.sendMessageRich(this, recipient, content, priority, replyToMsg, options, promise)
    }

    private fun parseReplyContext(map: ReadableMap?): ReplyContext? {
        if (map == null) return null
        return ReplyContext(
            sender = map.getString("sender") ?: "",
            text = map.getString("text") ?: "",
            timestamp = if (map.hasKey("timestamp") && !map.isNull("timestamp")) map.getDouble("timestamp").toLong() else null,
            replyMediaLabel = map.getString("reply_media_label"),
            replyContentType = map.getString("reply_content_type")
        )
    }

    private fun parseForwardInfo(map: ReadableMap?): ForwardInfo? {
        if (map == null) return null
        return ForwardInfo(
            originalSender = map.getString("original_sender") ?: "",
            originalMessageId = map.getString("original_message_id") ?: "",
            originalTimestamp = if (map.hasKey("original_timestamp") && !map.isNull("original_timestamp")) map.getDouble("original_timestamp").toLong() else 0L,
            forwardCount = if (map.hasKey("forward_count") && !map.isNull("forward_count")) map.getInt("forward_count").toUInt() else 1u
        )
    }

    /**
     * Full MediaMetadata parser for the rich send surface — unlike the
     * legacy sendMedia mapping, this includes the cloud/sticker fields
     * (they only ever travel MLS-sealed on this path).
     */
    private fun parseRichMediaMetadata(map: ReadableMap?): MediaMetadata? {
        if (map == null) return null
        return MediaMetadata(
            mimeType = map.getString("mime_type") ?: "",
            fileName = map.getString("file_name") ?: "",
            fileSize = if (map.hasKey("file_size") && !map.isNull("file_size")) map.getDouble("file_size").toULong() else 0u,
            durationMs = if (map.hasKey("duration_ms") && !map.isNull("duration_ms")) map.getDouble("duration_ms").toULong() else null,
            width = if (map.hasKey("width") && !map.isNull("width")) map.getInt("width").toUInt() else null,
            height = if (map.hasKey("height") && !map.isNull("height")) map.getInt("height").toUInt() else null,
            thumbnailBase64 = map.getString("thumbnail_base64"),
            mediaId = map.getString("media_id"),
            downloadUrl = map.getString("download_url"),
            thumbnailUrl = map.getString("thumbnail_url"),
            encryptionKey = map.getString("encryption_key"),
            iv = map.getString("iv"),
            ciphertextHash = map.getString("ciphertext_hash"),
            stickerProvider = map.getString("sticker_provider"),
            stickerRemoteId = map.getString("sticker_remote_id"),
            stickerKind = map.getString("sticker_kind")
        )
    }

    @ReactMethod
    fun forwardMessage(originalMessageJson: String, newRecipient: String, priority: Int, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.forwardMessage(this, originalMessageJson, newRecipient, priority, promise)
    }

    @ReactMethod
    fun sendConnectionRequest(recipient: String, senderName: String, keyPackage: ReadableArray?, initialMessage: String?, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.sendConnectionRequest(this, recipient, senderName, keyPackage, initialMessage, promise)
    }

    @ReactMethod
    fun acceptConnectionRequest(recipient: String, accepterName: String, keyPackage: ReadableArray?, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.acceptConnectionRequest(this, recipient, accepterName, keyPackage, promise)
    }

    @ReactMethod
    fun rejectConnectionRequest(recipient: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.rejectConnectionRequest(this, recipient, promise)
    }

    @ReactMethod
    fun cancelConnectionRequest(recipient: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.cancelConnectionRequest(this, recipient, promise)
    }

    // User Blocking

    @ReactMethod
    fun blockUser(userId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.blockUser(this, userId, promise)
    }

    @ReactMethod
    fun unblockUser(userId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.unblockUser(this, userId, promise)
    }

    @ReactMethod
    fun getBlockedUsers(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getBlockedUsers(this, promise)
    }

    @ReactMethod
    fun isUserBlocked(userId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.isUserBlocked(this, userId, promise)
    }

    // ─── Presence, Typing, Read Receipts ───────────────────────

    @ReactMethod
    fun sendPresenceUpdate(recipient: String, status: Int, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.sendPresenceUpdate(this, recipient, status, promise)
    }

    @ReactMethod
    fun sendTypingIndicator(recipient: String, conversationId: String, isTyping: Boolean, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.sendTypingIndicator(this, recipient, conversationId, isTyping, promise)
    }

    @ReactMethod
    fun sendReadReceipt(recipient: String, messageIds: ReadableArray, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.sendReadReceipt(this, recipient, messageIds, promise)
    }


    // ====================================================================
    // Replicated documents (via DataStore)
    // ====================================================================
    //
    // Generated shape, kept deliberately thin: every method resolves the
    // store or rejects, calls one UniFFI method, and resolves. Anything
    // smarter belongs in the core, where it is shared by all four bindings
    // instead of reimplemented in each.


    @ReactMethod
    fun dataCreateDoc(spaceId: String, docId: String, promise: Promise) {
        try {
            engineHost.dataCreateDoc(spaceId, docId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATACREATEDOC", "createDoc failed")
        }
    }

    @ReactMethod
    fun dataDeleteDoc(spaceId: String, docId: String, promise: Promise) {
        try {
            engineHost.dataDeleteDoc(spaceId, docId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATADELETEDOC", "deleteDoc failed")
        }
    }

    @ReactMethod
    fun dataRemoveDoc(spaceId: String, docId: String, promise: Promise) {
        try {
            engineHost.dataRemoveDoc(spaceId, docId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAREMOVEDOC", "removeDoc failed")
        }
    }

    @ReactMethod
    fun dataRemoveSpace(spaceId: String, promise: Promise) {
        try {
            engineHost.dataRemoveSpace(spaceId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAREMOVESPACE", "removeSpace failed")
        }
    }

    @ReactMethod
    fun dataFetchAttachmentFrom(spaceId: String, peerId: String, hash: String, promise: Promise) {
        try {
            engineHost.dataFetchAttachmentFrom(spaceId, peerId, hash)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(
                promise, e, "ERROR_DATAFETCHATTACHMENTFROM", "fetchAttachmentFrom failed"
            )
        }
    }

    @ReactMethod
    fun dataSetInterest(spaceId: String, patterns: ReadableArray, promise: Promise) {
        try {
            // An empty list is a real answer here ("send me nothing"), so it
            // is passed through rather than rejected the way an empty id
            // list is elsewhere in this bridge.
            //
            // A null entry becomes the empty string rather than being
            // dropped. Dropping it would silently hand the core a narrower
            // list than the application wrote; the empty string is not a
            // document name, so the core refuses the call and the mistake
            // is reported where it was made.
            val list = (0 until patterns.size()).map { patterns.getString(it) ?: "" }
            engineHost.dataSetInterest(spaceId, list)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATASETINTEREST", "setInterest failed")
        }
    }

    @ReactMethod
    fun dataListDocs(spaceId: String, promise: Promise) {
        try {
            promise.resolve(engineHost.dataListDocs(spaceId))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATALISTDOCS", "listDocs failed")
        }
    }

    @ReactMethod
    fun dataListSpaces(promise: Promise) {
        try {
            promise.resolve(engineHost.dataListSpaces())
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATALISTSPACES", "listSpaces failed")
        }
    }

    @ReactMethod
    fun dataMapSet(spaceId: String, docId: String, collection: String, key: String, valueJson: String, promise: Promise) {
        try {
            engineHost.dataMapSet(spaceId, docId, collection, key, valueJson)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAMAPSET", "mapSet failed")
        }
    }

    @ReactMethod
    fun dataMapDelete(spaceId: String, docId: String, collection: String, key: String, promise: Promise) {
        try {
            engineHost.dataMapDelete(spaceId, docId, collection, key)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAMAPDELETE", "mapDelete failed")
        }
    }

    @ReactMethod
    fun dataMapGetJson(spaceId: String, docId: String, collection: String, key: String, promise: Promise) {
        try {
            promise.resolve(engineHost.dataMapGetJson(spaceId, docId, collection, key))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAMAPGETJSON", "mapGet failed")
        }
    }

    @ReactMethod
    fun dataListPush(spaceId: String, docId: String, collection: String, valueJson: String, promise: Promise) {
        try {
            engineHost.dataListPush(spaceId, docId, collection, valueJson)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATALISTPUSH", "listPush failed")
        }
    }

    @ReactMethod
    fun dataListDelete(spaceId: String, docId: String, collection: String, index: Double, count: Double, promise: Promise) {
        try {
            engineHost.dataListDelete(spaceId, docId, collection, index, count)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATALISTDELETE", "listDelete failed")
        }
    }

    @ReactMethod
    fun dataListLen(spaceId: String, docId: String, collection: String, promise: Promise) {
        try {
            promise.resolve(engineHost.dataListLen(spaceId, docId, collection))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATALISTLEN", "listLength failed")
        }
    }

    @ReactMethod
    fun dataTextInsert(spaceId: String, docId: String, collection: String, position: Double, text: String, promise: Promise) {
        try {
            engineHost.dataTextInsert(spaceId, docId, collection, position, text)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATATEXTINSERT", "textInsert failed")
        }
    }

    @ReactMethod
    fun dataTextDelete(spaceId: String, docId: String, collection: String, position: Double, count: Double, promise: Promise) {
        try {
            engineHost.dataTextDelete(spaceId, docId, collection, position, count)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATATEXTDELETE", "textDelete failed")
        }
    }

    @ReactMethod
    fun dataTextValue(spaceId: String, docId: String, collection: String, promise: Promise) {
        try {
            promise.resolve(engineHost.dataTextValue(spaceId, docId, collection))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATATEXTVALUE", "textValue failed")
        }
    }

    @ReactMethod
    fun dataCounterIncrement(spaceId: String, docId: String, collection: String, amount: Double, promise: Promise) {
        try {
            engineHost.dataCounterIncrement(spaceId, docId, collection, amount)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATACOUNTERINCREMENT", "counterIncrement failed")
        }
    }

    @ReactMethod
    fun dataCounterValue(spaceId: String, docId: String, collection: String, promise: Promise) {
        try {
            promise.resolve(engineHost.dataCounterValue(spaceId, docId, collection))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATACOUNTERVALUE", "counterValue failed")
        }
    }

    @ReactMethod
    fun dataDocJson(spaceId: String, docId: String, promise: Promise) {
        try {
            promise.resolve(engineHost.dataDocJson(spaceId, docId))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATADOCJSON", "docJson failed")
        }
    }

    @ReactMethod
    fun dataExportRaw(spaceId: String, docId: String, promise: Promise) {
        try {
            promise.resolve(
                android.util.Base64.encodeToString(
                    engineHost.dataExportRaw(spaceId, docId),
                    android.util.Base64.NO_WRAP,
                ),
            )
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAEXPORTRAW", "exportRaw failed")
        }
    }

    @ReactMethod
    fun dataFlush(spaceId: String, docId: String, promise: Promise) {
        try {
            engineHost.dataFlush(spaceId, docId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAFLUSH", "flush failed")
        }
    }

    @ReactMethod
    fun dataFlushAll(promise: Promise) {
        try {
            engineHost.dataFlushAll()
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAFLUSHALL", "flushAll failed")
        }
    }

    @ReactMethod
    fun dataDocSize(spaceId: String, docId: String, promise: Promise) {
        try {
            promise.resolve(engineHost.dataDocSize(spaceId, docId))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATADOCSIZE", "docSize failed")
        }
    }

    // ---- attachments ---------------------------------------------------
    //
    // Blob bytes never enter a document and never enter protocol state, so
    // the app owns them. `dataAttachmentHash` gives the address to write into
    // a document; the other three are the fetch conversation.

    @ReactMethod
    fun dataAttachmentHash(bytesBase64: String, promise: Promise) {
        try {
            promise.resolve(engineHost.dataAttachmentHash(bytesBase64))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAATTACHMENTHASH", "attachmentHash failed")
        }
    }

    @ReactMethod
    fun dataFetchAttachment(spaceId: String, hash: String, promise: Promise) {
        try {
            engineHost.dataFetchAttachment(spaceId, hash)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAFETCHATTACHMENT", "fetchAttachment failed")
        }
    }

    @ReactMethod
    fun dataProvideAttachment(spaceId: String, peerId: String, hash: String, bytesBase64: String, promise: Promise) {
        try {
            engineHost.dataProvideAttachment(spaceId, peerId, hash, bytesBase64)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAPROVIDEATTACHMENT", "provideAttachment failed")
        }
    }

    @ReactMethod
    fun dataDeclineAttachment(spaceId: String, peerId: String, hash: String, promise: Promise) {
        try {
            engineHost.dataDeclineAttachment(spaceId, peerId, hash)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATADECLINEATTACHMENT", "declineAttachment failed")
        }
    }

    @ReactMethod
    fun dataWipeAll(promise: Promise) {
        try {
            engineHost.dataWipeAll()
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_DATAWIPEALL", "wipeAll failed")
        }
    }

    // Service Discovery & Request/Response (via MeshServices)

    @ReactMethod
    fun registerService(serviceId: String, version: String, capabilitiesJson: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.registerService(this, serviceId, version, capabilitiesJson, promise)
    }

    @ReactMethod
    fun unregisterService(serviceId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.unregisterService(this, serviceId, promise)
    }

    @ReactMethod
    fun discoverServices(serviceId: String?, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.discoverServices(this, serviceId, promise)
    }

    @ReactMethod
    fun sendServiceRequest(provider: String, serviceId: String, method: String, body: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.sendServiceRequest(this, provider, serviceId, method, body, promise)
    }

    @ReactMethod
    fun respondToServiceRequest(requestId: String, requester: String, serviceId: String, status: String, body: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.respondToServiceRequest(this, requestId, requester, serviceId, status, body, promise)
    }

    @ReactMethod
    fun receiveMessage(promise: Promise) {
        val messageJson = protocol?.receiveMessage()
        promise.resolve(messageJson)
    }

    /**
     * Tear the protocol down and release it, leaving the module able to
     * [create] a fresh one.
     *
     * Runs the same teardown as [stop] rather than its own copy, which is also
     * what brings the keep-alive service down: a `destroy` without a preceding
     * `stop` used to leave a "Mesh Active" notification over a protocol that no
     * longer existed. Sharing the path serializes it against a notification
     * Stop running concurrently, too.
     *
     * A step that throws is logged, not propagated: the sequence has already
     * run to completion by then, and `destroy` releasing its references is not
     * conditional on every transport having stopped cleanly — a caller that
     * could not destroy would have no way forward.
     */
    @ReactMethod
    fun destroy(promise: Promise) {
        try {
            try {
                stopTransportsAndProtocol()
            } catch (e: Exception) {
                android.util.Log.w(NAME, "Teardown failed during destroy; releasing anyway", e)
            }

            releaseForegroundStopCallback()
            engineHost.releaseHandlesAfterStop()
            // [listenerCount] is deliberately *not* reset here. React Native
            // owns it through addListener/removeListeners, and this module is a
            // singleton per React instance while `OfflineProtocol` is not: an
            // app that constructs a second one before destroying the first —
            // the logout-and-re-create sequence that shuts the gate in the
            // first place — leaves live subscriptions behind that this call
            // knows nothing about. Zeroing the count then reports no listeners
            // while JS is fully subscribed, and since removeListeners only ever
            // subtracts (coerced at zero), nothing resyncs it until the
            // survivor resubscribes. Through the JS SDK the reset was a no-op
            // anyway — destroy() removes both subscriptions before calling
            // here, so the count is already zero, which is exactly why the line
            // survived. Leave the counter to the framework.
            //
            // The app tore the SDK down itself, so it is not waiting to be told
            // about a teardown it did not initiate. Ends the session rather
            // than just emptying the buffer: a stop emitted moments earlier can
            // still be between its failed emit and its hold, and a flush can
            // still be carrying drained entries — both are refused by
            // generation once this has run.
            //
            // And the buffer stops holding entirely until the next start().
            // The generation gate only catches a teardown landing inside the
            // instant between an emit's generation read and its hold; a
            // notification Stop that began before this destroy() emits *after*
            // it — the two share stopTransportsAndProtocol()'s lock, so this
            // running to completion first is the ordinary interleaving, not the
            // exotic one — and would otherwise be held under the new generation
            // for whichever session subscribes next.
            stickyEvents.endSession()
            inboundEvents.endSession()
            currentConfig = null
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_DESTROY", "Failed to destroy protocol: ${e.message}", e)
        }
    }

    /**
     * Erases every byte of persisted SDK state for one account.
     *
     * Takes the identity explicitly rather than reading [currentConfig], because
     * the call this exists for happens *after* [destroy], which clears it. That
     * also lets an application wipe the account it just signed out of while a
     * different one is already running.
     *
     * Refuses to wipe the account this instance is currently running: the
     * protocol persists as it works — outbox entries on the send path, pending
     * snapshots, sealed state records — so a wipe underneath a live instance
     * races those writes and leaves a partially repopulated container. Call
     * `destroy()` first.
     */
    @ReactMethod
    fun wipePersistedState(appId: String, profile: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.wipePersistedState(this, appId, profile, promise)
    }

    // ========================================================================
    // TRANSPORT MANAGEMENT
    // ========================================================================

    @ReactMethod
    fun enableTransport(type: String, config: ReadableMap?, promise: Promise) {
        try {
            engineHost.enableTransport(
                reactApplicationContext,
                type,
                BridgeArgumentMaps.readableMapToJsonObject(config),
                internetTransportEventHandlers(),
            ) {
                stickyEvents.discard(EVENT_INTERNET_SESSION_SUPERSEDED)
            }
            promise.resolve(null)
        } catch (e: ProtocolException) {
            promise.reject("ERROR_TRANSPORT_ENABLE", "Failed to enable transport: ${e.message}", e)
        } catch (e: Exception) {
            promise.reject("ERROR_TRANSPORT_ENABLE", "Failed to enable transport: ${e.message}", e)
        }
    }

    private fun internetTransportEventHandlers(): ProtocolEngineHost.InternetTransportEventHandlers =
        ProtocolEngineHost.InternetTransportEventHandlers(
            serverMessageEmitter = { rawJson -> emitServerMessageEvent(rawJson) },
            connectionStatusEmitter = { connected, authenticated ->
                emitInternetStatusEvent(connected, authenticated)
            },
            supersededEmitter = { reason -> emitInternetSupersededEvent(reason) },
        )

    /**
     * Delegates to [InternetTransportBootstrap]; kept so legacy call sites and
     * docs that name this helper stay valid. All enable paths use the bootstrap.
     */
    private fun configureAndStartInternet(manager: InternetManager, config: ReadableMap?) {
        val json = BridgeArgumentMaps.readableMapToJsonObject(config) ?: org.json.JSONObject()
        InternetTransportBootstrap.configureAndStart(manager, json)
        emitDiagnostic("info", "Internet transport enabled", mapOf(
            "serverUrl" to json.optString("serverAddress", json.optString("server_url", "")),
            "autoReconnect" to json.optBooleanCompat("autoReconnect", "auto_reconnect"),
            "hasAuthToken" to json.has("authToken"),
        ))
    }

    @ReactMethod
    fun disableTransport(type: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.disableTransport(this, type, promise)
    }
    
    
    @ReactMethod
    fun isBluetoothEnabled(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.isBluetoothEnabled(this, promise)
    }

    @ReactMethod
    fun requestEnableBluetooth(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.requestEnableBluetooth(this, promise)
    }

    @ReactMethod
    fun getTopology(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getTopology(this, promise)
    }

    @ReactMethod
    fun getMessageStats(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getMessageStats(this, promise)
    }

    @ReactMethod
    fun getDeliverySuccessRate(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getDeliverySuccessRate(this, promise)
    }

    @ReactMethod
    fun getMedianLatency(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getMedianLatency(this, promise)
    }

    @ReactMethod
    fun getMedianHops(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getMedianHops(this, promise)
    }

    @ReactMethod
    fun sendFile(recipient: String, fileData: String, fileName: String, promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val bytes = android.util.Base64.decode(fileData, android.util.Base64.DEFAULT)
            val id = proto.sendFile(recipient, bytes.map { it.toUByte() }, fileName)
            promise.resolve(id)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_SEND_FILE", "Failed to send file")
        }
    }

    // The path twin of sendFile: hands the Rust core a filesystem path so the
    // payload never crosses the RN bridge. sendFile stages a 25 MB file as a
    // ~34 MB base64 JS string, a decoded ByteArray, and a List<UByte> with one
    // boxed object per byte before Rust sees it; that is the out-of-memory
    // failure on constrained devices. Here Kotlin holds only the path, and
    // Rust reads and chunks the file on its own heap.
    @ReactMethod
    fun sendFileFromPath(recipient: String, filePath: String, fileName: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.sendFileFromPath(this, recipient, filePath, fileName, promise)
    }

    @ReactMethod
    fun sendMedia(recipient: String, fileData: String, fileName: String, contentType: String, mediaMetadata: ReadableMap?, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.sendMedia(this, recipient, fileData, fileName, contentType, mediaMetadata, promise)
    }

    @ReactMethod
    fun sendMediaRich(recipient: String, fileData: String, fileName: String, contentType: String, options: ReadableMap?, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.sendMediaRich(this, recipient, fileData, fileName, contentType, options, promise)
    }

    private fun parseContentType(value: String): ContentType {
        return when (value.lowercase()) {
            "text" -> ContentType.TEXT
            "image" -> ContentType.IMAGE
            "video" -> ContentType.VIDEO
            "audio" -> ContentType.AUDIO
            "voice_note" -> ContentType.VOICE_NOTE
            "video_note" -> ContentType.VIDEO_NOTE
            "file" -> ContentType.FILE
            "file_chunk" -> ContentType.FILE_CHUNK
            "poll" -> ContentType.POLL
            else -> ContentType.FILE
        }
    }

    @ReactMethod
    fun getFileProgress(fileId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getFileProgress(this, fileId, promise)
    }

    @ReactMethod
    fun cancelFileTransfer(fileId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.cancelFileTransfer(this, fileId, promise)
    }
    
    // ========================================================================
    // BLE TRANSPORT METHODS
    // ========================================================================
    
    @ReactMethod
    fun blePeerDiscovered(peerId: String, rssi: Int, promise: Promise) {
        try {
            protocol?.blePeerDiscovered(peerId, rssi.toShort())
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_BLE", "BLE peer discovered failed: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun blePeerLost(peerId: String, promise: Promise) {
        try {
            protocol?.blePeerLost(peerId)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_BLE", "BLE peer lost failed: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun bleStatusChanged(isAvailable: Boolean, promise: Promise) {
        try {
            protocol?.bleStatusChanged(isAvailable)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_BLE", "BLE status changed failed: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun bleFragmentReceived(senderId: String, fragmentData: ReadableArray, promise: Promise) {
        try {
            val fragment = mutableListOf<UByte>()
            for (i in 0 until fragmentData.size()) {
                fragment.add(fragmentData.getInt(i).toUByte())
            }
            protocol?.bleFragmentReceived(senderId, fragment)
            promise.resolve(null)
        } catch (e: Exception) {
            promise.reject("ERROR_BLE", "BLE fragment received failed: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun bleGetNextFragment(promise: Promise) {
        try {
            val fragment = protocol?.bleGetNextFragment()
            if (fragment != null) {
                val map = Arguments.createMap()
                map.putString("recipientId", fragment.recipientId)
                val array = Arguments.createArray()
                fragment.data.forEach { array.pushInt(it.toInt()) }
                map.putArray("data", array)
                promise.resolve(map)
            } else {
                promise.resolve(null)
            }
        } catch (e: Exception) {
            promise.reject("ERROR_BLE", "BLE get next fragment failed: ${e.message}", e)
        }
    }
    
    @ReactMethod
    fun bleReturnFragment(promise: Promise) {
        protocol?.bleReturnFragment()
        promise.resolve(null)
    }
    
    @ReactMethod
    fun bleGetPeerCount(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.bleGetPeerCount(this, promise)
    }

    /**
     * BLE diagnostic counters — the address-migration rollout alarm.
     *
     * Exposed as one call because they are one signal: each counts a frame
     * that went out under a degraded path rather than failing, so they are
     * invisible in delivery metrics and only meaningful read together.
     * Sustained growth after a release means peers disagree about identity.
     * All three read zero when BLE is not registered.
     */
    @ReactMethod
    fun bleGetDiagnostics(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.bleGetDiagnostics(this, promise)
    }
    
    @ReactMethod
    fun getActiveTransports(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getActiveTransports(this, promise)
    }
    
    @ReactMethod
    fun getState(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getState(this, promise)
    }
    
    // MARK: - Battery Management
    
    @ReactMethod
    fun setBatteryLevel(level: Int, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.setBatteryLevel(this, level, promise)
    }

    @ReactMethod
    fun setBatteryState(level: Int, isCharging: Boolean, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.setBatteryState(this, level, isCharging, promise)
    }

    @ReactMethod
    fun getBatteryLevel(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getBatteryLevel(this, promise)
    }

    @ReactMethod
    fun getIsCharging(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getIsCharging(this, promise)
    }

    // MARK: - Relay Management

    @ReactMethod
    fun setRelayPriority(priorityString: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.setRelayPriority(this, priorityString, promise)
    }

    @ReactMethod
    fun getRelayPriority(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getRelayPriority(this, promise)
    }

    /**
     * Applies a partial relay configuration: fields absent from [configJson]
     * keep their current values, so an app can raise its battery floor without
     * restating the rest.
     */
    @ReactMethod
    fun updateRelayConfig(configJson: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.updateRelayConfig(this, configJson, promise)
    }

    @ReactMethod
    fun getRelayConfig(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getRelayConfig(this, promise)
    }

    @ReactMethod
    fun isRelay(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.isRelay(this, promise)
    }
    
    // MARK: - Transport Metrics
    
    @ReactMethod
    fun getTransportMetrics(transportType: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getTransportMetrics(this, transportType, promise)
    }
    
    // MARK: - Manual Transport Control
    
    @ReactMethod
    fun forceTransport(transportType: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.forceTransport(this, transportType, promise)
    }
    
    @ReactMethod
    fun releaseTransportLock(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.releaseTransportLock(this, promise)
    }
    
    // MARK: - DORS Configuration
    
    /**
     * Applies a partial DORS configuration: fields absent from [configJson]
     * keep their **current** values, read back from the live engine rather
     * than restated as literals here.
     *
     * Defaulting to literals is what made `updateDorsConfig` a silent reset —
     * an update meaning to change one field rewrote every other one to a
     * hardcoded number, and the three battery/relay fields could not even be
     * restated because they had no path across the bridge. Sourcing the
     * fallbacks from `getDorsConfig()` fixes both halves at once and keeps
     * this method's contract identical to [updateRelayConfig]'s.
     */
    @ReactMethod
    fun updateDorsConfig(configJson: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.updateDorsConfig(this, configJson, promise)
    }
    
    @ReactMethod
    fun getDorsConfig(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getDorsConfig(this, promise)
    }
    
    // MARK: - Reliability Configuration
    
    @ReactMethod
    fun updateAckConfig(configJson: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.updateAckConfig(this, configJson, promise)
    }
    
    @ReactMethod
    fun updateRetryConfig(configJson: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.updateRetryConfig(this, configJson, promise)
    }
    
    @ReactMethod
    fun updateDedupConfig(configJson: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.updateDedupConfig(this, configJson, promise)
    }
    
    @ReactMethod
    fun getDedupStats(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getDedupStats(this, promise)
    }
    
    /**
     * Cumulative mesh forwarding counters, read through to the Rust core.
     *
     * Counters are u64 in the core and cross to JS as doubles, which is exact
     * below 2^53 — a device would have to forward frames for millions of years
     * to reach that.
     */
    @ReactMethod
    fun getMeshRelayStats(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getMeshRelayStats(this, promise)
    }

    /**
     * The mesh forwarding tunables in force, read from the governor.
     *
     * Every field is populated, so no caller writes a fallback literal — a
     * fallback here would be a second copy of a core default, free to drift.
     */
    @ReactMethod
    fun getMeshRelayTunables(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getMeshRelayTunables(this, promise)
    }

    // Custody counters, read through to the Rust core (docs/spec/custody.md).
    @ReactMethod
    fun getCustodyStats(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getCustodyStats(this, promise)
    }

    // Drops every held frame and resets the custody counters. The data
    // layer's wipe calls the same erase in the core; this is the standalone
    // verb.
    @ReactMethod
    fun eraseCustody(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.eraseCustody(this, promise)
    }

    @ReactMethod
    fun getPendingAckCount(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getPendingAckCount(this, promise)
    }
    
    @ReactMethod
    fun getRetryQueueSize(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getRetryQueueSize(this, promise)
    }

    // ========================================================================
    // DORS DECISION SUPPORT
    // ========================================================================

    @ReactMethod
    fun shouldEscalateToWifi(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.shouldEscalateToWifi(this, promise)
    }

    // ========================================================================
    // FILE TRANSFER OPERATIONS
    // ========================================================================

    @ReactMethod
    fun processFileChunk(fileId: String, chunkIndex: Int, totalChunks: Int, fileSize: Double, fileName: String, fileChecksum: String, data: ReadableArray, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.processFileChunk(this, fileId, chunkIndex, totalChunks, fileSize, fileName, fileChecksum, data, promise)
    }

    @ReactMethod
    fun finalizeFile(fileId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.finalizeFile(this, fileId, promise)
    }

    // ========================================================================
    // WIFI DIRECT TRANSPORT METHODS
    // ========================================================================

    @ReactMethod
    fun wifiDirectStatusChanged(isConnected: Boolean, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.wifiDirectStatusChanged(this, isConnected, promise)
    }

    @ReactMethod
    fun wifiDirectMessageReceived(senderId: String, data: ReadableArray, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.wifiDirectMessageReceived(this, senderId, data, promise)
    }

    @ReactMethod
    fun wifiDirectGetNextMessage(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.wifiDirectGetNextMessage(this, promise)
    }

    @ReactMethod
    fun wifiDirectPeerConnected(peerId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.wifiDirectPeerConnected(this, peerId, promise)
    }

    @ReactMethod
    fun wifiDirectPeerDisconnected(peerId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.wifiDirectPeerDisconnected(this, peerId, promise)
    }

    // ========================================================================
    // INTERNET TRANSPORT METHODS
    // ========================================================================

    @ReactMethod
    fun internetStatusChanged(isConnected: Boolean, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.internetStatusChanged(this, isConnected, promise)
    }

    @ReactMethod
    fun internetMessageReceived(senderId: String, data: ReadableArray, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.internetMessageReceived(this, senderId, data, promise)
    }

    @ReactMethod
    fun internetGetNextMessage(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.internetGetNextMessage(this, promise)
    }

    @ReactMethod
    fun internetConfirmSent(messageId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.internetConfirmSent(this, messageId, promise)
    }

    @ReactMethod
    fun internetSendFailed(messageId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.internetSendFailed(this, messageId, promise)
    }

    // ========================================================================
    // MLS (END-TO-END ENCRYPTION)
    // ========================================================================

    /**
     * Initialize MLS with built-in secure storage (EncryptedSharedPreferences)
     */
    @ReactMethod
    fun initializeMlsWithSecureStorage(promise: Promise) {
        try {
            val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
            val config = currentConfig
                ?: throw IllegalStateException("Protocol config not initialized")
            val accountNamespace = StorageNamespace.account(config.appId, config.profile)
            val secureStorage =
                MlsSecureStorage(reactApplicationContext, accountNamespace)
            val protocolStateStorage =
                AppContainerProtocolStateStorage(
                    reactApplicationContext,
                    accountNamespace
                )
            proto.initializeMls(secureStorage, protocolStateStorage)
            // Either of these means this account is starting from a fresh MLS
            // identity — never let that pass silently.
            when (secureStorage.legacyAdoption) {
                is LegacyStoreAdoption.Decision.Conflict -> emitDiagnostic(
                    "error",
                    "Legacy secure store belongs to another account; this " +
                        "account starts from a fresh MLS identity and cannot " +
                        "decrypt its previous sessions. Its pre-split delivery " +
                        "state is unreachable too, so it also comes up with an " +
                        "empty outbox and an empty block list — every " +
                        "previously blocked peer is unblocked"
                )
                is LegacyStoreAdoption.Decision.ClaimUnverified -> emitDiagnostic(
                    "error",
                    "Could not record this account's claim on the legacy secure " +
                        "store, so it was not adopted: another account could " +
                        "otherwise inherit the same MLS identity. This account " +
                        "starts from a fresh identity and comes up with an empty " +
                        "outbox and an empty block list — every previously " +
                        "blocked peer is unblocked. The credential store is " +
                        "failing writes; retrying on a healthy store adopts " +
                        "normally"
                )
                else -> Unit
            }
            emitDiagnostic(
                "info",
                "MLS initialized with split secure and app-container storage"
            )
            promise.resolve(null)
        } catch (e: Exception) {
            emitDiagnostic("error", "Failed to initialize MLS", mapOf(
                "message" to (e.message ?: "unknown")
            ))
            promise.reject("ERROR_MLS", "Failed to initialize MLS: ${e.message}", e)
        }
    }

    /**
     * Check if MLS is initialized
     */
    @ReactMethod
    fun isMlsInitialized(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.isMlsInitialized(this, promise)
    }

    // ========================================================================
    // IDENTITY AND SIGNING OPERATIONS
    // ========================================================================

    /**
     * Get the identity public key (Ed25519, 32 bytes)
     */
    @ReactMethod
    fun getIdentityPublicKey(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getIdentityPublicKey(this, promise)
    }

    /**
     * This device's own address ("off1..."), or null before MLS is initialized.
     *
     * Derived from the identity key in this profile's storage, so the app does
     * not choose it; it is stable across restarts of the same profile. This is
     * the string peers must be given to reach this device.
     */
    @ReactMethod
    fun localAddress(promise: Promise) {
        val proto = protocol
        if (proto == null) {
            promise.resolve(null)
            return
        }
        promise.resolve(proto.localAddress())
    }

    /**
     * Derive the canonical self-certifying address ("off1...") from an Ed25519
     * public key. Needs no protocol instance, so an invite or QR code can be
     * checked before create().
     */
    @ReactMethod
    fun deriveAddress(publicKey: ReadableArray, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.deriveAddress(this, publicKey, promise)
    }

    /**
     * Decode and verify an invite blob. Needs no protocol instance, so a
     * scanner can check a QR code before create().
     *
     * Verification is total: a rejected blob rejects the promise rather than
     * resolving with a warning flag, because every failure mode here means the
     * invite must not be acted on.
     */
    @ReactMethod
    fun parseInvite(blob: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.parseInvite(this, blob, promise)
    }

    /**
     * Build an invite blob for this identity.
     *
     * Sign it when the invite may travel without its issuer; leave it unsigned
     * for a QR shown phone to phone. See the JS createInvite doc.
     */
    @ReactMethod
    fun createInvite(petname: String?, signed: Boolean, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.createInvite(this, petname, signed, promise)
    }

    /**
     * Resolve a username to the set of devices claiming it.
     *
     * The answer arrives as one username_resolved event carrying every
     * verified claim. Never auto-select from it: see the JS resolveUsername
     * doc for why picking the first entry defeats the design.
     */
    @ReactMethod
    fun resolveUsername(username: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.resolveUsername(this, username, promise)
    }

    /**
     * Derive a user ID from a public key.
     *
     * Deprecated: use deriveAddress, which needs no protocol instance and
     * rejects keys that are not 32 bytes. Returns the same "off1..." address.
     */
    @ReactMethod
    fun deriveUserIdFromPublicKey(publicKey: ReadableArray, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.deriveUserIdFromPublicKey(this, publicKey, promise)
    }

    /**
     * Sign data with the identity private key
     */
    @ReactMethod
    fun signData(data: ReadableArray, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.signData(this, data, promise)
    }

    /**
     * Byte-oriented variant of `signData`: the input crosses the bridge as one
     * base64 string and the signature comes back as one. The decoded
     * ByteArray goes to `signDataBytes` (UDL `bytes`) as is. Mapping it to
     * the List<UByte> that `signData` takes would allocate one boxed object
     * per byte, twice (the list, then UniFFI's allocationSize pass), which
     * is the Android heap exhaustion this method exists to avoid.
     */
    @ReactMethod
    fun signDataBase64(dataBase64: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.signDataBase64(this, dataBase64, promise)
    }

    /**
     * Verify a signature against a public key
     */
    @ReactMethod
    fun verifySignature(publicKey: ReadableArray, data: ReadableArray, signature: ReadableArray, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.verifySignature(this, publicKey, data, signature, promise)
    }

    /**
     * Byte-oriented variant of `verifySignature`, base64 across the bridge
     * for the same reason as `signDataBase64`.
     */
    @ReactMethod
    fun verifySignatureBase64(publicKeyBase64: String, dataBase64: String, signatureBase64: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.verifySignatureBase64(this, publicKeyBase64, dataBase64, signatureBase64, promise)
    }

    /**
     * Generate a new MLS key package
     */
    @ReactMethod
    fun mlsGenerateKeyPackage(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsGenerateKeyPackage(this, promise)
    }

    /**
     * Get existing or generate new key package
     */
    @ReactMethod
    fun mlsGetOrCreateKeyPackage(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsGetOrCreateKeyPackage(this, promise)
    }

    /**
     * Get pending key packages to upload
     */
    @ReactMethod
    fun mlsGetPendingKeyPackages(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsGetPendingKeyPackages(this, promise)
    }

    /**
     * Mark key package as synced (uploaded)
     */
    @ReactMethod
    fun mlsMarkKeyPackageSynced(packageId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsMarkKeyPackageSynced(this, packageId, promise)
    }

    /**
     * Create a 1:1 session (returns Welcome message)
     */
    @ReactMethod
    fun mlsCreateSession(otherUserId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsCreateSession(this, otherUserId, promise)
    }

    /**
     * Join a session from Welcome message
     */
    @ReactMethod
    fun mlsJoinSession(welcomeJson: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsJoinSession(this, welcomeJson, promise)
    }

    /**
     * Decrypt a message from a user
     */
    @ReactMethod
    fun mlsDecryptFromUser(encryptedJson: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsDecryptFromUser(this, encryptedJson, promise)
    }

    /**
     * Delete a session
     */
    @ReactMethod
    fun mlsDeleteSession(otherUserId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsDeleteSession(this, otherUserId, promise)
    }

    /**
     * Import a contact's key package
     */
    @ReactMethod
    fun mlsImportKeyPackage(userId: String, keyPackageData: ReadableArray, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsImportKeyPackage(this, userId, keyPackageData, promise)
    }

    /**
     * Check if a session exists with a user
     */
    @ReactMethod
    fun mlsHasSession(otherUserId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsHasSession(this, otherUserId, promise)
    }

    /**
     * Check if a pending key package is available for a peer
     */
    @ReactMethod
    fun hasPendingKeyPackage(peerId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.hasPendingKeyPackage(this, peerId, promise)
    }

    /**
     * Returns the current session establishment state for a peer.
     */
    @ReactMethod
    fun getEstablishmentState(peerId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.getEstablishmentState(this, peerId, promise)
    }

    /**
     * Establish a secure session with a peer (high-level API)
     */
    @ReactMethod
    fun establishSecureSession(peerId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.establishSecureSession(this, peerId, promise)
    }

    /**
     * Rotate the 1:1 session with a peer, advancing post-compromise security.
     *
     * Resolves true when the rotation was driven and false when the per-peer
     * rate-limit window has not lapsed, which is not a failure: a caller on a
     * fixed schedule that briefly runs faster than the floor is behaving
     * correctly and a later call succeeds.
     *
     * A rejection changes nothing: the reset is advertised before the local
     * session is torn down, so the session is left intact and the window
     * unspent. Rotate while the peer is reachable and retry later.
     */
    @ReactMethod
    fun rekeySession(peerId: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.rekeySession(this, peerId, promise)
    }

    /**
     * Encrypt a message for a user
     */
    @ReactMethod
    fun mlsEncryptForUser(otherUserId: String, plaintext: ReadableArray, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsEncryptForUser(this, otherUserId, plaintext, promise)
    }

    /**
     * Decrypt a message
     */
    @ReactMethod
    fun mlsDecrypt(encryptedJson: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsDecrypt(this, encryptedJson, promise)
    }

    /**
     * List all active sessions
     */
    @ReactMethod
    fun mlsListSessions(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsListSessions(this, promise)
    }

    /**
     * Process a Welcome message
     */
    @ReactMethod
    fun mlsProcessWelcome(welcomeJson: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.mlsProcessWelcome(this, welcomeJson, promise)
    }

    // ========================================================================
    // GROUP MANAGEMENT (MESH / MLS PROTOCOL-LEVEL)
    // ========================================================================

    private fun meshGroupInfoWritableMap(info: Map<String, Any?>): WritableMap =
        Arguments.createMap().apply {
            putString("groupId", info["groupId"] as? String)
            putString("name", info["name"] as? String)
            val members = Arguments.createArray()
            @Suppress("UNCHECKED_CAST")
            (info["members"] as? List<String>)?.forEach { members.pushString(it) }
            putArray("members", members)
            putDouble("epoch", (info["epoch"] as? Double) ?: 0.0)
            putBoolean("isSession", info["isSession"] as? Boolean ?: false)
            putDouble("createdAtMs", (info["createdAtMs"] as? Double) ?: 0.0)
            putDouble("lastActivityMs", (info["lastActivityMs"] as? Double) ?: 0.0)
        }

    /**
     * Create a new MLS group via the mesh transport.
     */
    @ReactMethod
    fun meshCreateGroup(name: String, promise: Promise) {
        try {
            promise.resolve(meshGroupInfoWritableMap(engineHost.meshCreateGroup(name)))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to create mesh group")
        }
    }

    /**
     * Invite a user to a mesh group (sends Welcome+Commit to peer).
     */
    @ReactMethod
    fun meshInviteToGroup(groupId: String, inviteeUserId: String, promise: Promise) {
        try {
            engineHost.meshInviteToGroup(groupId, inviteeUserId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to invite to mesh group")
        }
    }

    /**
     * Send an MLS-encrypted message to all members of a mesh group.
     */
    @ReactMethod
    fun meshSendGroupMessage(groupId: String, content: String, priority: String?, replyToMsg: String?, promise: Promise) {
        try {
            val messageIds = engineHost.meshSendGroupMessage(groupId, content, priority, replyToMsg)
            val result = Arguments.createArray()
            messageIds.forEach { result.pushString(it) }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to send mesh group message")
        }
    }

    /**
     * Forward a message to all members of a mesh group with forwarding attribution.
     */
    @ReactMethod
    fun meshForwardMessageToGroup(originalMessageJson: String, groupId: String, priority: String?, promise: Promise) {
        try {
            val messageIds = engineHost.meshForwardMessageToGroup(originalMessageJson, groupId, priority)
            val result = Arguments.createArray()
            messageIds.forEach { result.pushString(it) }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to forward message to group")
        }
    }

    /**
     * Remove a member from a mesh group with notification.
     */
    @ReactMethod
    fun meshRemoveFromGroup(groupId: String, memberId: String, promise: Promise) {
        try {
            engineHost.meshRemoveFromGroup(groupId, memberId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to remove from mesh group")
        }
    }

    /**
     * Leave a mesh group with notification.
     */
    @ReactMethod
    fun meshLeaveGroup(groupId: String, promise: Promise) {
        try {
            engineHost.meshLeaveGroup(groupId)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to leave mesh group")
        }
    }

    /**
     * List all mesh groups.
     */
    @ReactMethod
    fun meshListGroups(promise: Promise) {
        try {
            val result = Arguments.createArray()
            engineHost.meshListGroups().forEach { result.pushString(it) }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to list mesh groups")
        }
    }

    /**
     * Get group information via high-level API.
     */
    @ReactMethod
    fun meshGetGroupInfo(groupId: String, promise: Promise) {
        try {
            val info = engineHost.meshGetGroupInfo(groupId)
            promise.resolve(info?.let { meshGroupInfoWritableMap(it) })
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to get group info")
        }
    }

    /**
     * Whether a rich group send right now would seal its extras, and which
     * members hold the gate closed (point-in-time, advisory).
     */
    @ReactMethod
    fun meshGroupRichReadiness(groupId: String, promise: Promise) {
        try {
            val readiness = engineHost.meshGroupRichReadiness(groupId)
            val result = Arguments.createMap().apply {
                putBoolean("ready", readiness["ready"] as? Boolean ?: false)
                val unknownMembers = Arguments.createArray()
                @Suppress("UNCHECKED_CAST")
                (readiness["unknownMembers"] as? List<String>)?.forEach { unknownMembers.pushString(it) }
                putArray("unknownMembers", unknownMembers)
            }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to get group rich readiness")
        }
    }

    /**
     * Relay-side registration state of a group ('synced' | 'pending' |
     * 'unsynced'). Point-in-time; transitions surface as
     * `group_relay_sync_changed` events.
     */
    @ReactMethod
    fun meshGroupRelaySyncState(groupId: String, promise: Promise) {
        try {
            promise.resolve(engineHost.meshGroupRelaySyncState(groupId))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to get relay sync state")
        }
    }

    /**
     * Registers (or re-registers) a group with the relay server on demand —
     * the supported path before relay-dependent raw server commands
     * (invite links). Outcome arrives as `group_relay_sync_changed`;
     * resolves true when the frame was queued or the group is already
     * synced, false when relay grouping is disabled or Internet is down.
     */
    @ReactMethod
    fun meshRequestGroupRelayRegistration(groupId: String, promise: Promise) {
        try {
            promise.resolve(engineHost.meshRequestGroupRelayRegistration(groupId))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to request relay registration")
        }
    }

    @ReactMethod
    fun meshSetMemberRole(groupId: String, userId: String, role: String, promise: Promise) {
        try {
            engineHost.meshSetMemberRole(groupId, userId, role)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to set member role")
        }
    }

    @ReactMethod
    fun meshGetMemberRole(groupId: String, userId: String, promise: Promise) {
        try {
            promise.resolve(engineHost.meshGetMemberRole(groupId, userId))
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to get member role")
        }
    }

    @ReactMethod
    fun meshGetGroupRoles(groupId: String, promise: Promise) {
        try {
            val result = Arguments.createMap()
            engineHost.meshGetGroupRoles(groupId).forEach { (userId, role) -> result.putString(userId, role) }
            promise.resolve(result)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to get group roles")
        }
    }

    @ReactMethod
    fun meshRenameGroup(groupId: String, newName: String, promise: Promise) {
        try {
            engineHost.meshRenameGroup(groupId, newName)
            promise.resolve(null)
        } catch (e: Exception) {
            rejectWithProtocolError(promise, e, "ERROR_MESH_GROUP", "Failed to rename group")
        }
    }

    /**
     * One-shot relay presence query for a peer (last-seen display, pre-send
     * checks). Fire-and-event: resolves true if the query was sent; the
     * answer arrives as the SDK's `presence_updated` event.
     * `options.force` parks the query through the chat-open reconnect
     * window instead of failing fast (see InternetManager.checkPresence).
     */
    @ReactMethod
    fun checkInternetPresence(userId: String, options: ReadableMap, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.checkInternetPresence(this, userId, options, promise)
    }

    /**
     * Sends a raw, caller-built relay command verbatim over the SDK's
     * socket (invite-link lifecycle and other server-plane ops). Resolves
     * true when written; false when invalid JSON or not connected and
     * authenticated. Responses arrive as `internet_server_message` events.
     */
    @ReactMethod
    fun internetSendRawCommand(json: String, promise: Promise) {
        ProtocolEngineHostBridgeReactNative.internetSendRawCommand(this, json, promise)
    }

    /**
     * Whether the internet socket is connected AND relay-authenticated —
     * the same gate `internetSendRawCommand` checks. Point-in-time;
     * transitions arrive as `internet_status_changed` events. Resolves
     * false (never rejects) when the internet transport isn't initialized.
     */
    @ReactMethod
    fun internetIsReady(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.internetIsReady(this, promise)
    }

    /**
     * Whether the relay displaced this session ("connected elsewhere") and
     * latched the transport stopped. Resolves false (never rejects) when the
     * internet transport isn't initialized.
     *
     * The pull half of the supersede contract, and the disambiguation
     * [internetIsReady] cannot offer: a `false` there from an ordinary
     * disconnect — which reconnects itself — and a `false` from a displacement
     * — which will not reconnect on its own, ever — are indistinguishable.
     * True here means the only recovery is a deliberate re-enable
     * (`enableTransport('internet', …)`), which clears the latch.
     *
     * Complements the `internet_session_superseded` event rather than
     * replacing it: the event tells an app that is listening, this answers an
     * app that asks — including one whose process died and came back, which no
     * in-memory hold survives.
     */
    @ReactMethod
    fun internetIsSuperseded(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.internetIsSuperseded(this, promise)
    }

    /**
     * Forces an immediate teardown + reconnect + re-authenticate of the
     * internet socket, bypassing the exponential backoff — the deterministic
     * recovery for a foreground-after-background where the cached ready flags
     * may be stale (see InternetManager.forceReconnect). Resolves true when
     * the request reached a live internet transport ("accepted", not
     * "reconnected" — also true when the transport is initialized but not
     * running, where forceReconnect is a deliberate no-op); false (never
     * rejects) only when the internet transport isn't initialized.
     */
    @ReactMethod
    fun internetForceReconnect(promise: Promise) {
        ProtocolEngineHostBridgeReactNative.internetForceReconnect(this, promise)
    }

    /**
     * Start the foreground service to prevent process death while mesh is active.
     */
    private fun startForegroundService() {
        try {
            // Register before the service exists, so the notification's Stop
            // action always has a host to defer to. The callback lives in a
            // companion field and captures this module, so teardown must clear
            // it or the module — and the ReactContext it holds — is pinned for
            // the process lifetime. Kept in a field so that clear can check it
            // is still ours (see releaseForegroundStopCallback).
            val stopCallback = { handleUserRequestedMeshStop() }
            foregroundStopCallback = stopCallback
            MeshForegroundService.registerStopRequestCallback(stopCallback)
            MeshForegroundService.start(reactApplicationContext)
            emitDiagnostic("info", "Mesh foreground service started")
        } catch (e: Exception) {
            android.util.Log.w(NAME, "Failed to start foreground service: ${e.message}", e)
            emitDiagnostic("warning", "Foreground service start failed", mapOf(
                "error" to (e.message ?: "unknown")
            ))
        }
    }

    /**
     * Stop the foreground service.
     */
    private fun stopForegroundService() {
        try {
            // Surrender our registration along with the stop. That slot is also
            // the service's sticky-restart liveness signal, so it has to fall
            // when the mesh does — not when this module does, which is later
            // and may never happen at all. Left set over a stopped mesh, a
            // sticky restart reads it as "a host has mesh up" and re-posts
            // "Mesh Active" over a protocol that is gone. Cleared by identity
            // inside stop(), so a reload's replacement host stays armed.
            val ours = foregroundStopCallback
            foregroundStopCallback = null
            MeshForegroundService.stop(reactApplicationContext, ours)
            emitDiagnostic("info", "Mesh foreground service stopped")
        } catch (e: Exception) {
            android.util.Log.w(NAME, "Failed to stop foreground service: ${e.message}", e)
        }
    }

    private fun parseInternetConfig(config: ReadableMap?): Pair<String, Int> {
        var serverAddress: String? = null
        if (config != null) {
            if (config.hasKey("serverAddress") && !config.isNull("serverAddress")) {
                serverAddress = config.getString("serverAddress")
            } else if (config.hasKey("server_url") && !config.isNull("server_url")) {
                serverAddress = config.getString("server_url")
            }
        }

        if (serverAddress.isNullOrBlank()) {
            throw IllegalArgumentException("Internet transport requires serverAddress")
        }

        var port: Int? = null
        if (config != null) {
            if (config.hasKey("port") && !config.isNull("port")) {
                port = config.getInt("port")
            } else if (config.hasKey("serverPort") && !config.isNull("serverPort")) {
                port = config.getInt("serverPort")
            }
        }

        val uri = try {
            Uri.parse(serverAddress)
        } catch (_: Exception) {
            null
        }

        var host = serverAddress
        if (uri != null && !uri.scheme.isNullOrBlank()) {
            host = uri.host ?: serverAddress
            if (uri.port != -1) {
                port = uri.port
            }
        }

        if (port == null) {
            port = when (uri?.scheme?.lowercase()) {
                "wss", "https" -> Constants.HTTPS_PORT
                "ws", "http" -> Constants.HTTP_PORT
                else -> Constants.HTTPS_PORT
            }
        }

        val safePort = port!!.coerceIn(0, 65535)
        return host!! to safePort
    }

    /**
     * The one transport-name mapper on this bridge. Every transport the core
     * has is here, and an unknown name is an error rather than a default:
     * `react_native_transport_name_mappers_know_every_transport` pins both.
     */
    private fun mapTransportType(type: String): TransportType {
        return when (type.lowercase()) {
            "internet" -> TransportType.INTERNET
            "ble" -> TransportType.BLE
            "wifidirect", "wifi_direct" -> TransportType.WI_FI_DIRECT
            "reticulum" -> TransportType.RETICULUM
            "nostr" -> TransportType.NOSTR
            else -> throw IllegalArgumentException("Unsupported transport type: $type")
        }
    }

    private fun buildTopologyJson(topology: NetworkTopology): String {
        val linksArray = JSONArray()
        val nodesArray = JSONArray()
        val connectionCounts = mutableMapOf<String, Int>()
        val transportsByNode = mutableMapOf<String, MutableSet<String>>()

        topology.links.forEach { link ->
            val transportName = normalizeTransportName(link.transport)
            val linkObj = JSONObject().apply {
                put("from", link.sourceId)
                put("to", link.targetId)
                put("quality", link.quality.toDouble())
                put("transport", transportName)
                put("rssi", JSONObject.NULL)
            }
            linksArray.put(linkObj)

            transportsByNode.getOrPut(link.sourceId) { mutableSetOf() }.add(transportName)
            transportsByNode.getOrPut(link.targetId) { mutableSetOf() }.add(transportName)

            connectionCounts[link.sourceId] = (connectionCounts[link.sourceId] ?: 0) + 1
            connectionCounts[link.targetId] = (connectionCounts[link.targetId] ?: 0) + 1
        }

        topology.nodes.forEach { node ->
            val transportsArray = JSONArray()
            transportsByNode[node.nodeId]?.forEach { transportsArray.put(it) }

            // Prefer the node's own connection count when the visualizer has
            // one; fall back to counting the links we can see.
            val reportedConnections = node.connectionCount.toInt()
            val nodeObj = JSONObject().apply {
                put("user_id", node.nodeId)
                put("role", node.role.lowercase())
                put(
                    "connection_count",
                    if (reportedConnections > 0) reportedConnections else (connectionCounts[node.nodeId] ?: 0)
                )
                put("battery_level", node.batteryLevel?.toInt() ?: JSONObject.NULL)
                put("last_seen", node.lastSeenMs.toLong() / Constants.MILLISECONDS_PER_SECOND)
                put("transports", transportsArray)
            }
            nodesArray.put(nodeObj)
        }

        val avgQuality = if (topology.links.isNotEmpty()) {
            topology.links.map { it.quality }.average()
        } else {
            0.0
        }

        val statsObj = JSONObject().apply {
            put("total_nodes", topology.nodes.size)
            put("relay_nodes", topology.nodes.count { it.role.equals("relay", true) })
            put("total_connections", topology.links.size)
            put("avg_link_quality", avgQuality)
            put("network_diameter", JSONObject.NULL)
        }

        // The address, never the profile. `local_user_id` is the one field in
        // this payload that names *this device to other devices*, and the
        // profile is a local storage-namespace selector that is not supposed
        // to leave the device at all — handing it to JS under an id-shaped key
        // invites an app to store or display it as its identity. Empty until
        // `initializeMlsWithSecureStorage` derives the address; empty is the
        // honest answer, because before that there is no identity to report.
        val root = JSONObject().apply {
            put("timestamp", System.currentTimeMillis() / Constants.MILLISECONDS_PER_SECOND)
            put("local_user_id", protocol?.localAddress() ?: "")
            put("nodes", nodesArray)
            put("links", linksArray)
            put("stats", statsObj)
        }

        return root.toString()
    }

    private fun buildMessageStatsJson(stats: List<MessageStats>): String {
        val array = JSONArray()
        stats.forEach { stat ->
            val obj = JSONObject().apply {
                put("message_id", stat.messageId)
                put("sender", JSONObject.NULL)
                put("recipient", JSONObject.NULL)
                put("sent_at", stat.sentAtMs.toLong())
                if (stat.deliveredAtMs != null) {
                    put("delivered_at", stat.deliveredAtMs!!.toLong())
                    put("latency_ms", (stat.deliveredAtMs!!.toLong() - stat.sentAtMs.toLong()).coerceAtLeast(0))
                } else {
                    put("delivered_at", JSONObject.NULL)
                    put("latency_ms", JSONObject.NULL)
                }
                put("hop_count", stat.hopCount.toInt())
                put("transport", JSONObject.NULL)
                put("retry_count", 0)
                put("status", stat.status)
            }
            array.put(obj)
        }
        return array.toString()
    }

    private fun normalizeTransportName(name: String): String {
        return when (name.lowercase()) {
            "ble" -> "ble"
            "internet" -> "internet"
            "wifi_direct", "wifidirect", "wi_fi_direct" -> "wifiDirect"
            else -> name.lowercase()
        }
    }
}
