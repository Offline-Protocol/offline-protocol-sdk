#!/usr/bin/env python3
"""Generate ProtocolEngineHostBridge.kt (shared Android bridge)."""

from __future__ import annotations

import pathlib
import textwrap

REPO = pathlib.Path(__file__).resolve().parents[1]
OUT = REPO / "bindings/react-native/android/src/main/java/com/offlineprotocol/ProtocolEngineHostBridge.kt"

# Methods EngineMethodHandlers delegates here (must match gen_engine_method_handlers.py).
METHODS = [
    "acceptConnectionRequest",
    "acceptServiceSubscription",
    "bleGetDiagnostics",
    "bleGetPeerCount",
    "blockUser",
    "cancelConnectionRequest",
    "cancelFileTransfer",
    "checkInternetPresence",
    "clearServiceTopic",
    "createInvite",
    "deriveAddress",
    "deriveUserIdFromPublicKey",
    "disableTransport",
    "discoverServices",
    "endServiceSubscription",
    "eraseCustody",
    "establishSecureSession",
    "finalizeFile",
    "forceTransport",
    "forwardMessage",
    "getActiveTransports",
    "getBatteryLevel",
    "getBlockedUsers",
    "getCustodyStats",
    "getDedupStats",
    "getDeliverySuccessRate",
    "getDorsConfig",
    "getEstablishmentState",
    "getFileProgress",
    "getIdentityPublicKey",
    "getIsCharging",
    "getMedianHops",
    "getMedianLatency",
    "getMeshRelayStats",
    "getMeshRelayTunables",
    "getMessageStats",
    "getPendingAckCount",
    "getRelayConfig",
    "getRelayPriority",
    "getRetryQueueSize",
    "getState",
    "getTopology",
    "getTransportMetrics",
    "hasPendingKeyPackage",
    "internetConfirmSent",
    "internetForceReconnect",
    "internetGetNextMessage",
    "internetIsReady",
    "internetIsSuperseded",
    "internetMessageReceived",
    "internetSendFailed",
    "internetSendRawCommand",
    "internetStatusChanged",
    "isBluetoothEnabled",
    "isMlsInitialized",
    "isRelay",
    "isUserBlocked",
    "mlsCreateSession",
    "mlsDecrypt",
    "mlsDecryptFromUser",
    "mlsDeleteSession",
    "mlsEncryptForUser",
    "mlsGenerateKeyPackage",
    "mlsGetOrCreateKeyPackage",
    "mlsGetPendingKeyPackages",
    "mlsHasSession",
    "mlsImportKeyPackage",
    "mlsJoinSession",
    "mlsListSessions",
    "mlsMarkKeyPackageSynced",
    "mlsProcessWelcome",
    "parseInvite",
    "processFileChunk",
    "publishServiceUpdate",
    "registerService",
    "rejectConnectionRequest",
    "rejectServiceSubscription",
    "rekeySession",
    "releaseTransportLock",
    "requestEnableBluetooth",
    "resolveUsername",
    "respondToServiceRequest",
    "resyncServiceSubscription",
    "sendConnectionRequest",
    "sendFileFromPath",
    "sendMedia",
    "sendMediaRich",
    "sendMessageRich",
    "sendPresenceUpdate",
    "sendReadReceipt",
    "sendServiceRequest",
    "sendTypingIndicator",
    "serviceSubscribers",
    "serviceSubscriptions",
    "setBatteryLevel",
    "setBatteryState",
    "setRelayPriority",
    "setServiceSubscriptionPolicy",
    "setServiceTopicCheckpoint",
    "shouldEscalateToWifi",
    "signData",
    "signDataBase64",
    "subscribeService",
    "unblockUser",
    "unregisterService",
    "unsubscribeService",
    "updateAckConfig",
    "updateDedupConfig",
    "updateDorsConfig",
    "updateRelayConfig",
    "updateRetryConfig",
    "verifySignature",
    "verifySignatureBase64",
    "wifiDirectGetNextMessage",
    "wifiDirectMessageReceived",
    "wifiDirectPeerConnected",
    "wifiDirectPeerDisconnected",
    "wifiDirectStatusChanged",
    "wipePersistedState",
]

