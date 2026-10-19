package com.offlineprotocol.flutter

import android.os.Handler
import android.os.Looper
import android.util.Log
import com.offlineprotocol.AppContainerProtocolStateStorage
import com.offlineprotocol.InternetManager
import com.offlineprotocol.InternetTransportBootstrap
import com.offlineprotocol.InitialRuntimeConfigApplier
import com.offlineprotocol.MlsSecureStorage
import com.offlineprotocol.ProtocolConfigParser
import com.offlineprotocol.StorageNamespace
import com.offlineprotocol.mapProtocolBridgeError
import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.MethodChannel.MethodCallHandler
import io.flutter.plugin.common.MethodChannel.Result
import org.json.JSONObject
import uniffi.offline_protocol.EventCallback
import uniffi.offline_protocol.MessagePriority
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.ProtocolConfig
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Flutter host for the Offline Protocol engine.
 *
 * All UniFFI calls run on a dedicated engine thread ([engineExecutor]); results
 * and [EventChannel] deliveries are posted on the main looper ([mainHandler]).
 * See bridge contract K2.
 */
class OfflineProtocolPlugin : FlutterPlugin, MethodCallHandler, EventChannel.StreamHandler {
    private lateinit var channel: MethodChannel
    private var eventsChannel: EventChannel? = null
    private var applicationContext: android.content.Context? = null
    private var protocol: OfflineProtocol? = null
    private var currentConfig: ProtocolConfig? = null
    private var internetManager: InternetManager? = null

    private var engineExecutor: ExecutorService = newEngineExecutor()
    private val mainHandler = Handler(Looper.getMainLooper())
    private val attached = AtomicBoolean(false)

    @Volatile
    private var eventSink: EventChannel.EventSink? = null
    private val eventSeqLock = Any()
    private var eventSeq = 0

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        if (engineExecutor.isShutdown) {
            engineExecutor = newEngineExecutor()
        }
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
        eventsChannel?.setStreamHandler(null)
        eventsChannel = null
        eventSink = null
        if (::channel.isInitialized) {
            channel.setMethodCallHandler(null)
        }
        val executor = engineExecutor
        try {
            if (!executor.isShutdown) {
                executor.submit {
                    tearDownProtocol()
                    applicationContext = null
                }.get(TEARDOWN_TIMEOUT_SEC, TimeUnit.SECONDS)
            }
        } catch (e: Exception) {
            Log.w(TAG, "Engine teardown timed out or failed during detach", e)
            applicationContext = null
        } finally {
            if (!executor.isShutdown) {
                executor.shutdown()
            }
        }
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
        val executor = engineExecutor
        if (executor.isShutdown) {
            postDetachedError(result)
            return
        }
        try {
            executor.execute {
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
                            enableInternetTransportOnEngine(call.arguments)
                            null
                        }
                        else -> {
                            mainHandler.post { result.notImplemented() }
                            return@execute
                        }
                    }
                    mainHandler.post { result.success(payload) }
                } catch (e: Exception) {
                    Log.e(TAG, "${call.method} failed", e)
                    mainHandler.post {
                        val mapped = mapProtocolBridgeError(e)
                        if (mapped != null) {
                            result.error(mapped.code, mapped.message, null)
                        } else {
                            result.error("ERROR", e.message, null)
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

    private fun createOnEngine(arguments: Any?) {
        val configJson = arguments as? String
            ?: throw IllegalArgumentException("create expects a JSON string")
        tearDownProtocol()
        val parsed = ProtocolConfigParser.parse(configJson)
        val proto = OfflineProtocol(parsed.coreConfig)
        proto.setEventCallback(object : EventCallback {
            override fun onEvent(eventJson: String) {
                try {
                    emitEventJson(eventJson)
                } catch (e: Exception) {
                    Log.e(TAG, "Core event not handed to Dart", e)
                }
            }
        })
        InitialRuntimeConfigApplier.apply(proto, parsed.rawJson)
        protocol = proto
        currentConfig = parsed.coreConfig
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
        // Internet auto-enable runs in Dart after start() (same layer as React Native JS).
    }

    private fun stopOnEngine() {
        try {
            internetManager?.stop()
        } catch (e: Exception) {
            Log.w(TAG, "internet stop during protocol stop", e)
        }
        protocol?.stop()
    }

    private fun enableInternetTransportOnEngine(arguments: Any?) {
        val configJson = arguments as? String
            ?: throw IllegalArgumentException("enableInternetTransport expects a JSON string")
        val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
        val config = currentConfig ?: throw IllegalStateException("Protocol config not initialized")
        val json = JSONObject(configJson)
        val manager = ensureInternetManager(proto, config)
        InternetTransportBootstrap.configureAndStart(manager, json)
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
        try {
            internetManager?.stop()
        } catch (e: Exception) {
            Log.w(TAG, "internet stop during teardown", e)
        }
        internetManager = null
        synchronized(eventSeqLock) {
            eventSeq = 0
        }
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
    }

    companion object {
        private const val TAG = "OfflineProtocolPlugin"
        private const val CHANNEL_NAME = "com.offlineprotocol/offline_protocol"
        private const val EVENTS_CHANNEL_NAME = "com.offlineprotocol/offline_protocol_events"
        private const val TEARDOWN_TIMEOUT_SEC = 10L

        private fun newEngineExecutor(): ExecutorService =
            Executors.newSingleThreadExecutor { runnable ->
                Thread(runnable, "OfflineProtocolEngine").apply { isDaemon = true }
            }
    }
}
