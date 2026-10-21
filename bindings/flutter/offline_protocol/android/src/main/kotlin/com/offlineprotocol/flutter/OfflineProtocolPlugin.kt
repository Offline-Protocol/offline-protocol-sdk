package com.offlineprotocol.flutter

import android.app.Activity
import android.app.Application
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import java.util.Collections
import com.offlineprotocol.EventReplayRing
import com.offlineprotocol.ForegroundReconnectPolicy
import com.offlineprotocol.ProtocolEngineHost
import com.offlineprotocol.StickyEventBuffer
import com.offlineprotocol.StickyEventDispatcher
import com.offlineprotocol.TeardownSequence
import com.offlineprotocol.mapProtocolBridgeError
import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.embedding.engine.plugins.activity.ActivityAware
import io.flutter.embedding.engine.plugins.activity.ActivityPluginBinding
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.MethodChannel.MethodCallHandler
import io.flutter.plugin.common.MethodChannel.Result
import org.json.JSONObject
import uniffi.offline_protocol.AppState
import uniffi.offline_protocol.EventCallback
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Flutter host for the Offline Protocol engine.
 *
 * All UniFFI calls run on a dedicated engine thread ([engineExecutor]); results
 * and [EventChannel] deliveries are posted on the main looper ([mainHandler]).
 * See bridge contract K2.
 */