HEADER = textwrap.dedent(
    """
    package com.offlineprotocol

    import android.bluetooth.BluetoothAdapter
    import android.content.Context
    import android.content.Intent
    import android.util.Base64
    import org.json.JSONArray
    import org.json.JSONObject
    import uniffi.offline_protocol.*
    import kotlin.math.max
    import kotlin.math.min

    /**
     * Engine operations shared by React Native and Flutter hosts. Flutter
     * reaches these through [com.offlineprotocol.flutter.ProtocolEngineHostBridge]
     * adapters; React Native reaches these through [ProtocolEngineHostBridgeReactNative]
     * (regenerate with `tools/gen_rn_protocol_engine_bridge_react_native.py`).
     */
    object ProtocolEngineHostBridge {
        private object Limits {
            const val MIN_BATTERY = InitialRuntimeConfigApplier.Limits.MIN_BATTERY_LEVEL
            const val MAX_BATTERY = InitialRuntimeConfigApplier.Limits.MAX_BATTERY_LEVEL
            const val MIN_HISTORY = InitialRuntimeConfigApplier.Limits.MIN_HISTORY_WINDOW
            const val MAX_HISTORY = InitialRuntimeConfigApplier.Limits.MAX_HISTORY_WINDOW
            const val MS_PER_SEC = 1000L
        }

        fun mapFromAny(arguments: Any?): Map<String, Any?> {
            @Suppress("UNCHECKED_CAST")
            return when (arguments) {
                is Map<*, *> -> arguments.entries.associate { (k, v) -> k.toString() to v }
                else -> throw IllegalArgumentException("Expected map arguments")
            }
        }

        private fun requireString(map: Map<String, Any?>, key: String): String =
            (map[key] as? String)?.takeIf { it.isNotEmpty() }
                ?: throw IllegalArgumentException("Missing '$key'")

        private fun optionalString(map: Map<String, Any?>, key: String): String? =
            (map[key] as? String)?.takeIf { it.isNotEmpty() }

        private fun requireInt(map: Map<String, Any?>, key: String): Int =
            when (val v = map[key]) {
                is Int -> v
                is Number -> v.toInt()
                else -> throw IllegalArgumentException("Missing int '$key'")
            }

        private fun requireDouble(map: Map<String, Any?>, key: String): Double =
            when (val v = map[key]) {
                is Double -> v
                is Number -> v.toDouble()
                else -> throw IllegalArgumentException("Missing number '$key'")
            }

        private fun ubyteList(map: Map<String, Any?>, key: String): List<UByte> {
            val raw = map[key] ?: return emptyList()
            // Flutter's standard codec hands a Dart Uint8List over as a
            // ByteArray; iOS takes the same value as FlutterStandardTypedData.
            if (raw is ByteArray) return raw.map { it.toUByte() }
            if (raw is List<*>) {
                return raw.map {
                    when (it) {
                        is Int -> it.toUByte()
                        is Number -> it.toInt().toUByte()
                        else -> throw IllegalArgumentException("Invalid byte in '$key'")
                    }
                }
            }
            throw IllegalArgumentException("'$key' must be a byte list")
        }

        private fun stringList(map: Map<String, Any?>, key: String): List<String> {
            val raw = map[key] ?: return emptyList()
            if (raw is List<*>) return raw.mapNotNull { it as? String }
            throw IllegalArgumentException("'$key' must be a string list")
        }

        private fun messagePriority(priority: Int): MessagePriority = when (priority) {
            0 -> MessagePriority.LOW
            1 -> MessagePriority.MEDIUM
            2 -> MessagePriority.HIGH
            3 -> MessagePriority.CRITICAL
            else -> MessagePriority.MEDIUM
        }

        private fun parseContentType(value: String): ContentType = when (value.lowercase()) {
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

        private fun parseReplyContext(map: Map<String, Any?>?): ReplyContext? {
            if (map == null) return null
            return ReplyContext(
                sender = map["sender"] as? String ?: "",
                text = map["text"] as? String ?: "",
                timestamp = (map["timestamp"] as? Number)?.toLong(),
                replyMediaLabel = map["reply_media_label"] as? String,
                replyContentType = map["reply_content_type"] as? String,
            )
        }

        private fun parseForwardInfo(map: Map<String, Any?>?): ForwardInfo? {
            if (map == null) return null
            return ForwardInfo(
                originalSender = map["original_sender"] as? String ?: "",
                originalMessageId = map["original_message_id"] as? String ?: "",
                originalTimestamp = (map["original_timestamp"] as? Number)?.toLong() ?: 0L,
                forwardCount = (map["forward_count"] as? Number)?.toInt()?.toUInt() ?: 1u,
            )
        }

        @Suppress("UNCHECKED_CAST")
        private fun parseRichMediaMetadata(map: Map<String, Any?>?): MediaMetadata? {
            if (map == null) return null
            return MediaMetadata(
                mimeType = map["mime_type"] as? String ?: "",
                fileName = map["file_name"] as? String ?: "",
                fileSize = (map["file_size"] as? Number)?.toLong()?.toULong() ?: 0u,
                durationMs = (map["duration_ms"] as? Number)?.toLong()?.toULong(),
                width = (map["width"] as? Number)?.toInt()?.toUInt(),
                height = (map["height"] as? Number)?.toInt()?.toUInt(),
                thumbnailBase64 = map["thumbnail_base64"] as? String,
                mediaId = map["media_id"] as? String,
                downloadUrl = map["download_url"] as? String,
                thumbnailUrl = map["thumbnail_url"] as? String,
                encryptionKey = map["encryption_key"] as? String,
                iv = map["iv"] as? String,
                ciphertextHash = map["ciphertext_hash"] as? String,
                stickerProvider = map["sticker_provider"] as? String,
                stickerRemoteId = map["sticker_remote_id"] as? String,
                stickerKind = map["sticker_kind"] as? String,
            )
        }

        private fun mlsWelcomeFromJson(json: JSONObject): MlsWelcomeMessage {
            val welcomeDataArray = json.optJSONArray("welcomeData") ?: JSONArray()
            val welcomeData = mutableListOf<UByte>()
            for (i in 0 until welcomeDataArray.length()) {
                welcomeData.add(welcomeDataArray.getInt(i).toUByte())
            }
            return MlsWelcomeMessage(
                groupId = json.optString("groupId"),
                welcomeData = welcomeData,
                inviterId = json.optString("inviterId"),
                groupName = json.optNullableString("groupName"),
                timestampMs = json.optLong("timestampMs", 0).toULong(),
            )
        }

        private fun mlsEncryptedFromJson(json: JSONObject): MlsEncryptedMessage {
            val ciphertextArray = json.optJSONArray("ciphertext") ?: JSONArray()
            val ciphertext = mutableListOf<UByte>()
            for (i in 0 until ciphertextArray.length()) {
                ciphertext.add(ciphertextArray.getInt(i).toUByte())
            }
            return MlsEncryptedMessage(
                groupId = json.optString("groupId"),
                messageType = json.optString("messageType", "Application"),
                epoch = json.optLong("epoch", 0).toULong(),
                ciphertext = ciphertext,
                senderId = json.optString("senderId"),
                timestampMs = json.optLong("timestampMs", 0).toULong(),
            )
        }

        private fun mlsGroupInfoMap(info: MlsGroupInfo): Map<String, Any?> = mapOf(
            "groupId" to info.groupId,
            "name" to info.name,
            "members" to info.members,
            "epoch" to info.epoch.toDouble(),
            "isSession" to info.isSession,
            "createdAtMs" to info.createdAtMs.toDouble(),
            "lastActivityMs" to info.lastActivityMs.toDouble(),
        )

        private fun mlsWelcomeMap(welcome: MlsWelcomeMessage): Map<String, Any?> = mapOf(
            "groupId" to welcome.groupId,
            "welcomeData" to welcome.welcomeData.map { it.toInt() },
            "inviterId" to welcome.inviterId,
            "groupName" to welcome.groupName,
            "timestampMs" to welcome.timestampMs.toDouble(),
        )

        private fun mlsEncryptedMap(encrypted: MlsEncryptedMessage): Map<String, Any?> = mapOf(
            "groupId" to encrypted.groupId,
            "messageType" to encrypted.messageType,
            "epoch" to encrypted.epoch.toDouble(),
            "ciphertext" to encrypted.ciphertext.map { it.toInt() },
            "senderId" to encrypted.senderId,
            "timestampMs" to encrypted.timestampMs.toDouble(),
        )

        private fun keyPackageMap(bundle: MlsKeyPackageBundle): Map<String, Any?> = mapOf(
            "packageId" to bundle.packageId,
            "userId" to bundle.userId,
            "keyPackageData" to bundle.keyPackageData.map { byte -> byte.toInt() },
            "createdAtMs" to bundle.createdAtMs.toDouble(),
            "expiresAtMs" to bundle.expiresAtMs.toDouble(),
            "synced" to bundle.synced,
        )

        private fun JSONObject.optNullableString(key: String): String? =
            if (has(key) && !isNull(key)) optString(key) else null

        private fun configJsonString(arguments: Any?): String = when (arguments) {
            is String -> arguments
            else -> requireString(mapFromAny(arguments), "configJson")
        }

        private fun JSONObject.optIntCompat(vararg keys: String): Int? {
            for (key in keys) {
                if (has(key) && !isNull(key)) return optDouble(key).toInt()
            }
            return null
        }

        private fun JSONObject.optBooleanCompat(vararg keys: String): Boolean? {
            for (key in keys) {
                if (has(key) && !isNull(key)) return getBoolean(key)
            }
            return null
        }
    """
)

