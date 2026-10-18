package com.offlineprotocol.flutter

import android.os.Handler
import android.os.Looper
import android.util.Log
import com.offlineprotocol.AppContainerProtocolStateStorage
import com.offlineprotocol.MlsSecureStorage
import com.offlineprotocol.ProtocolConfigParser
import com.offlineprotocol.StorageNamespace
import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.MethodChannel.MethodCallHandler
import io.flutter.plugin.common.MethodChannel.Result
import uniffi.offline_protocol.EventCallback
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.ProtocolConfig
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Flutter host for the Offline Protocol engine.
 *
 * All UniFFI calls run on a dedicated engine thread ([engineExecutor]); results
 * are delivered on the main looper ([mainHandler]). See bridge contract K2.
 *
 * Transport managers (ADR 0025) are wired in a follow-up PR.
 */
class OfflineProtocolPlugin : FlutterPlugin, MethodCallHandler {
    private lateinit var channel: MethodChannel
    private var applicationContext: android.content.Context? = null
    private var protocol: OfflineProtocol? = null
    private var currentConfig: ProtocolConfig? = null

    private val engineExecutor = Executors.newSingleThreadExecutor { runnable ->
        Thread(runnable, "OfflineProtocolEngine").apply { isDaemon = true }
    }
    private val mainHandler = Handler(Looper.getMainLooper())

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        applicationContext = binding.applicationContext
        channel = MethodChannel(binding.binaryMessenger, CHANNEL_NAME)
        channel.setMethodCallHandler(this)
    }

    override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        channel.setMethodCallHandler(null)
        applicationContext = null
        try {
            engineExecutor.submit { tearDownProtocol() }.get(TEARDOWN_TIMEOUT_SEC, TimeUnit.SECONDS)
        } catch (e: Exception) {
            Log.w(TAG, "Engine teardown timed out or failed during detach", e)
        } finally {
            engineExecutor.shutdown()
        }
    }

    override fun onMethodCall(call: MethodCall, result: Result) {
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
                    else -> {
                        mainHandler.post { result.notImplemented() }
                        return@execute
                    }
                }
                mainHandler.post { result.success(payload) }
            } catch (e: Exception) {
                Log.e(TAG, "${call.method} failed", e)
                mainHandler.post {
                    result.error("ERROR", e.message, null)
                }
            }
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
                // EventChannel wiring lands in PR2.
            }
        })
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
        protocol?.start() ?: throw IllegalStateException("Protocol not initialized")
    }

    private fun stopOnEngine() {
        protocol?.stop()
    }

    private fun tearDownProtocol() {
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
        private const val TEARDOWN_TIMEOUT_SEC = 10L
    }
}