class OfflineProtocolPlugin :
    FlutterPlugin,
    MethodCallHandler,
    EventChannel.StreamHandler,
    ActivityAware {

    private lateinit var channel: MethodChannel
    private var eventsChannel: EventChannel? = null
    @Volatile
    private var applicationContext: android.content.Context? = null

    private val engineHost by lazy {
        ProtocolEngineHost(logTag = TAG) { level, message, context ->
            Log.println(
                when (level) {
                    "error" -> Log.ERROR
                    "warning" -> Log.WARN
                    else -> Log.INFO
                },
                TAG,
                "$message ${context ?: emptyMap()}",
            )
        }
    }

    private val mainHandler = Handler(Looper.getMainLooper())
    private val attached = AtomicBoolean(false)

    private val foregroundReconnectPolicy = ForegroundReconnectPolicy()
    private var activityBinding: ActivityPluginBinding? = null
    private var activityLifecycleCallbacks: Application.ActivityLifecycleCallbacks? = null

    /** Process visibility for telemetry (C12), same edges as React Native. */
    private val startedActivities: MutableSet<Activity> = Collections.synchronizedSet(
        Collections.newSetFromMap(java.util.IdentityHashMap()),
    )
    @Volatile
    private var sawActivityStart = false
    private var processLifecycleCallbacks: Application.ActivityLifecycleCallbacks? = null

    @Volatile
    private var eventSink: EventChannel.EventSink? = null
    private val eventSeqLock = Any()
    private var eventSeq = 0
    private val eventReplay = EventReplayRing()

    /** One-shot events when the EventChannel has no listener (RN sticky parity). */
    private val oneShotEvents = StickyEventDispatcher(
        buffer = StickyEventBuffer(),
        canEmit = { eventSink != null },
        emit = { eventJson -> emitEventJsonForSticky(eventJson) },
        schedule = { runnable ->
            mainHandler.post(runnable)
            true
        },
    )

    private val channelEnvironment: FlutterEngineChannel.Environment
        get() = FlutterEngineChannel.Environment(
            context = applicationContext ?: throw IllegalStateException("Plugin not attached"),
            activityProvider = { activityBinding?.activity },
            internetHandlers = internetTransportEventHandlers(),
            replayEvents = { fromSeq, toSeq -> replayEventsOnEngine(fromSeq, toSeq) },
            tearDownProtocol = { tearDownProtocol() },
            createOnEngine = { configJson -> createOnEngine(configJson) },
            startOnEngine = { startOnEngine() },
            stopOnEngine = { stopOnEngine() },
            seedAppState = { seedTelemetryAppState() },
            discardOneShotEvent = { key -> oneShotEvents.discard(key) },
        )

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        applicationContext = binding.applicationContext
        attached.set(true)
        installProcessLifecycleWatcher(binding.applicationContext)
        channel = MethodChannel(binding.binaryMessenger, CHANNEL_NAME)
        channel.setMethodCallHandler(this)
        eventsChannel = EventChannel(binding.binaryMessenger, EVENTS_CHANNEL_NAME).also {
            it.setStreamHandler(this)
        }
    }

    override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        if (!attached.getAndSet(false)) {
            return
        }
        removeProcessLifecycleWatcher()
        detachActivityLifecycle()
        eventsChannel?.setStreamHandler(null)
        eventsChannel = null
        eventSink = null
        if (::channel.isInitialized) {
            channel.setMethodCallHandler(null)
        }
        applicationContext = null
        runOnEngine("detach teardown") { tearDownProtocol() }
    }

    private fun runOnEngine(what: String, block: () -> Unit) {
        try {
            engineExecutor.execute {
                try {
                    block()
                } catch (e: Throwable) {
                    Log.w(TAG, "$what failed", e)
                }
            }
        } catch (e: RejectedExecutionException) {
            Log.w(TAG, "$what rejected by the engine executor", e)
        }
    }

    override fun onAttachedToActivity(binding: ActivityPluginBinding) {
        val trackedActivity = binding.activity
        val callbacks = object : Application.ActivityLifecycleCallbacks {
            override fun onActivityPaused(activity: Activity) {
                if (activity !== trackedActivity) return
                runOnEngine("background mark") {
                    foregroundReconnectPolicy.didEnterBackground(
                        nowMs = SystemClock.elapsedRealtime(),
                    )
                }
            }

            override fun onActivityResumed(activity: Activity) {
                if (activity !== trackedActivity) return
                runOnEngine("foreground reconnect") {
                    if (foregroundReconnectPolicy.shouldReconnectOnForeground(
                            nowMs = SystemClock.elapsedRealtime(),
                        )
                    ) {
                        try {
                            engineHost.internetManager?.forceReconnect()
                        } catch (e: Exception) {
                            Log.w(TAG, "Internet forceReconnect on foreground failed", e)
                        }
                    }
                }
            }

            override fun onActivityCreated(activity: Activity, savedInstanceState: android.os.Bundle?) {}
            override fun onActivityStarted(activity: Activity) {}
            override fun onActivityStopped(activity: Activity) {}
            override fun onActivitySaveInstanceState(activity: Activity, outState: android.os.Bundle) {}
            override fun onActivityDestroyed(activity: Activity) {}
        }
        activityBinding = binding
        activityLifecycleCallbacks = callbacks
        trackedActivity.application.registerActivityLifecycleCallbacks(callbacks)
    }

    override fun onDetachedFromActivityForConfigChanges() {
        detachActivityLifecycle()
    }

    override fun onReattachedToActivityForConfigChanges(binding: ActivityPluginBinding) {
        onAttachedToActivity(binding)
    }

    override fun onDetachedFromActivity() {
        detachActivityLifecycle()
    }

    private fun detachActivityLifecycle() {
        val callbacks = activityLifecycleCallbacks
        if (callbacks != null) {
            activityBinding?.activity?.application?.unregisterActivityLifecycleCallbacks(callbacks)
        }
        activityLifecycleCallbacks = null
        activityBinding = null
    }

    private fun installProcessLifecycleWatcher(context: android.content.Context) {
        val application = context.applicationContext as? Application ?: return
        removeProcessLifecycleWatcher()
        val watcher = object : Application.ActivityLifecycleCallbacks {
            override fun onActivityStarted(activity: Activity) {
                sawActivityStart = true
                startedActivities.add(activity)
                oneShotEvents.flush()
                runOnEngine("telemetry foreground") { notifyAppStateQuietly(AppState.ACTIVE) }
            }

            override fun onActivityStopped(activity: Activity) {
                startedActivities.remove(activity)
                if (startedActivities.isEmpty() && !activity.isChangingConfigurations) {
                    runOnEngine("telemetry background") { notifyAppStateQuietly(AppState.BACKGROUND) }
                }
            }

            override fun onActivityDestroyed(activity: Activity) {
                startedActivities.remove(activity)
            }

            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}
            override fun onActivityResumed(activity: Activity) {}
            override fun onActivityPaused(activity: Activity) {}
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
        }
        application.registerActivityLifecycleCallbacks(watcher)
        processLifecycleCallbacks = watcher
    }

    private fun removeProcessLifecycleWatcher() {
        val watcher = processLifecycleCallbacks ?: return
        processLifecycleCallbacks = null
        (applicationContext?.applicationContext as? Application)
            ?.unregisterActivityLifecycleCallbacks(watcher)
        startedActivities.clear()
        sawActivityStart = false
    }

    /**
     * Seeds telemetry the way [OfflineProtocolModule.seedAppState] does on React Native.
     */
    private fun seedTelemetryAppState(): AppState = when {
        sawActivityStart -> if (startedActivities.isEmpty()) AppState.BACKGROUND else AppState.ACTIVE
        activityBinding?.activity != null -> AppState.ACTIVE
        else -> AppState.BACKGROUND
    }

    private fun notifyAppStateQuietly(state: AppState) {
        try {
            engineHost.protocol?.notifyAppState(state)
        } catch (e: Exception) {
            Log.d(TAG, "Telemetry lifecycle transition skipped: the protocol handle is gone", e)
        }
    }

    override fun onListen(arguments: Any?, events: EventChannel.EventSink?) {
        eventSink = events
        oneShotEvents.flush()
    }

    override fun onCancel(arguments: Any?) {
        eventSink = null
    }

    override fun onMethodCall(call: MethodCall, result: Result) {
        if (!attached.get()) {
            postDetachedError(result)
            return
        }
        try {
            engineExecutor.execute {
                try {
                    val payload = FlutterEngineChannel.dispatch(
                        engineHost,
                        channelEnvironment,
                        call.method,
                        call.arguments,
                    )
                    mainHandler.post { result.success(payload) }
                } catch (e: Throwable) {
                    Log.e(TAG, "${call.method} failed", e)
                    mainHandler.post {
                        val mapped = mapProtocolBridgeError(e)
                        when {
                            mapped != null -> result.error(mapped.code, mapped.message, null)
                            e is LinkageError -> result.error(
                                NATIVE_LIBRARY_ERROR,
                                "The Offline Protocol native library could not be loaded: ${e.message}",
                                null,
                            )
                            else -> result.error("ERROR", e.message, null)
                        }
                    }
                }
            }
        } catch (e: RejectedExecutionException) {
            Log.w(TAG, "Method call rejected: engine executor shut down", e)
            postDetachedError(result)
        }
    }

    private fun postDetachedError(result: Result) {
        mainHandler.post {
            result.error("ERROR", "Plugin detached", null)
        }
    }

    private fun emitEventJson(eventJson: String) {
        if (eventSink == null) return
        emitEventJsonForSticky(eventJson)
    }

    /** @return true when a listener is attached and the event was queued for delivery. */
    private fun emitEventJsonForSticky(eventJson: String): Boolean {
        val sink = eventSink ?: return false
        val seq: Int
        synchronized(eventSeqLock) {
            seq = eventSeq
            eventReplay.record(seq, eventJson)
            eventSeq += 1
        }
        mainHandler.post {
            sink.success(
                mapOf(
                    "eventJson" to eventJson,
                    "seq" to seq,
                ),
            )
        }
        return true
    }

    private fun sendStickyOneShotEvent(typeKey: String, eventJson: String) {
        oneShotEvents.send(typeKey, eventJson)
    }

    private fun replayEventsOnEngine(fromSeq: Int, toSeq: Int): List<Map<String, Any>> =
        eventReplay.replay(fromSeq, toSeq).map { entry ->
            mapOf(
                "seq" to entry.seq,
                "eventJson" to entry.eventJson,
            )
        }

    private fun createOnEngine(configJson: String) {
        tearDownProtocol()
        val ctx = applicationContext ?: throw IllegalStateException("Plugin not attached")
        engineHost.create(
            ctx,
            configJson,
            object : EventCallback {
                override fun onEvent(eventJson: String) {
                    try {
                        emitEventJson(eventJson)
                    } catch (e: Throwable) {
                        Log.e(TAG, "Core event not handed to Dart", e)
                    }
                }
            },
            internetTransportEventHandlers(),
        )
    }

    private fun internetTransportEventHandlers(): ProtocolEngineHost.InternetTransportEventHandlers =
        ProtocolEngineHost.InternetTransportEventHandlers(
            serverMessageEmitter = { rawJson -> emitEventJson(buildServerMessageEvent(rawJson)) },
            connectionStatusEmitter = { connected, authenticated ->
                emitEventJson(buildInternetStatusEvent(connected, authenticated))
            },
            supersededEmitter = { reason ->
                sendStickyOneShotEvent(
                    com.offlineprotocol.SupersededLatchPolicy.EVENT_TYPE,
                    com.offlineprotocol.SupersededLatchPolicy.eventJson(reason),
                )
            },
        )

    private fun buildServerMessageEvent(rawJson: String): String {
        val json = JSONObject()
        json.put("type", "internet_server_message")
        json.put("json", rawJson)
        return json.toString()
    }

    private fun buildInternetStatusEvent(connected: Boolean, authenticated: Boolean): String {
        val json = JSONObject()
        json.put("type", "internet_status_changed")
        json.put("connected", connected)
        json.put("authenticated", authenticated)
        return json.toString()
    }

    private fun startOnEngine() {
        oneShotEvents.beginSession()
        engineHost.startProtocolCore()
        engineHost.startBleTransportIfPresent()
        val ctx = applicationContext
        if (ctx != null) {
            FlutterMeshForegroundKeepAlive.start(ctx, ::handleUserRequestedMeshStop)
        }
    }

    private fun handleUserRequestedMeshStop() {
        engineExecutor.execute {
            try {
                stopOnEngine(skipForegroundStop = true)
            } catch (e: Exception) {
                Log.e(TAG, "User-requested mesh stop failed", e)
            } finally {
                val json = JSONObject()
                json.put("type", EVENT_MESH_STOPPED_BY_USER)
                sendStickyOneShotEvent(EVENT_MESH_STOPPED_BY_USER, json.toString())
                applicationContext?.let { FlutterMeshForegroundKeepAlive.stop(it) }
            }
        }
    }

    private fun stopOnEngine(skipForegroundStop: Boolean = false) {
        val teardown = TeardownSequence()
        if (!skipForegroundStop) {
            teardown.step("mesh foreground service") {
                applicationContext?.let { FlutterMeshForegroundKeepAlive.stop(it) }
            }
        }
        teardown.step("process scheduler") { engineHost.backgroundProcessor.stopProcessScheduler() }
        teardown.step("BLE manager") { engineHost.bleTransport?.stop() }
        teardown.step("Internet manager") { engineHost.internetManager?.stop() }
        teardown.step("Wi-Fi Direct manager") { engineHost.wifiDirectManager?.stop() }
        teardown.step("Reticulum manager") { engineHost.reticulumManager?.stop() }
        teardown.step("Nostr manager") { engineHost.nostrManager?.stop() }
        teardown.step("protocol") { engineHost.protocol?.stop() }
        teardown.firstFailureOrNull()?.let { throw it }
    }

    private fun tearDownProtocol() {
        oneShotEvents.endSession()
        applicationContext?.let { FlutterMeshForegroundKeepAlive.stop(it) }
        engineHost.backgroundProcessor.stopProcessScheduler()
        val teardown = TeardownSequence()
        teardown.step("BLE manager") { engineHost.bleTransport?.stop() }
        teardown.step("Internet manager") { engineHost.internetManager?.stop() }
        teardown.step("Wi-Fi Direct manager") { engineHost.wifiDirectManager?.stop() }
        teardown.step("Reticulum manager") { engineHost.reticulumManager?.stop() }
        teardown.step("Nostr manager") { engineHost.nostrManager?.stop() }
        try {
            engineHost.protocol?.stop()
        } catch (e: Exception) {
            Log.w(TAG, "stop during teardown", e)
        }
        engineHost.releaseHandlesAfterStop()
        synchronized(eventSeqLock) {
            eventSeq = 0
            eventReplay.clear()
        }
    }

    companion object {
        private const val TAG = "OfflineProtocolPlugin"
        private const val EVENT_MESH_STOPPED_BY_USER = "mesh_stopped_by_user"
        private const val CHANNEL_NAME = "com.offlineprotocol/offline_protocol"
        private const val EVENTS_CHANNEL_NAME = "com.offlineprotocol/offline_protocol_events"
        private const val ENGINE_IDLE_SECONDS = 30L
        private const val NATIVE_LIBRARY_ERROR = "NATIVE_LIBRARY_UNAVAILABLE"

        private val engineExecutor: ThreadPoolExecutor =
            ThreadPoolExecutor(1, 1, ENGINE_IDLE_SECONDS, TimeUnit.SECONDS, LinkedBlockingQueue()) { runnable ->
                Thread(runnable, "OfflineProtocolEngine").apply { isDaemon = true }
            }.apply { allowCoreThreadTimeOut(true) }
    }
}