# Per-method Kotlin bodies (host + context + map args). `ctx` is Flutter-only; RN passes null.
IMPL: dict[str, str] = {}

from bridge_impl_supplement import SUPPLEMENT_IMPL  # noqa: E402

IMPL.update(SUPPLEMENT_IMPL)

IMPL["forwardMessage"] = """
    fun forwardMessage(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val proto = host.requireProtocolBridge()
        val id = proto.forwardMessage(
            requireString(map, "originalMessageJson"),
            requireString(map, "newRecipient"),
            messagePriority(requireInt(map, "priority")),
        )
        return id
    }
"""

IMPL["sendMessageRich"] = """
    fun sendMessageRich(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        @Suppress("UNCHECKED_CAST")
        val optionsMap = map["options"] as? Map<String, Any?>
        val msgPriority = messagePriority(requireInt(map, "priority"))
        val sendOptions = SendMessageOptions(
            priority = msgPriority,
            replyToMsg = optionalString(map, "replyToMsg"),
            contentType = optionsMap?.get("content_type")?.let { parseContentType(it as String) },
            replyContext = parseReplyContext(optionsMap?.get("reply_context") as? Map<String, Any?>),
            mediaMetadata = parseRichMediaMetadata(optionsMap?.get("media_metadata") as? Map<String, Any?>),
            forwardInfo = parseForwardInfo(optionsMap?.get("forward_info") as? Map<String, Any?>),
            appId = optionsMap?.get("app_id") as? String,
        )
        return host.sendMessageRich(requireString(map, "recipient"), requireString(map, "content"), sendOptions)
    }
"""

