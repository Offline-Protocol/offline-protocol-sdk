
package com.offlineprotocol.flutter

import android.bluetooth.BluetoothAdapter
import android.content.Intent
import android.util.Base64
import android.util.Log
import com.offlineprotocol.InternetTransportBootstrap
import com.offlineprotocol.ProtocolEngineHost
import com.offlineprotocol.TeardownSequence
import com.offlineprotocol.TransportBridge
import com.offlineprotocol.TransportState
import com.offlineprotocol.InitialRuntimeConfigApplier
import org.json.JSONArray
import org.json.JSONObject
import uniffi.offline_protocol.*
import kotlin.math.max
import kotlin.math.min

/**
 * Per-method bodies for [FlutterEngineChannel]. Arguments are the Dart-side
 * maps from `mesh_sdk.dart` (named keys, camelCase).
 */
internal object EngineMethodHandlers {
    private const val TAG = "EngineMethodHandlers"

    @JvmStatic
    fun acceptConnectionRequest(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.acceptConnectionRequest(host, env.context, arguments)
    }

    @JvmStatic
    fun bleGetDiagnostics(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.bleGetDiagnostics(host, env.context, arguments)
    }

    @JvmStatic
    fun bleGetPeerCount(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.bleGetPeerCount(host, env.context, arguments)
    }

    @JvmStatic
    fun blockUser(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.blockUser(host, env.context, arguments)
    }

    @JvmStatic
    fun cancelConnectionRequest(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.cancelConnectionRequest(host, env.context, arguments)
    }

    @JvmStatic
    fun cancelFileTransfer(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.cancelFileTransfer(host, env.context, arguments)
    }

    @JvmStatic
    fun checkInternetPresence(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.checkInternetPresence(host, env.context, arguments)
    }

    @JvmStatic
    fun create(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val configJson = arguments as? String
            ?: throw IllegalArgumentException("create expects a JSON string")
        env.createOnEngine(configJson)
        return null
    }

    @JvmStatic
    fun createInvite(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.createInvite(host, env.context, arguments)
    }

    @JvmStatic
    fun dataAttachmentHash(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataAttachmentHash expects a map")
        return host.dataAttachmentHash(FlutterBridgeArgs.requireString(map, "bytesBase64"))
    }

