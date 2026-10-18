package com.offlineprotocol.flutter

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

/**
 * Flutter host for the Offline Protocol engine.
 *
 * Bridge transport managers live under bindings/react-native/android (ADR 0025)
 * and are wired in a follow-up PR. This plugin runs the core only.
 */
class OfflineProtocolPlugin : FlutterPlugin, MethodCallHandler {
    private lateinit var channel: MethodChannel
    private var applicationContext: android.content.Context? = null
    private var protocol: OfflineProtocol? = null
    private var currentConfig: ProtocolConfig? = null

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        applicationContext = binding.applicationContext
        channel = MethodChannel(binding.binaryMessenger, CHANNEL_NAME)
        channel.setMethodCallHandler(this)
    }

    override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        channel.setMethodCallHandler(null)
        applicationContext = null
        tearDownProtocol()
    }

    override fun onMethodCall(call: MethodCall, result: Result) {
        try {
            when (call.method) {
                "create" -> create(call.arguments, result)
                "initializeMlsWithSecureStorage" -> initializeMls(result)
                "start" -> start(result)
                "stop" -> stop(result)
                "destroy" -> destroy(result)
                "localAddress" -> localAddress(result)
                else -> result.notImplemented()
            }
        } catch (e: Exception) {
            Log.e(TAG, "${call.method} failed", e)
            result.error("ERROR", e.message, null)
        }
    }

    private fun create(arguments: Any?, result: Result) {
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
        result.success(null)
    }

    private fun initializeMls(result: Result) {
        val ctx = applicationContext
            ?: throw IllegalStateException("Plugin not attached")
        val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
        val config = currentConfig ?: throw IllegalStateException("Protocol config not initialized")
        val accountNamespace = StorageNamespace.account(config.appId, config.profile)
        val secureStorage = MlsSecureStorage(ctx, accountNamespace)
        val protocolStateStorage = AppContainerProtocolStateStorage(ctx, accountNamespace)
        proto.initializeMls(secureStorage, protocolStateStorage)
        result.success(null)
    }

    private fun start(result: Result) {
        protocol?.start() ?: throw IllegalStateException("Protocol not initialized")
        result.success(null)
    }

    private fun stop(result: Result) {
        protocol?.stop()
        result.success(null)
    }

    private fun destroy(result: Result) {
        tearDownProtocol()
        result.success(null)
    }

    private fun localAddress(result: Result) {
        val proto = protocol
        if (proto == null) {
            result.success(null)
            return
        }
        result.success(proto.localAddress())
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
    }
}