IMPL["sendConnectionRequest"] = """
    fun sendConnectionRequest(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val proto = host.requireProtocolBridge()
        return proto.sendConnectionRequest(
            requireString(map, "recipient"),
            requireString(map, "senderName"),
            ubyteList(map, "keyPackage").ifEmpty { null },
            optionalString(map, "initialMessage"),
        )
    }
"""

IMPL["acceptConnectionRequest"] = """
    fun acceptConnectionRequest(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val proto = host.requireProtocolBridge()
        return proto.acceptConnectionRequest(
            requireString(map, "recipient"),
            requireString(map, "accepterName"),
            ubyteList(map, "keyPackage").ifEmpty { null },
        )
    }
"""

IMPL["rejectConnectionRequest"] = """
    fun rejectConnectionRequest(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().rejectConnectionRequest(requireString(map, "recipient"))
    }
"""

IMPL["cancelConnectionRequest"] = """
    fun cancelConnectionRequest(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().cancelConnectionRequest(requireString(map, "recipient"))
    }
"""

for name in ["blockUser", "unblockUser"]:
    field = "userId"
    IMPL[name] = f"""
    fun {name}(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {{
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().{name}(requireString(map, "{field}"))
        return null
    }}
"""

IMPL["getBlockedUsers"] = """
    fun getBlockedUsers(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.requireProtocolBridge().getBlockedUsers()
"""