    @JvmStatic
    fun dataCounterIncrement(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataCounterIncrement expects a map")
        host.dataCounterIncrement(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"), FlutterBridgeArgs.requireDouble(map, "amount"))
        return null
    }

    @JvmStatic
    fun dataCounterValue(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataCounterValue expects a map")
        return host.dataCounterValue(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"))
    }

    @JvmStatic
    fun dataCreateDoc(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataCreateDoc expects a map")
        host.dataCreateDoc(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"))
        return null
    }

    @JvmStatic
    fun dataDeclineAttachment(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataDeclineAttachment expects a map")
        host.dataDeclineAttachment(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "peerId"), FlutterBridgeArgs.requireString(map, "hash"))
        return null
    }

    @JvmStatic
    fun dataDeleteDoc(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataDeleteDoc expects a map")
        host.dataDeleteDoc(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"))
        return null
    }

    @JvmStatic
    fun dataDocJson(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataDocJson expects a map")
        return host.dataDocJson(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"))
    }

    @JvmStatic
    fun dataDocSize(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataDocSize expects a map")
        return host.dataDocSize(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"))
    }

    @JvmStatic
    fun dataExportRaw(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataExportRaw expects a map")
        return host.dataExportRaw(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"))
    }

    @JvmStatic
    fun dataFetchAttachment(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataFetchAttachment expects a map")
        host.dataFetchAttachment(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "hash"))
        return null
    }

    @JvmStatic
    fun dataFetchAttachmentFrom(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataFetchAttachmentFrom expects a map")
        host.dataFetchAttachmentFrom(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "peerId"), FlutterBridgeArgs.requireString(map, "hash"))
        return null
    }

    @JvmStatic
    fun dataFlush(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataFlush expects a map")
        host.dataFlush(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"))
        return null
    }

    @JvmStatic
    fun dataFlushAll(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataFlushAll expects a map")
        host.dataFlushAll()
        return null
    }

    @JvmStatic
    fun dataListDelete(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataListDelete expects a map")
        host.dataListDelete(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"), FlutterBridgeArgs.requireDouble(map, "index"), FlutterBridgeArgs.requireDouble(map, "count"))
        return null
    }

    @JvmStatic
    fun dataListDocs(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataListDocs expects a map")
        return host.dataListDocs(FlutterBridgeArgs.requireString(map, "spaceId"))
    }

    @JvmStatic
    fun dataListLen(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataListLen expects a map")
        return host.dataListLen(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"))
    }

    @JvmStatic
    fun dataListPush(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataListPush expects a map")
        host.dataListPush(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"), FlutterBridgeArgs.requireString(map, "valueJson"))
        return null
    }

    @JvmStatic
    fun dataListSpaces(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataListSpaces expects a map")
        return host.dataListSpaces()
    }

    @JvmStatic
    fun dataMapDelete(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataMapDelete expects a map")
        host.dataMapDelete(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"), FlutterBridgeArgs.requireString(map, "key"))
        return null
    }

    @JvmStatic
    fun dataMapGetJson(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataMapGetJson expects a map")
        return host.dataMapGetJson(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"), FlutterBridgeArgs.requireString(map, "key"))
    }

    @JvmStatic
    fun dataMapSet(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataMapSet expects a map")
        host.dataMapSet(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"), FlutterBridgeArgs.requireString(map, "key"), FlutterBridgeArgs.requireString(map, "valueJson"))
        return null
    }

    @JvmStatic
    fun dataProvideAttachment(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataProvideAttachment expects a map")
        host.dataProvideAttachment(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "peerId"), FlutterBridgeArgs.requireString(map, "hash"), FlutterBridgeArgs.requireString(map, "bytesBase64"))
        return null
    }

    @JvmStatic
    fun dataRemoveDoc(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataRemoveDoc expects a map")
        host.dataRemoveDoc(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"))
        return null
    }

    @JvmStatic
    fun dataRemoveSpace(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataRemoveSpace expects a map")
        host.dataRemoveSpace(FlutterBridgeArgs.requireString(map, "spaceId"))
        return null
    }

    @JvmStatic
    fun dataSetInterest(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataSetInterest expects a map")
        host.dataSetInterest(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireStringList(map, "patterns"))
        return null
    }

    @JvmStatic
    fun dataTextDelete(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataTextDelete expects a map")
        host.dataTextDelete(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"), FlutterBridgeArgs.requireDouble(map, "position"), FlutterBridgeArgs.requireDouble(map, "count"))
        return null
    }

    @JvmStatic
    fun dataTextInsert(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataTextInsert expects a map")
        host.dataTextInsert(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"), FlutterBridgeArgs.requireDouble(map, "position"), FlutterBridgeArgs.requireString(map, "text"))
        return null
    }

    @JvmStatic
    fun dataTextValue(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataTextValue expects a map")
        return host.dataTextValue(FlutterBridgeArgs.requireString(map, "spaceId"), FlutterBridgeArgs.requireString(map, "docId"), FlutterBridgeArgs.requireString(map, "collection"))
    }

    @JvmStatic
    fun dataWipeAll(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("dataWipeAll expects a map")
        host.dataWipeAll()
        return null
    }

    @JvmStatic
    fun deriveAddress(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.deriveAddress(host, env.context, arguments)
    }

    @JvmStatic
    fun deriveUserIdFromPublicKey(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.deriveUserIdFromPublicKey(host, env.context, arguments)
    }

    @JvmStatic
    fun destroy(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    env.tearDownProtocol()
        return null
    }

    @JvmStatic
    fun disableTelemetry(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    host.disableTelemetry()
        return null
    }

    @JvmStatic
    fun disableTransport(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.disableTransport(host, env.context, arguments)
    }

    @JvmStatic
    fun discoverServices(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.discoverServices(host, env.context, arguments)
    }

    @JvmStatic
    fun emitTestEvent(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    host.requireProtocolBridge().emitTestEvent()
        return null
    }

    @JvmStatic
    fun enableTelemetry(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    @Suppress("UNCHECKED_CAST")
        val config = arguments as? Map<String, Any?>
        host.enableTelemetry(config, env.seedAppState())
        return null
    }

    @JvmStatic
    fun enableTransport(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *>
            ?: throw IllegalArgumentException("enableTransport expects a map")
        val type = FlutterBridgeArgs.requireString(map, "type")
        val configJson = map["config"] as? String
        val json = configJson?.let { JSONObject(it) }
        when (type.lowercase()) {
            "internet" -> {
                val manager = ensureInternetManager(host, env)
                if (manager.state == TransportState.RUNNING || manager.state == TransportState.STARTING) {
                    manager.stop()
                }
                InternetTransportBootstrap.configureAndStart(manager, json ?: JSONObject())
            }
            else -> host.enableTransport(env.context, type, json, env.internetHandlers)
        }
        return null
    }

    @JvmStatic
    fun endTelemetrySession(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    host.endTelemetrySession()
        return null
    }

    @JvmStatic
    fun eraseCustody(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.eraseCustody(host, env.context, arguments)
    }

    @JvmStatic
    fun establishSecureSession(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.establishSecureSession(host, env.context, arguments)
    }

    @JvmStatic
    fun finalizeFile(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.finalizeFile(host, env.context, arguments)
    }

    @JvmStatic
    fun flushTelemetry(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    host.flushTelemetry()
        return null
    }

    @JvmStatic
    fun forceTransport(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.forceTransport(host, env.context, arguments)
    }

    @JvmStatic
    fun forwardMessage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.forwardMessage(host, env.context, arguments)
    }

    @JvmStatic
    fun getActiveTransports(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getActiveTransports(host, env.context, arguments)
    }

    @JvmStatic
    fun getBatteryLevel(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getBatteryLevel(host, env.context, arguments)
    }

    @JvmStatic
    fun getBlockedUsers(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getBlockedUsers(host, env.context, arguments)
    }

    @JvmStatic
    fun getCustodyStats(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getCustodyStats(host, env.context, arguments)
    }

    @JvmStatic
    fun getDedupStats(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getDedupStats(host, env.context, arguments)
    }

    @JvmStatic
    fun getDeliverySuccessRate(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getDeliverySuccessRate(host, env.context, arguments)
    }

    @JvmStatic
    fun getDorsConfig(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getDorsConfig(host, env.context, arguments)
    }

    @JvmStatic
    fun getEstablishmentState(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getEstablishmentState(host, env.context, arguments)
    }

    @JvmStatic
    fun getFileProgress(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getFileProgress(host, env.context, arguments)
    }

    @JvmStatic
    fun getIdentityPublicKey(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getIdentityPublicKey(host, env.context, arguments)
    }

    @JvmStatic
    fun getIsCharging(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getIsCharging(host, env.context, arguments)
    }

    @JvmStatic
    fun getMedianHops(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getMedianHops(host, env.context, arguments)
    }

    @JvmStatic
    fun getMedianLatency(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getMedianLatency(host, env.context, arguments)
    }

    @JvmStatic
    fun getMeshRelayStats(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getMeshRelayStats(host, env.context, arguments)
    }

    @JvmStatic
    fun getMeshRelayTunables(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getMeshRelayTunables(host, env.context, arguments)
    }

    @JvmStatic
    fun getMessageStats(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getMessageStats(host, env.context, arguments)
    }

    @JvmStatic
    fun getPendingAckCount(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getPendingAckCount(host, env.context, arguments)
    }

    @JvmStatic
    fun getRelayConfig(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getRelayConfig(host, env.context, arguments)
    }

    @JvmStatic
    fun getRelayPriority(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getRelayPriority(host, env.context, arguments)
    }

    @JvmStatic
    fun getRetryQueueSize(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getRetryQueueSize(host, env.context, arguments)
    }

    @JvmStatic
    fun getState(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getState(host, env.context, arguments)
    }

    @JvmStatic
    fun getTopology(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getTopology(host, env.context, arguments)
    }

    @JvmStatic
    fun getTransportMetrics(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.getTransportMetrics(host, env.context, arguments)
    }

    @JvmStatic
    fun hasPendingKeyPackage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.hasPendingKeyPackage(host, env.context, arguments)
    }

    @JvmStatic
    fun initializeMlsWithSecureStorage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    host.initializeMlsWithSecureStorage(env.context)
        return null
    }

    @JvmStatic
    fun internetConfirmSent(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.internetConfirmSent(host, env.context, arguments)
    }

    @JvmStatic
    fun internetForceReconnect(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.internetForceReconnect(host, env.context, arguments)
    }

    @JvmStatic
    fun internetGetNextMessage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.internetGetNextMessage(host, env.context, arguments)
    }

    @JvmStatic
    fun internetIsReady(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.internetIsReady(host, env.context, arguments)
    }

    @JvmStatic
    fun internetIsSuperseded(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.internetIsSuperseded(host, env.context, arguments)
    }

    @JvmStatic
    fun internetMessageReceived(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.internetMessageReceived(host, env.context, arguments)
    }

    @JvmStatic
    fun internetSendFailed(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.internetSendFailed(host, env.context, arguments)
    }

    @JvmStatic
    fun internetSendRawCommand(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.internetSendRawCommand(host, env.context, arguments)
    }

    @JvmStatic
    fun internetStatusChanged(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.internetStatusChanged(host, env.context, arguments)
    }

    @JvmStatic
    fun isBluetoothEnabled(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.isBluetoothEnabled(host, env.context, arguments)
    }

    @JvmStatic
    fun isMlsInitialized(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.isMlsInitialized(host, env.context, arguments)
    }

    @JvmStatic
    fun isRelay(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.isRelay(host, env.context, arguments)
    }

    @JvmStatic
    fun isUserBlocked(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.isUserBlocked(host, env.context, arguments)
    }

    @JvmStatic
    fun localAddress(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return host.protocol?.localAddress()
    }

    @JvmStatic
    fun meshCreateGroup(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshCreateGroup expects a map")
        return host.meshCreateGroup(FlutterBridgeArgs.requireString(map, "name"))
    }

    @JvmStatic
    fun meshForwardMessageToGroup(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshForwardMessageToGroup expects a map")
        return host.meshForwardMessageToGroup(FlutterBridgeArgs.requireString(map, "originalMessageJson"), FlutterBridgeArgs.requireString(map, "groupId"), map["priority"] as? String)
    }

    @JvmStatic
    fun meshGetGroupInfo(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshGetGroupInfo expects a map")
        return host.meshGetGroupInfo(FlutterBridgeArgs.requireString(map, "groupId"))
    }

    @JvmStatic
    fun meshGetGroupRoles(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshGetGroupRoles expects a map")
        return host.meshGetGroupRoles(FlutterBridgeArgs.requireString(map, "groupId"))
    }

    @JvmStatic
    fun meshGetMemberRole(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshGetMemberRole expects a map")
        return host.meshGetMemberRole(FlutterBridgeArgs.requireString(map, "groupId"), FlutterBridgeArgs.requireString(map, "userId"))
    }

    @JvmStatic
    fun meshGroupRelaySyncState(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshGroupRelaySyncState expects a map")
        return host.meshGroupRelaySyncState(FlutterBridgeArgs.requireString(map, "groupId"))
    }

    @JvmStatic
    fun meshGroupRichReadiness(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshGroupRichReadiness expects a map")
        return host.meshGroupRichReadiness(FlutterBridgeArgs.requireString(map, "groupId"))
    }

    @JvmStatic
    fun meshInviteToGroup(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshInviteToGroup expects a map")
        host.meshInviteToGroup(FlutterBridgeArgs.requireString(map, "groupId"), FlutterBridgeArgs.requireString(map, "inviteeUserId"))
        return null
    }

    @JvmStatic
    fun meshLeaveGroup(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshLeaveGroup expects a map")
        host.meshLeaveGroup(FlutterBridgeArgs.requireString(map, "groupId"))
        return null
    }

    @JvmStatic
    fun meshListGroups(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshListGroups expects a map")
        return host.meshListGroups()
    }

    @JvmStatic
    fun meshRemoveFromGroup(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshRemoveFromGroup expects a map")
        host.meshRemoveFromGroup(FlutterBridgeArgs.requireString(map, "groupId"), FlutterBridgeArgs.requireString(map, "memberId"))
        return null
    }

    @JvmStatic
    fun meshRenameGroup(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshRenameGroup expects a map")
        host.meshRenameGroup(FlutterBridgeArgs.requireString(map, "groupId"), FlutterBridgeArgs.requireString(map, "newName"))
        return null
    }

    @JvmStatic
    fun meshRequestGroupRelayRegistration(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshRequestGroupRelayRegistration expects a map")
        return host.meshRequestGroupRelayRegistration(FlutterBridgeArgs.requireString(map, "groupId"))
    }

    @JvmStatic
    fun meshSendGroupMessage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshSendGroupMessage expects a map")
        return host.meshSendGroupMessage(FlutterBridgeArgs.requireString(map, "groupId"), FlutterBridgeArgs.requireString(map, "content"), map["priority"] as? String, map["replyToMsg"] as? String)
    }

    @JvmStatic
    fun meshSetMemberRole(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("meshSetMemberRole expects a map")
        host.meshSetMemberRole(FlutterBridgeArgs.requireString(map, "groupId"), FlutterBridgeArgs.requireString(map, "userId"), FlutterBridgeArgs.requireString(map, "role"))
        return null
    }

    @JvmStatic
    fun mlsCreateSession(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsCreateSession(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsDecrypt(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsDecrypt(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsDecryptFromUser(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsDecryptFromUser(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsDeleteSession(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsDeleteSession(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsEncryptForUser(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsEncryptForUser(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsGenerateKeyPackage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsGenerateKeyPackage(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsGetOrCreateKeyPackage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsGetOrCreateKeyPackage(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsGetPendingKeyPackages(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsGetPendingKeyPackages(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsHasSession(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsHasSession(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsImportKeyPackage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsImportKeyPackage(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsJoinSession(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsJoinSession(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsListSessions(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsListSessions(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsMarkKeyPackageSynced(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsMarkKeyPackageSynced(host, env.context, arguments)
    }

    @JvmStatic
    fun mlsProcessWelcome(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.mlsProcessWelcome(host, env.context, arguments)
    }

    @JvmStatic
    fun parseInvite(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.parseInvite(host, env.context, arguments)
    }

    @JvmStatic
    fun pause(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    host.pause()
        return null
    }

    @JvmStatic
    fun processFileChunk(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.processFileChunk(host, env.context, arguments)
    }

    @JvmStatic
    fun receiveMessage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return host.receiveMessage()
    }

    @JvmStatic
    fun registerService(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.registerService(host, env.context, arguments)
    }

    @JvmStatic
    fun rejectConnectionRequest(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.rejectConnectionRequest(host, env.context, arguments)
    }

    @JvmStatic
    fun rekeySession(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.rekeySession(host, env.context, arguments)
    }

    @JvmStatic
    fun releaseTransportLock(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.releaseTransportLock(host, env.context, arguments)
    }

    @JvmStatic
    fun replayEvents(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *>
            ?: throw IllegalArgumentException("replayEvents expects a map")
        val fromSeq = FlutterBridgeArgs.requireInt(map, "fromSeq")
        val toSeq = FlutterBridgeArgs.requireInt(map, "toSeq")
        return env.replayEvents(fromSeq, toSeq)
    }

    @JvmStatic
    fun requestEnableBluetooth(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val activity = env.activityProvider()
            ?: throw IllegalStateException("No activity available to show Bluetooth enable dialog")
        return com.offlineprotocol.ProtocolEngineHostBridge.requestEnableBluetooth(host, activity, arguments)
    }

    @JvmStatic
    fun resolveUsername(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.resolveUsername(host, env.context, arguments)
    }

    @JvmStatic
    fun respondToServiceRequest(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.respondToServiceRequest(host, env.context, arguments)
    }

    @JvmStatic
    fun resume(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    host.resume()
        return null
    }

    @JvmStatic
    fun sendConnectionRequest(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.sendConnectionRequest(host, env.context, arguments)
    }

    @JvmStatic
    fun sendFileFromPath(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.sendFileFromPath(host, env.context, arguments)
    }

    @JvmStatic
    fun sendMedia(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.sendMedia(host, env.context, arguments)
    }

    @JvmStatic
    fun sendMediaRich(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.sendMediaRich(host, env.context, arguments)
    }

    @JvmStatic
    fun sendMessage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val map = arguments as? Map<*, *>
            ?: throw IllegalArgumentException("sendMessage expects a map")
        val recipient = FlutterBridgeArgs.requireString(map, "recipient")
        val content = FlutterBridgeArgs.requireString(map, "content")
        val priority = FlutterBridgeArgs.optionalInt(map, "priority") ?: 1
        val replyTo = map["replyToMsg"] as? String
        return host.sendMessage(recipient, content, priority, replyTo)
    }

    @JvmStatic
    fun sendMessageRich(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.sendMessageRich(host, env.context, arguments)
    }

    @JvmStatic
    fun sendPresenceUpdate(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.sendPresenceUpdate(host, env.context, arguments)
    }

    @JvmStatic
    fun sendReadReceipt(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.sendReadReceipt(host, env.context, arguments)
    }

    @JvmStatic
    fun sendServiceRequest(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.sendServiceRequest(host, env.context, arguments)
    }

    @JvmStatic
    fun sendTypingIndicator(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.sendTypingIndicator(host, env.context, arguments)
    }

    @JvmStatic
    fun setBatteryLevel(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.setBatteryLevel(host, env.context, arguments)
    }

    @JvmStatic
    fun setBatteryState(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.setBatteryState(host, env.context, arguments)
    }

    @JvmStatic
    fun setRelayPriority(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.setRelayPriority(host, env.context, arguments)
    }

    @JvmStatic
    fun setTelemetryEnabled(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val enabled = when (arguments) {
            is Boolean -> arguments
            is Map<*, *> -> FlutterBridgeArgs.requireBoolean(arguments, "enabled")
            else -> throw IllegalArgumentException("setTelemetryEnabled expects boolean or map")
        }
        host.setTelemetryEnabled(enabled)
        return null
    }

    @JvmStatic
    fun shouldEscalateToWifi(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.shouldEscalateToWifi(host, env.context, arguments)
    }

    @JvmStatic
    fun signData(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.signData(host, env.context, arguments)
    }

    @JvmStatic
    fun signDataBase64(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.signDataBase64(host, env.context, arguments)
    }

    @JvmStatic
    fun start(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    env.startOnEngine()
        return null
    }

    @JvmStatic
    fun stop(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    env.stopOnEngine()
        return null
    }

    @JvmStatic
    fun telemetryInstallId(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return host.telemetryInstallId()
    }

    @JvmStatic
    fun telemetryStats(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    val stats = host.telemetryStats() ?: return null
        return host.telemetryStatsMap(stats)
    }

    @JvmStatic
    fun unblockUser(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.unblockUser(host, env.context, arguments)
    }

    @JvmStatic
    fun unregisterService(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.unregisterService(host, env.context, arguments)
    }

    @JvmStatic
    fun updateAckConfig(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.updateAckConfig(host, env.context, arguments)
    }

    @JvmStatic
    fun updateDedupConfig(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.updateDedupConfig(host, env.context, arguments)
    }

    @JvmStatic
    fun updateDorsConfig(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.updateDorsConfig(host, env.context, arguments)
    }

    @JvmStatic
    fun updateRelayConfig(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.updateRelayConfig(host, env.context, arguments)
    }

    @JvmStatic
    fun updateRetryConfig(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.updateRetryConfig(host, env.context, arguments)
    }

    @JvmStatic
    fun verifySignature(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.verifySignature(host, env.context, arguments)
    }

    @JvmStatic
    fun verifySignatureBase64(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.verifySignatureBase64(host, env.context, arguments)
    }

    @JvmStatic
    fun wifiDirectGetNextMessage(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.wifiDirectGetNextMessage(host, env.context, arguments)
    }

    @JvmStatic
    fun wifiDirectMessageReceived(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.wifiDirectMessageReceived(host, env.context, arguments)
    }

    @JvmStatic
    fun wifiDirectPeerConnected(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.wifiDirectPeerConnected(host, env.context, arguments)
    }

    @JvmStatic
    fun wifiDirectPeerDisconnected(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.wifiDirectPeerDisconnected(host, env.context, arguments)
    }

    @JvmStatic
    fun wifiDirectStatusChanged(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.wifiDirectStatusChanged(host, env.context, arguments)
    }

    @JvmStatic
    fun wipePersistedState(
        host: ProtocolEngineHost,
        env: FlutterEngineChannel.Environment,
        arguments: Any?,
    ): Any? {
    return com.offlineprotocol.ProtocolEngineHostBridge.wipePersistedState(host, env.context, arguments)
    }

private fun ensureInternetManager(
    host: ProtocolEngineHost,
    env: FlutterEngineChannel.Environment,
): com.offlineprotocol.InternetManager {
    host.internetManager?.let { return it }
    val config = host.currentConfig
        ?: throw IllegalStateException("Protocol config not initialized")
    val proto = host.protocol
        ?: throw IllegalStateException("Protocol not initialized")
    return InternetTransportBootstrap.createManager(
        env.context,
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
        InternetTransportBootstrap.attachEmitters(manager) { json ->
            env.internetHandlers.serverMessageEmitter(json)
        }
        host.internetManager = manager
    }
}
}
