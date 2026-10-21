package com.offlineprotocol.flutter

import android.app.Activity
import android.app.Application
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import com.offlineprotocol.AppContainerProtocolStateStorage
import com.offlineprotocol.EventReplayRing
import com.offlineprotocol.ForegroundReconnectPolicy
import com.offlineprotocol.InitialRuntimeConfigApplier
import com.offlineprotocol.InternetManager
import com.offlineprotocol.InternetTransportBootstrap
import com.offlineprotocol.MlsSecureStorage
import com.offlineprotocol.ProtocolConfigParser
import com.offlineprotocol.StorageNamespace
import com.offlineprotocol.TeardownSequence
import com.offlineprotocol.TransportManagerListener
import com.offlineprotocol.TransportState
import com.offlineprotocol.WifiDirectGroupFormation
import com.offlineprotocol.WifiDirectManager
import com.offlineprotocol.ble.BleTransportFacade
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
import uniffi.offline_protocol.BleTransportCallback
import uniffi.offline_protocol.EventCallback
import uniffi.offline_protocol.MessagePriority
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.ProtocolConfig
import uniffi.offline_protocol.WifiDirectTransportCallback
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
 *
 * The executor is one per process, in the companion object, and is never shut
 * down. Flutter creates a new plugin instance for every engine, so an executor
 * per instance would let a new engine's `create` run while the previous
 * engine's teardown is still blocked on transport I/O: two cores under one
 * identity, both on BLE and the relay. Detach queues the teardown behind every
 * call already submitted and returns, so the main thread never waits, and any
 * later call from any instance queues behind that teardown on the same thread.
 * The one thread exits after it has been idle for [ENGINE_IDLE_SECONDS].
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
    private var protocol: OfflineProtocol? = null
    private var currentConfig: ProtocolConfig? = null
    private var internetManager: InternetManager? = null
    private var bleTransport: BleTransportFacade? = null
    private var wifiDirectManager: WifiDirectManager? = null

    private val mainHandler = Handler(Looper.getMainLooper())
    private val attached = AtomicBoolean(false)

    private val foregroundReconnectPolicy = ForegroundReconnectPolicy()
    private var activityBinding: ActivityPluginBinding? = null
    private var activityLifecycleCallbacks: Application.ActivityLifecycleCallbacks? = null

    @Volatile
    private var eventSink: EventChannel.EventSink? = null
    private val eventSeqLock = Any()
    private var eventSeq = 0
    private val eventReplay = EventReplayRing()

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        applicationContext = binding.applicationContext
        attached.set(true)
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
        detachActivityLifecycle()
        eventsChannel?.setStreamHandler(null)
        eventsChannel = null
        eventSink = null
        if (::channel.isInitialized) {
            channel.setMethodCallHandler(null)
        }
        // Cleared here, on the main thread, and not by the queued teardown:
        // a teardown that ran after a re-attach would clear the new context.
        applicationContext = null
        // Queued, never waited on: engine stop can block on transport I/O, and
        // the main thread is not where that wait belongs (K2).
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
                            internetManager?.forceReconnect()
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

    override fun onListen(arguments: Any?, events: EventChannel.EventSink?) {
        eventSink = events
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
                    val payload: Any? = when (call.method) {
                        "create" -> {
                            createOnEngine(call.arguments)
                            null
                        }
                        "initializeMlsWithSecureStorage" -> {
                            initializeMlsOnEngine()
                            null
                        }
                        "start" -> {
                            startOnEngine()
                            null
                        }
                        "stop" -> {
                            stopOnEngine()
                            null
                        }
                        "destroy" -> {
                            tearDownProtocol()
                            null
                        }
                        "localAddress" -> protocol?.localAddress()
                        "sendMessage" -> sendMessageOnEngine(call.arguments)
                        "enableInternetTransport" -> {
                            enableTransportOnEngine("internet", call.arguments)
                            null
                        }
                        "enableTransport" -> {
                            val map = call.arguments as? Map<*, *>
                                ?: throw IllegalArgumentException("enableTransport expects a map")
                            val type = map["type"] as? String
                                ?: throw IllegalArgumentException("enableTransport requires type")
                            val configJson = map["config"] as? String
                            enableTransportOnEngine(type, configJson)
                            null
                        }
                        "replayEvents" -> replayEventsOnEngine(call.arguments)
                        else -> {
                            mainHandler.post { result.notImplemented() }
                            return@execute
                        }
                    }
                    mainHandler.post { result.success(payload) }
                } catch (e: Throwable) {
                    // Throwable, not Exception: the generated bindings load the
                    // native library on first use, and a missing or mismatched
                    // .so throws UnsatisfiedLinkError. Uncaught on this thread it
                    // kills the process and the call is never answered.
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
        val sink = eventSink ?: return
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
    }

    private fun replayEventsOnEngine(arguments: Any?): List<Map<String, Any>> {
        val map = arguments as? Map<*, *>
            ?: throw IllegalArgumentException("replayEvents expects a map")
        val fromSeq = (map["fromSeq"] as? Number)?.toInt()
            ?: throw IllegalArgumentException("replayEvents requires fromSeq")
        val toSeq = (map["toSeq"] as? Number)?.toInt()
            ?: throw IllegalArgumentException("replayEvents requires toSeq")
        return eventReplay.replay(fromSeq, toSeq).map { entry ->
            mapOf(
                "seq" to entry.seq,
                "eventJson" to entry.eventJson,
            )
        }
    }

    private fun createOnEngine(arguments: Any?) {
        val configJson = arguments as? String
            ?: throw IllegalArgumentException("create expects a JSON string")
        // Validated before the running engine is torn down: a config this
        // plugin refuses must leave the current session as it was.
        val parsed = ProtocolConfigParser.parse(configJson)
        refuseUnhostedTransports(parsed.coreConfig)
        tearDownProtocol()
        val proto = OfflineProtocol(parsed.coreConfig)
        proto.setEventCallback(object : EventCallback {
            override fun onEvent(eventJson: String) {
                try {
                    emitEventJson(eventJson)
                } catch (e: Throwable) {
                    Log.e(TAG, "Core event not handed to Dart", e)
                }
            }
        })
        InitialRuntimeConfigApplier.apply(proto, parsed.rawJson)
        protocol = proto
        currentConfig = parsed.coreConfig

        val ctx = applicationContext
        val config = parsed.coreConfig
        if (config.bleEnabled && ctx != null) {
            bleTransport = BleTransportFacade(
                ctx,
                proto,
                config.profile,
                config.appId,
            ) { level, message, context ->
                Log.println(
                    when (level) {
                        "error" -> Log.ERROR
                        "warning" -> Log.WARN
                        else -> Log.INFO
                    },
                    TAG,
                    "$message ${context ?: emptyMap()}",
                )
            }.also { manager ->
                manager.listener = object : TransportManagerListener {
                    override fun onTransportStateChanged(
                        manager: com.offlineprotocol.TransportManager,
                        state: TransportState,
                    ) {
                        Log.i(TAG, "BLE transport state: ${state.name.lowercase()}")
                    }

                    override fun onTransportError(
                        manager: com.offlineprotocol.TransportManager,
                        error: Throwable,
                    ) {
                        Log.w(TAG, "BLE transport error", error)
                    }

                    override fun onTransportDiagnostic(
                        manager: com.offlineprotocol.TransportManager,
                        level: String,
                        message: String,
                        context: Map<String, Any?>,
                    ) {
                        Log.println(
                            when (level) {
                                "error" -> Log.ERROR
                                "warning" -> Log.WARN
                                else -> Log.INFO
                            },
                            TAG,
                            "$message $context",
                        )
                    }
                }
            }
        }
        wireTransportCallbacks(proto)
    }

    /**
     * Refuses a config that enables a transport this plugin has no manager
     * for. Accepted, the core would route to a transport nothing serves and
     * messages would wait on it with no error anywhere.
     */
    private fun refuseUnhostedTransports(config: ProtocolConfig) {
        val unhosted = buildList {
            if (config.reticulumEnabled) add("reticulumEnabled")
            if (config.nostrEnabled) add("nostrEnabled")
        }
        if (unhosted.isNotEmpty()) {
            throw IllegalArgumentException(
                "${unhosted.joinToString()} is not supported by the Flutter plugin yet; set it to false",
            )
        }
    }

    private fun wireTransportCallbacks(proto: OfflineProtocol) {
        bleTransport?.let { manager ->
            try {
                proto.setBleTransportCallback(object : BleTransportCallback {
                    override fun onFragmentsAvailable() {
                        manager.onFragmentsAvailable()
                    }
                })
            } catch (e: Throwable) {
                Log.w(TAG, "BLE transport callback not available", e)
            }
        }
        if (currentConfig?.wifiDirectEnabled == true) {
            try {
                proto.setWifiDirectTransportCallback(object : WifiDirectTransportCallback {
                    override fun onMessagesAvailable() {
                        wifiDirectManager?.onMessagesAvailable()
                    }
                })
            } catch (e: Throwable) {
                Log.w(TAG, "Wi-Fi Direct transport callback not available", e)
            }
        }
    }

    private fun initializeMlsOnEngine() {
        val ctx = applicationContext
            ?: throw IllegalStateException("Plugin not attached")
        val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
        val config = currentConfig ?: throw IllegalStateException("Protocol config not initialized")
        val accountNamespace = StorageNamespace.account(config.appId, config.profile)
        val secureStorage = MlsSecureStorage(ctx, accountNamespace)
        val protocolStateStorage = AppContainerProtocolStateStorage(ctx, accountNamespace)
        proto.initializeMls(secureStorage, protocolStateStorage)
    }

    private fun startOnEngine() {
        val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
        proto.start()
        bleTransport?.let { manager ->
            try {
                manager.start()
            } catch (e: Exception) {
                Log.e(TAG, "Failed to start BLE manager", e)
            }
        }
    }

    private fun stopOnEngine() {
        val teardown = TeardownSequence()
        teardown.step("BLE manager") { bleTransport?.stop() }
        teardown.step("Internet manager") { internetManager?.stop() }
        teardown.step("Wi-Fi Direct manager") { wifiDirectManager?.stop() }
        teardown.step("protocol") { protocol?.stop() }
        teardown.firstFailureOrNull()?.let { throw it }
    }

    private fun enableTransportOnEngine(type: String, arguments: Any?) {
        val configJson = arguments as? String
        val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
        val config = currentConfig ?: throw IllegalStateException("Protocol config not initialized")
        val ctx = applicationContext ?: throw IllegalStateException("Plugin not attached")
        when (type.lowercase()) {
            "internet" -> {
                val json = configJson?.let { JSONObject(it) } ?: JSONObject()
                val manager = ensureInternetManager(proto, config)
                if (manager.state == TransportState.RUNNING ||
                    manager.state == TransportState.STARTING
                ) {
                    manager.stop()
                }
                InternetTransportBootstrap.configureAndStart(manager, json)
            }
            "wifidirect", "wifi_direct" -> {
                if (wifiDirectManager == null) {
                    wifiDirectManager = WifiDirectManager(
                        ctx,
                        proto,
                        config.profile,
                        appId = config.appId,
                    ) { level, message, context ->
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
                val manager = wifiDirectManager
                    ?: throw IllegalStateException("Failed to create Wi-Fi Direct manager")
                wireTransportCallbacks(proto)
                val autoAccept = configJson?.let {
                    val json = JSONObject(it)
                    if (json.has("autoAccept") && !json.isNull("autoAccept")) {
                        json.getBoolean("autoAccept")
                    } else {
                        null
                    }
                }
                if (manager.state == TransportState.RUNNING) {
                    manager.stop()
                }
                manager.formGroups =
                    WifiDirectGroupFormation.formGroupsAfterEnable(manager.formGroups, autoAccept)
                manager.start()
            }
            "ble" -> {
                if (bleTransport == null) {
                    bleTransport = BleTransportFacade(
                        ctx,
                        proto,
                        config.profile,
                        config.appId,
                    ) { level, message, context ->
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
                val manager = bleTransport
                    ?: throw IllegalStateException("Failed to create BLE manager")
                wireTransportCallbacks(proto)
                if (manager.state != TransportState.RUNNING) {
                    manager.start()
                }
            }
            "reticulum", "nostr" -> throw UnsupportedOperationException(
                "Transport $type is not compiled into the Flutter plugin yet",
            )
            else -> throw IllegalArgumentException("Unsupported transport type: $type")
        }
    }

    private fun ensureInternetManager(
        proto: OfflineProtocol,
        config: ProtocolConfig,
    ): InternetManager {
        internetManager?.let { return it }
        val ctx = applicationContext ?: throw IllegalStateException("Plugin not attached")
        return InternetTransportBootstrap.createManager(
            ctx,
            proto,
            config.profile,
            config.appId,
        ) { level, message, context ->
            Log.println(
                when (level) {
                    "error" -> Log.ERROR
                    "warning" -> Log.WARN
                    else -> Log.INFO
                },
                TAG,
                "$message ${context ?: emptyMap()}",
            )
        }.also { manager ->
            InternetTransportBootstrap.attachEmitters(manager, ::emitEventJson)
            internetManager = manager
        }
    }

    private fun sendMessageOnEngine(arguments: Any?): String {
        val map = arguments as? Map<*, *>
            ?: throw IllegalArgumentException("sendMessage expects a map")
        val recipient = map["recipient"] as? String
            ?: throw IllegalArgumentException("sendMessage requires recipient")
        val content = map["content"] as? String
            ?: throw IllegalArgumentException("sendMessage requires content")
        val priorityValue = (map["priority"] as? Number)?.toInt() ?: 1
        val replyTo = map["replyToMsg"] as? String
        val msgPriority = when (priorityValue) {
            0 -> MessagePriority.LOW
            1 -> MessagePriority.MEDIUM
            2 -> MessagePriority.HIGH
            3 -> MessagePriority.CRITICAL
            else -> MessagePriority.MEDIUM
        }
        val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
        return proto.sendMessage(recipient, content, msgPriority, replyTo)
    }

    private fun tearDownProtocol() {
        val teardown = TeardownSequence()
        teardown.step("BLE manager") { bleTransport?.stop() }
        bleTransport = null
        teardown.step("Internet manager") { internetManager?.stop() }
        internetManager = null
        teardown.step("Wi-Fi Direct manager") { wifiDirectManager?.stop() }
        wifiDirectManager = null
        try {
            protocol?.stop()
        } catch (e: Exception) {
            Log.w(TAG, "stop during teardown", e)
        }
        try {
            protocol?.destroy()
        } catch (e: Exception) {
            Log.w(TAG, "destroy during teardown", e)
        }
        protocol = null
        currentConfig = null
        synchronized(eventSeqLock) {
            eventSeq = 0
            eventReplay.clear()
        }
    }

    companion object {
        private const val TAG = "OfflineProtocolPlugin"
        private const val CHANNEL_NAME = "com.offlineprotocol/offline_protocol"
        private const val EVENTS_CHANNEL_NAME = "com.offlineprotocol/offline_protocol_events"
        private const val ENGINE_IDLE_SECONDS = 30L

        /** Error code for a native library that is missing or does not match the bindings. */
        private const val NATIVE_LIBRARY_ERROR = "NATIVE_LIBRARY_UNAVAILABLE"

        /**
         * The engine thread for every plugin instance in the process, so calls
         * from all engines run in submission order (see the class comment).
         * It exits when idle and is recreated by the next submission.
         */
        private val engineExecutor: ThreadPoolExecutor =
            ThreadPoolExecutor(1, 1, ENGINE_IDLE_SECONDS, TimeUnit.SECONDS, LinkedBlockingQueue()) { runnable ->
                Thread(runnable, "OfflineProtocolEngine").apply { isDaemon = true }
            }.apply { allowCoreThreadTimeOut(true) }
    }
}