IMPL["isUserBlocked"] = """
    fun isUserBlocked(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().isUserBlocked(requireString(map, "userId"))
    }
"""

IMPL["sendPresenceUpdate"] = """
    fun sendPresenceUpdate(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val status = when (requireInt(map, "status")) {
            0 -> PresenceStatus.ONLINE
            1 -> PresenceStatus.AWAY
            2 -> PresenceStatus.OFFLINE
            else -> PresenceStatus.ONLINE
        }
        return host.requireProtocolBridge().sendPresenceUpdate(requireString(map, "recipient"), status)
    }
"""

IMPL["sendTypingIndicator"] = """
    fun sendTypingIndicator(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().sendTypingIndicator(
            requireString(map, "recipient"),
            requireString(map, "conversationId"),
            map["isTyping"] as? Boolean ?: false,
        )
    }
"""

IMPL["sendReadReceipt"] = """
    fun sendReadReceipt(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val ids = stringList(map, "messageIds").filter { it.isNotEmpty() }.ifEmpty {
            throw IllegalArgumentException("No valid message IDs provided")
        }
        return host.requireProtocolBridge().sendReadReceipt(requireString(map, "recipient"), ids)
    }
"""

# Mesh services
IMPL["registerService"] = """
    fun registerService(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val svc = host.requireMeshServicesBridge()
        val capabilities = mutableMapOf<String, String>()
        try {
            val json = JSONObject(requireString(map, "capabilitiesJson"))
            json.keys().forEach { key -> capabilities[key] = json.getString(key) }
        } catch (_: Exception) { }
        svc.registerService(requireString(map, "serviceId"), requireString(map, "version"), capabilities)
        return null
    }
"""

IMPL["unregisterService"] = """
    fun unregisterService(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireMeshServicesBridge().unregisterService(requireString(map, "serviceId"))
    }
"""

IMPL["discoverServices"] = """
    fun discoverServices(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireMeshServicesBridge().discoverServices(optionalString(map, "serviceId"))
    }
"""

IMPL["sendServiceRequest"] = """
    fun sendServiceRequest(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireMeshServicesBridge().sendServiceRequest(
            requireString(map, "provider"),
            requireString(map, "serviceId"),
            requireString(map, "method"),
            requireString(map, "body"),
        )
    }
"""

IMPL["respondToServiceRequest"] = """
    fun respondToServiceRequest(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireMeshServicesBridge().respondToServiceRequest(
            requireString(map, "requestId"),
            requireString(map, "requester"),
            requireString(map, "serviceId"),
            requireString(map, "status"),
            requireString(map, "body"),
        )
    }
"""

IMPL["disableTransport"] = """
    fun disableTransport(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val type = requireString(map, "type")
        val proto = host.requireProtocolBridge()
        when (type.lowercase()) {
            "internet" -> {
                host.internetManager?.stop()
                try { proto.internetStatusChanged(false) } catch (_: Exception) { }
            }
            "wifidirect", "wifi_direct" -> host.wifiDirectManager?.stop()
            "ble" -> {
                host.bleTransport?.stop()
                try { proto.bleStatusChanged(false) } catch (_: Exception) { }
            }
            "reticulum" -> {
                host.reticulumManager?.stop()
                try { proto.reticulumStatusChanged(false) } catch (_: Exception) { }
            }
            "nostr" -> {
                host.nostrManager?.stop()
                try { proto.nostrStatusChanged(false) } catch (_: Exception) { }
            }
            else -> throw IllegalArgumentException("Unsupported transport type: $type")
        }
        return null
    }
"""

IMPL["getActiveTransports"] = """
    fun getActiveTransports(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.protocol?.getActiveTransports() ?: emptyList<String>()
"""

IMPL["getState"] = """
    fun getState(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        return when (host.protocol?.getState()) {
            ProtocolState.STOPPED -> "Stopped"
            ProtocolState.RUNNING -> "Running"
            ProtocolState.PAUSED -> "Paused"
            else -> "Stopped"
        }
    }
"""

IMPL["bleGetPeerCount"] = """
    fun bleGetPeerCount(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.protocol?.bleGetPeerCount()?.toInt() ?: 0
"""

IMPL["bleGetDiagnostics"] = """
    fun bleGetDiagnostics(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val proto = host.protocol
        return mapOf(
            "fragmentFallbacks" to (proto?.bleFragmentFallbackCount()?.toDouble() ?: 0.0),
            "recipientNotAmongPeers" to (proto?.bleRecipientNotAmongPeersCount()?.toDouble() ?: 0.0),
            "undersizedMtuReports" to (proto?.bleUndersizedMtuReports()?.toDouble() ?: 0.0),
        )
    }
"""

IMPL["releaseTransportLock"] = """
    fun releaseTransportLock(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        host.protocol?.releaseTransportLock()
        return null
    }
"""

IMPL["forceTransport"] = """
    fun forceTransport(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().forceTransport(TransportBridge.mapTransportType(requireString(map, "transportType")))
        return null
    }
"""

IMPL["getTransportMetrics"] = """
    fun getTransportMetrics(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val type = TransportBridge.mapTransportType(requireString(map, "transportType"))
        val metrics = host.protocol?.getTransportMetrics(type) ?: return null
        return mapOf(
            "packetsSent" to metrics.packetsSent.toInt(),
            "packetsReceived" to metrics.packetsReceived.toInt(),
            "bytesSent" to metrics.bytesSent.toInt(),
            "bytesReceived" to metrics.bytesReceived.toInt(),
            "errorRate" to metrics.errorRate.toDouble(),
            "avgLatencyMs" to metrics.avgLatencyMs.toInt(),
        )
    }
"""

IMPL["getTopology"] = """
    fun getTopology(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val topology = host.requireProtocolBridge().getTopology()
        return TransportBridge.buildTopologyJson(topology, host.protocol?.localAddress())
    }
"""

IMPL["isRelay"] = """
    fun isRelay(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.protocol?.isRelay() ?: false
"""

IMPL["shouldEscalateToWifi"] = """
    fun shouldEscalateToWifi(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.requireProtocolBridge().shouldEscalateToWifi()
"""

IMPL["getPendingAckCount"] = """
    fun getPendingAckCount(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        (host.protocol?.getPendingAckCount() ?: 0uL).toDouble()
"""

IMPL["getRetryQueueSize"] = """
    fun getRetryQueueSize(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        (host.protocol?.getRetryQueueSize() ?: 0uL).toDouble()
"""

IMPL["isMlsInitialized"] = """
    fun isMlsInitialized(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.protocol?.isMlsInitialized() ?: false
"""

IMPL["getIdentityPublicKey"] = """
    fun getIdentityPublicKey(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val key = host.requireProtocolBridge().getIdentityPublicKey()
        return key.map { it.toInt() }
    }
"""

IMPL["internetIsReady"] = """
    fun internetIsReady(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.internetManager?.isReady() ?: false
"""

IMPL["internetIsSuperseded"] = """
    fun internetIsSuperseded(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.internetManager?.isSessionSuperseded() ?: false
"""

IMPL["internetForceReconnect"] = """
    fun internetForceReconnect(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val manager = host.internetManager ?: return false
        manager.forceReconnect()
        return true
    }
"""

IMPL["internetSendRawCommand"] = """
    fun internetSendRawCommand(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val manager = host.internetManager
            ?: throw IllegalStateException("Internet transport not initialized")
        return manager.sendRawCommand(requireString(map, "json"))
    }
"""

# Service subscriptions (ADR 0035). Mode, policy kind and fallback cross as
# lowercase strings; the policy as one JSON object (`policyJson`), because the
# React Native adapter turns an array argument into a list of integers.
IMPL["subscribeService"] = """
    fun subscribeService(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val mode = when (requireString(map, "mode").lowercase()) {
            "latest" -> SubscriptionMode.LATEST
            "log" -> SubscriptionMode.LOG
            else -> throw IllegalArgumentException("'mode' must be latest or log")
        }
        return host.requireMeshServicesBridge().subscribeService(
            requireString(map, "provider"),
            requireString(map, "serviceId"),
            requireString(map, "topic"),
            mode,
            (map["params"] as? String) ?: "",
            subscriptionKeys(map),
        )
    }

    // `keys` as a list (Flutter), or `keysJson` as a JSON array (React Native,
    // whose adapter reads every array as integers).
    private fun subscriptionKeys(map: Map<String, Any?>): List<String> {
        val raw = map["keysJson"] as? String ?: return stringList(map, "keys")
        val array = JSONArray(raw)
        return List(array.length()) { i -> array.getString(i) }
    }
"""

IMPL["unsubscribeService"] = """
    fun unsubscribeService(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireMeshServicesBridge().unsubscribeService(requireString(map, "subId"))
    }
"""

IMPL["resyncServiceSubscription"] = """
    fun resyncServiceSubscription(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        host.requireMeshServicesBridge().resyncServiceSubscription(requireString(map, "subId"))
        return null
    }
"""

IMPL["serviceSubscriptions"] = """
    fun serviceSubscriptions(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        return host.requireMeshServicesBridge().serviceSubscriptions().map { info ->
            mapOf(
                "subId" to info.subId,
                "provider" to info.provider,
                "serviceId" to info.serviceId,
                "topic" to info.topic,
                "mode" to info.mode.name.lowercase(),
                "state" to info.state,
                "keys" to info.keys,
                "topics" to info.topics,
            )
        }
    }
"""

IMPL["setServiceSubscriptionPolicy"] = """
    fun setServiceSubscriptionPolicy(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val json = JSONObject(requireString(map, "policyJson"))
        val kind = when (json.optString("kind", "restricted").lowercase()) {
            "restricted" -> SubscriptionPolicyKind.RESTRICTED
            "accept_all" -> SubscriptionPolicyKind.ACCEPT_ALL
            "rules" -> SubscriptionPolicyKind.RULES
            else -> throw IllegalArgumentException("'kind' must be restricted, accept_all or rules")
        }
        val fallback = when (json.optString("fallback", "ask").lowercase()) {
            "ask" -> SubscriptionRulesFallback.ASK
            "reject" -> SubscriptionRulesFallback.REJECT
            else -> throw IllegalArgumentException("'fallback' must be ask or reject")
        }
        fun strings(key: String): List<String> {
            val array = json.optJSONArray(key) ?: return emptyList()
            return (0 until array.length()).map { array.getString(it) }
        }
        host.requireMeshServicesBridge().setServiceSubscriptionPolicy(
            requireString(map, "serviceId"),
            SubscriptionPolicyConfig(kind, strings("allowPeers"), strings("allowGroups"), fallback),
        )
        return null
    }
"""

IMPL["acceptServiceSubscription"] = """
    fun acceptServiceSubscription(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        host.requireMeshServicesBridge().acceptServiceSubscription(requireString(map, "subId"))
        return null
    }
"""

IMPL["rejectServiceSubscription"] = """
    fun rejectServiceSubscription(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        host.requireMeshServicesBridge().rejectServiceSubscription(
            requireString(map, "subId"),
            (map["reason"] as? String) ?: "",
        )
        return null
    }
"""

IMPL["endServiceSubscription"] = """
    fun endServiceSubscription(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        host.requireMeshServicesBridge().endServiceSubscription(
            requireString(map, "subId"),
            (map["reason"] as? String) ?: "",
        )
        return null
    }
"""

IMPL["publishServiceUpdate"] = """
    fun publishServiceUpdate(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val report = host.requireMeshServicesBridge().publishServiceUpdate(
            requireString(map, "serviceId"),
            requireString(map, "topic"),
            optionalString(map, "key"),
            (map["body"] as? String) ?: throw IllegalArgumentException("Missing 'body'"),
        )
        return mapOf(
            "subscribers" to report.subscribers.toDouble(),
            "sentLive" to report.sentLive.toDouble(),
            "queued" to report.queued.toDouble(),
            "coalesced" to report.coalesced.toDouble(),
            "failed" to report.failed.toDouble(),
        )
    }
"""

IMPL["setServiceTopicCheckpoint"] = """
    fun setServiceTopicCheckpoint(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        host.requireMeshServicesBridge().setServiceTopicCheckpoint(
            requireString(map, "serviceId"),
            requireString(map, "topic"),
            (map["body"] as? String) ?: throw IllegalArgumentException("Missing 'body'"),
        )
        return null
    }
"""

IMPL["clearServiceTopic"] = """
    fun clearServiceTopic(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        host.requireMeshServicesBridge().clearServiceTopic(
            requireString(map, "serviceId"),
            requireString(map, "topic"),
        )
        return null
    }
"""

IMPL["serviceSubscribers"] = """
    fun serviceSubscribers(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireMeshServicesBridge().serviceSubscribers(optionalString(map, "serviceId")).map { info ->
            mapOf(
                "subId" to info.subId,
                "subscriber" to info.subscriber,
                "serviceId" to info.serviceId,
                "topic" to info.topic,
                "mode" to info.mode.name.lowercase(),
                "state" to info.state,
                "params" to info.params,
                "keys" to info.keys,
            )
        }
    }
"""

# Stub remaining with TODO - user said no stubs. I need to fill ALL.

def stub(name: str) -> str:
    return f"""
    fun {name}(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {{
        throw UnsupportedOperationException("ProtocolEngineHostBridge.{name} not implemented yet")
    }}
"""

def render() -> str:
    parts = [HEADER]
    for m in METHODS:
        parts.append(IMPL.get(m, stub(m)))
    parts.append("\n}\n")
    return "".join(parts)


def main() -> None:
    import sys

    rendered = render()
    missing = [m for m in METHODS if m not in IMPL]
    if "--check" in sys.argv:
        if OUT.read_text() != rendered:
            print(f"{OUT.relative_to(REPO)} is stale; run {pathlib.Path(__file__).name}", file=sys.stderr)
            raise SystemExit(1)
        if missing:
            print(f"stubbed methods remain: {missing}", file=sys.stderr)
            raise SystemExit(1)
        print(f"{OUT.name}: OK ({len(METHODS)} methods)")
        return
    OUT.write_text(rendered)
    print(f"Wrote {OUT}; {len(missing)} methods still stubbed: {missing[:10]}...")


if __name__ == "__main__":
    main()
