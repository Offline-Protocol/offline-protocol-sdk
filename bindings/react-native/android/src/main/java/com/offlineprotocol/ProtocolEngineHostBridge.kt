
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

    fun acceptConnectionRequest(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val proto = host.requireProtocolBridge()
        return proto.acceptConnectionRequest(
            requireString(map, "recipient"),
            requireString(map, "accepterName"),
            ubyteList(map, "keyPackage").ifEmpty { null },
        )
    }

    fun bleGetDiagnostics(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val proto = host.protocol
        return mapOf(
            "fragmentFallbacks" to (proto?.bleFragmentFallbackCount()?.toDouble() ?: 0.0),
            "recipientNotAmongPeers" to (proto?.bleRecipientNotAmongPeersCount()?.toDouble() ?: 0.0),
            "undersizedMtuReports" to (proto?.bleUndersizedMtuReports()?.toDouble() ?: 0.0),
        )
    }

    fun bleGetPeerCount(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.protocol?.bleGetPeerCount()?.toInt() ?: 0

    fun blockUser(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().blockUser(requireString(map, "userId"))
        return null
    }

    fun cancelConnectionRequest(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().cancelConnectionRequest(requireString(map, "recipient"))
    }

    fun cancelFileTransfer(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().cancelFileTransfer(requireString(map, "fileId"))
        return null

    }

    fun checkInternetPresence(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val manager = host.internetManager
            ?: throw IllegalStateException("Internet transport not initialized")
        val force = map["force"] as? Boolean ?: false
        val latch = java.util.concurrent.CountDownLatch(1)
        var result = false
        manager.checkPresence(requireString(map, "userId"), force) { written ->
            result = written
            latch.countDown()
        }
        latch.await()
        return result

    }

    fun createInvite(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().createInvite(
            optionalString(map, "petname"),
            map["signed"] as? Boolean ?: false,
        )

    }

    fun deriveAddress(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        return uniffi.offline_protocol.deriveAddress(ubyteList(map, "publicKey"))

    }

    fun deriveUserIdFromPublicKey(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().deriveUserIdFromPublicKey(ubyteList(map, "publicKey"))

    }

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

    fun discoverServices(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireMeshServicesBridge().discoverServices(optionalString(map, "serviceId"))
    }

    fun eraseCustody(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        host.requireProtocolBridge().eraseCustody()
        return null

    }

    fun establishSecureSession(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val welcome = host.requireProtocolBridge().establishSecureSession(requireString(map, "peerId"))
        return welcome?.let { mlsWelcomeMap(it) }

    }

    fun finalizeFile(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().finalizeFile(requireString(map, "fileId"))
        return null

    }

    fun forceTransport(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().forceTransport(TransportBridge.mapTransportType(requireString(map, "transportType")))
        return null
    }

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

    fun getActiveTransports(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.protocol?.getActiveTransports() ?: emptyList<String>()

    fun getBatteryLevel(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        return host.protocol?.getBatteryLevel()?.toInt()

    }

    fun getBlockedUsers(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.requireProtocolBridge().getBlockedUsers()

    fun getCustodyStats(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val stats = host.protocol?.getCustodyStats() ?: return null
        return mapOf(
            "held" to stats.held.toDouble(),
            "heldBytes" to stats.heldBytes.toDouble(),
            "accepted" to stats.accepted.toDouble(),
            "delivered" to stats.delivered.toDouble(),
            "reOriginated" to stats.reOriginated.toDouble(),
            "expired" to stats.expired.toDouble(),
            "duplicates" to stats.duplicates.toDouble(),
            "evicted" to stats.evicted.toDouble(),
            "receiptsSent" to stats.receiptsSent.toDouble(),
            "receiptsDropped" to stats.receiptsDropped.toDouble(),
            "receiptsReceived" to stats.receiptsReceived.toDouble(),
            "receiptsIgnored" to stats.receiptsIgnored.toDouble(),
            "refusedDisabled" to stats.refusedDisabled.toDouble(),
            "refusedNoRequest" to stats.refusedNoRequest.toDouble(),
            "refusedUnknownClass" to stats.refusedUnknownClass.toDouble(),
            "refusedNotSealed" to stats.refusedNotSealed.toDouble(),
            "refusedUnprovenPeer" to stats.refusedUnprovenPeer.toDouble(),
            "refusedNotDepositor" to stats.refusedNotDepositor.toDouble(),
            "refusedStranger" to stats.refusedStranger.toDouble(),
            "refusedDepositorFull" to stats.refusedDepositorFull.toDouble(),
            "refusedStoreFull" to stats.refusedStoreFull.toDouble(),
            "refusedBattery" to stats.refusedBattery.toDouble(),
        )

    }

    fun getDedupStats(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val stats = host.protocol?.getDedupStats() ?: return null
        return mapOf(
            "totalTracked" to stats.totalTracked.toDouble(),
            "recentTracked" to stats.recentTracked.toDouble(),
            "capacityUsedPercent" to stats.capacityUsedPercent.toInt(),
            "mode" to stats.mode,
        )

    }

    fun getDeliverySuccessRate(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        return host.requireProtocolBridge().getDeliverySuccessRate().toDouble()

    }

    fun getDorsConfig(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val config = host.requireProtocolBridge().getDorsConfig()
        return JSONObject().apply {
            put("preferOnline", config.preferOnline)
            put("switchHysteresis", config.switchHysteresis.toDouble())
            put("switchCooldownSecs", config.switchCooldownSecs.toLong())
            put("bleToWifiRetryThreshold", config.bleToWifiRetryThreshold.toInt())
            put("minSuccessRateBeforeEscalation", config.minSuccessRateBeforeEscalation.toDouble())
            put("minBleSamplesBeforeSuccessRateEscalation", config.minBleSamplesBeforeSuccessRateEscalation.toLong())
            put("rssiSwitchThreshold", config.rssiSwitchThreshold.toInt())
            put("congestionQueueThreshold", config.congestionQueueThreshold.toLong())
            put("stabilityWindowSecs", config.stabilityWindowSecs.toLong())
            put("poorSignalDurationSecs", config.poorSignalDurationSecs.toLong())
            put("ttlEscalationThreshold", config.ttlEscalationThreshold.toInt())
            put("congestionDurationSecs", config.congestionDurationSecs.toLong())
            put("ttlEscalationHoldSecs", config.ttlEscalationHoldSecs.toLong())
            put("historyWindowSize", config.historyWindowSize.toLong())
            put("queueRecoveryRatio", config.queueRecoveryRatio.toDouble())
            put("lowBatteryThreshold", config.lowBatteryThreshold.toInt())
            put("relayMinBatteryLevel", config.relayMinBatteryLevel.toInt())
            put("relayOptimalConnectionCount", config.relayOptimalConnectionCount.toInt())
        }.toString()

    }

    fun getEstablishmentState(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val proto = host.protocol ?: return "NoKeyPackage"
        return when (proto.getEstablishmentState(requireString(map, "peerId"))) {
            EstablishmentState.NO_KEY_PACKAGE -> "NoKeyPackage"
            EstablishmentState.HAVE_KEY_PACKAGE -> "HaveKeyPackage"
            EstablishmentState.SESSION_PENDING -> "SessionPending"
            EstablishmentState.SESSION_CONFIRMED -> "SessionConfirmed"
        }

    }

    fun getFileProgress(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val progress = host.requireProtocolBridge().getFileProgress(requireString(map, "fileId"))
            ?: return null
        return mapOf(
            "file_id" to progress.fileId,
            "chunks_sent" to progress.chunksSent.toInt(),
            "total_chunks" to progress.totalChunks.toInt(),
            "percentage" to progress.percentage.toInt(),
        )

    }

    fun getIdentityPublicKey(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val key = host.requireProtocolBridge().getIdentityPublicKey()
        return key.map { it.toInt() }
    }

    fun getIsCharging(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        return host.protocol?.getIsCharging() ?: false

    }

    fun getMedianHops(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val hops = host.requireProtocolBridge().getMedianHops().toInt()
        return if (hops == 0) null else hops

    }

    fun getMedianLatency(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val latency = host.requireProtocolBridge().getMedianLatency().toLong()
        return if (latency == 0L) null else latency

    }

    fun getMeshRelayStats(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val stats = host.protocol?.getMeshRelayStats() ?: return null
        return mapOf(
            "forwarded" to stats.forwarded.toDouble(),
            "transmissions" to stats.transmissions.toDouble(),
            "queued" to stats.queued.toDouble(),
            "awaitingTransmission" to stats.awaitingTransmission.toDouble(),
            "duplicatesSuppressed" to stats.duplicatesSuppressed.toDouble(),
            "coveredByANeighbor" to stats.coveredByANeighbor.toDouble(),
            "peerRateLimited" to stats.peerRateLimited.toDouble(),
            "refusedQueueFull" to stats.refusedQueueFull.toDouble(),
            "rateDeferred" to stats.rateDeferred.toDouble(),
            "abandonedOverdue" to stats.abandonedOverdue.toDouble(),
            "hopLimitReached" to stats.hopLimitReached.toDouble(),
            "reachClamped" to stats.reachClamped.toDouble(),
            "droppedForCapacity" to stats.droppedForCapacity.toDouble(),
        )

    }

    fun getMeshRelayTunables(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val tunables = host.protocol?.getMeshRelayTunables() ?: return null
        return mapOf(
            "maxTtl" to tunables.maxTtl.toInt(),
            "denseMaxTtl" to tunables.denseMaxTtl.toInt(),
            "denseDegree" to tunables.denseDegree.toDouble(),
            "fanout" to tunables.fanout.toDouble(),
            "jitterMinMs" to tunables.jitterMinMs.toDouble(),
            "jitterMaxMs" to tunables.jitterMaxMs.toDouble(),
            "ratePerSec" to tunables.ratePerSec.toDouble(),
            "burst" to tunables.burst.toDouble(),
            "peerRatePerSec" to tunables.peerRatePerSec.toDouble(),
            "peerBurst" to tunables.peerBurst.toDouble(),
            "queueCapacity" to tunables.queueCapacity.toDouble(),
            "biasMinScale" to tunables.biasMinScale.toDouble(),
            "biasMaxHandicapMs" to tunables.biasMaxHandicapMs.toDouble(),
            "activityWindowMs" to tunables.activityWindowMs.toDouble(),
            "activityMinForwards" to tunables.activityMinForwards.toDouble(),
            "activityIdleWindows" to tunables.activityIdleWindows.toDouble(),
        )

    }

    fun getMessageStats(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val stats = host.requireProtocolBridge().getMessageStats()
        return TransportBridge.buildMessageStatsJson(stats)

    }

    fun getPendingAckCount(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        (host.protocol?.getPendingAckCount() ?: 0uL).toDouble()

    fun getRelayConfig(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val config = host.requireProtocolBridge().getRelayConfig()
        return JSONObject().apply {
            put("minBatteryForRelay", config.minBatteryForRelay.toInt())
            put("allowRelay", config.allowRelay)
            put("relayPriority", InitialRuntimeConfigApplier.relayPriorityName(config.relayPriority))
        }.toString()

    }

    fun getRelayPriority(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        return InitialRuntimeConfigApplier.relayPriorityName(host.protocol?.getRelayPriority())

    }

    fun getRetryQueueSize(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        (host.protocol?.getRetryQueueSize() ?: 0uL).toDouble()

    fun getState(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        return when (host.protocol?.getState()) {
            ProtocolState.STOPPED -> "Stopped"
            ProtocolState.RUNNING -> "Running"
            ProtocolState.PAUSED -> "Paused"
            else -> "Stopped"
        }
    }

    fun getTopology(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val topology = host.requireProtocolBridge().getTopology()
        return TransportBridge.buildTopologyJson(topology, host.protocol?.localAddress())
    }

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

    fun hasPendingKeyPackage(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        return host.protocol?.hasPendingKeyPackage(requireString(map, "peerId")) ?: false

    }

    fun internetConfirmSent(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().internetConfirmSent(requireString(map, "messageId"))
        return null

    }

    fun internetForceReconnect(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val manager = host.internetManager ?: return false
        manager.forceReconnect()
        return true
    }

    fun internetGetNextMessage(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val message = host.requireProtocolBridge().internetGetNextMessage() ?: return null
        return mapOf(
            "messageId" to message.messageId,
            "recipientId" to message.recipientId,
            "data" to message.data.map { it.toInt() },
        )

    }

    fun internetIsReady(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.internetManager?.isReady() ?: false

    fun internetIsSuperseded(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.internetManager?.isSessionSuperseded() ?: false

    fun internetMessageReceived(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().internetMessageReceived(
            requireString(map, "senderId"),
            ubyteList(map, "data"),
        )
        return null

    }

    fun internetSendFailed(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().internetSendFailed(requireString(map, "messageId"))
        return null

    }

    fun internetSendRawCommand(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val manager = host.internetManager
            ?: throw IllegalStateException("Internet transport not initialized")
        return manager.sendRawCommand(requireString(map, "json"))
    }

    fun internetStatusChanged(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().internetStatusChanged(map["isConnected"] as? Boolean ?: false)
        return null

    }

    fun isBluetoothEnabled(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val context = ctx ?: throw IllegalStateException("Context required")
        val adapter = (context.getSystemService(Context.BLUETOOTH_SERVICE) as? android.bluetooth.BluetoothManager)?.adapter
        return adapter?.isEnabled == true

    }

    fun isMlsInitialized(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.protocol?.isMlsInitialized() ?: false

    fun isRelay(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.protocol?.isRelay() ?: false

    fun isUserBlocked(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().isUserBlocked(requireString(map, "userId"))
    }

    fun mlsCreateSession(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        return mlsWelcomeMap(host.requireProtocolBridge().mlsCreateSession(requireString(map, "otherUserId")))

    }

    fun mlsDecrypt(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val plaintext = host.requireProtocolBridge().mlsDecrypt(
            mlsEncryptedFromJson(JSONObject(requireString(map, "encryptedJson"))),
        )
        return plaintext?.map { it.toInt() }

    }

    fun mlsDecryptFromUser(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val plaintext = host.requireProtocolBridge().mlsDecryptFromUser(
            mlsEncryptedFromJson(JSONObject(requireString(map, "encryptedJson"))),
        )
        return plaintext?.map { it.toInt() }

    }

    fun mlsDeleteSession(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().mlsDeleteSession(requireString(map, "otherUserId"))
        return null

    }

    fun mlsEncryptForUser(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val encrypted = host.requireProtocolBridge().mlsEncryptForUser(
            requireString(map, "otherUserId"),
            ubyteList(map, "plaintext"),
        )
        return mlsEncryptedMap(encrypted)

    }

    fun mlsGenerateKeyPackage(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val bundle = host.requireProtocolBridge().mlsGenerateKeyPackage()
        return mapOf(
            "packageId" to bundle.packageId,
            "keyPackage" to bundle.keyPackage.map { it.toInt() },
        )

    }

    fun mlsGetOrCreateKeyPackage(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val bundle = host.requireProtocolBridge().mlsGetOrCreateKeyPackage()
        return mapOf(
            "packageId" to bundle.packageId,
            "keyPackage" to bundle.keyPackage.map { it.toInt() },
        )

    }

    fun mlsGetPendingKeyPackages(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val packages = host.requireProtocolBridge().mlsGetPendingKeyPackages()
        return packages.map { mapOf("packageId" to it.packageId, "keyPackage" to it.keyPackage.map { b -> b.toInt() }) }

    }

    fun mlsHasSession(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        return host.protocol?.mlsHasSession(requireString(map, "otherUserId")) ?: false

    }

    fun mlsImportKeyPackage(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().mlsImportKeyPackage(requireString(map, "userId"), ubyteList(map, "keyPackageData"))
        return null

    }

    fun mlsJoinSession(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val json = JSONObject(requireString(map, "welcomeJson"))
        val info = host.requireProtocolBridge().mlsJoinSession(mlsWelcomeFromJson(json))
        return mlsGroupInfoMap(info)

    }

    fun mlsListSessions(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        return host.protocol?.mlsListSessions() ?: emptyList<String>()

    }

    fun mlsMarkKeyPackageSynced(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().mlsMarkKeyPackageSynced(requireString(map, "packageId"))
        return null

    }

    fun mlsProcessWelcome(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val json = JSONObject(requireString(map, "welcomeJson"))
        val info = host.requireProtocolBridge().mlsProcessWelcome(mlsWelcomeFromJson(json))
        return mlsGroupInfoMap(info)

    }

    fun parseInvite(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val invite = uniffi.offline_protocol.parseInvite(requireString(map, "blob"))
        return mapOf(
            "address" to invite.address,
            "public_key" to invite.publicKey.map { it.toInt() },
            "petname" to invite.petname,
            "signed" to invite.signed,
        )

    }

    fun processFileChunk(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().processFileChunk(
            requireString(map, "fileId"),
            requireInt(map, "chunkIndex").toUInt(),
            requireInt(map, "totalChunks").toUInt(),
            requireDouble(map, "fileSize").toULong(),
            requireString(map, "fileName"),
            requireString(map, "fileChecksum"),
            ubyteList(map, "data"),
        )
        return null

    }

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

    fun rejectConnectionRequest(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().rejectConnectionRequest(requireString(map, "recipient"))
    }

    fun rekeySession(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().rekeySession(requireString(map, "peerId"))

    }

    fun releaseTransportLock(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        host.protocol?.releaseTransportLock()
        return null
    }

    fun requestEnableBluetooth(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val context = ctx ?: throw IllegalStateException("Context required")
        val adapter = (context.getSystemService(Context.BLUETOOTH_SERVICE) as? android.bluetooth.BluetoothManager)?.adapter
            ?: throw IllegalStateException("Bluetooth not available")
        if (adapter.isEnabled) return true
        val activity = context as? android.app.Activity
            ?: throw IllegalStateException("No activity available to show Bluetooth enable dialog")
        @Suppress("DEPRECATION")
        activity.startActivityForResult(Intent(BluetoothAdapter.ACTION_REQUEST_ENABLE), 1001)
        return false

    }

    fun resolveUsername(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().resolveUsername(requireString(map, "username"))

    }

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

    fun sendFileFromPath(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().sendFileFromPath(
            requireString(map, "recipient"),
            requireString(map, "filePath"),
            requireString(map, "fileName"),
        )

    }

    fun sendMedia(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val bytes = Base64.decode(requireString(map, "fileData"), Base64.DEFAULT)
        @Suppress("UNCHECKED_CAST")
        val metaMap = map["mediaMetadata"] as? Map<String, Any?>
        val meta = metaMap?.let {
            MediaMetadata(
                mimeType = it["mime_type"] as? String ?: "",
                fileName = it["file_name"] as? String ?: requireString(map, "fileName"),
                fileSize = (it["file_size"] as? Number)?.toLong()?.toULong() ?: 0u,
                durationMs = (it["duration_ms"] as? Number)?.toLong()?.toULong(),
                width = (it["width"] as? Number)?.toInt()?.toUInt(),
                height = (it["height"] as? Number)?.toInt()?.toUInt(),
                thumbnailBase64 = it["thumbnail_base64"] as? String,
            )
        }
        return host.requireProtocolBridge().sendMedia(
            requireString(map, "recipient"),
            bytes.map { it.toUByte() },
            requireString(map, "fileName"),
            parseContentType(requireString(map, "contentType")),
            meta,
        )

    }

    fun sendMediaRich(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val bytes = Base64.decode(requireString(map, "fileData"), Base64.DEFAULT)
        @Suppress("UNCHECKED_CAST")
        val options = map["options"] as? Map<String, Any?>
        val sendOptions = MediaSendOptions(
            mediaMetadata = parseRichMediaMetadata(options?.get("media_metadata") as? Map<String, Any?>),
            caption = options?.get("caption") as? String,
            replyToMsg = options?.get("reply_to_msg") as? String,
            replyContext = parseReplyContext(options?.get("reply_context") as? Map<String, Any?>),
            forwardInfo = parseForwardInfo(options?.get("forward_info") as? Map<String, Any?>),
            fileId = options?.get("file_id") as? String,
            appId = options?.get("app_id") as? String,
        )
        return host.requireProtocolBridge().sendMediaRich(
            requireString(map, "recipient"),
            bytes.map { it.toUByte() },
            requireString(map, "fileName"),
            parseContentType(requireString(map, "contentType")),
            sendOptions,
        )

    }

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

    fun sendReadReceipt(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        val ids = stringList(map, "messageIds").ifEmpty {
            throw IllegalArgumentException("No valid message IDs provided")
        }
        return host.requireProtocolBridge().sendReadReceipt(requireString(map, "recipient"), ids)
    }

    fun sendServiceRequest(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireMeshServicesBridge().sendServiceRequest(
            requireString(map, "provider"),
            requireString(map, "serviceId"),
            requireString(map, "method"),
            requireString(map, "body"),
        )
    }

    fun sendTypingIndicator(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().sendTypingIndicator(
            requireString(map, "recipient"),
            requireString(map, "conversationId"),
            map["isTyping"] as? Boolean ?: false,
        )
    }

    fun setBatteryLevel(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val level = requireInt(map, "level").coerceIn(Limits.MIN_BATTERY, Limits.MAX_BATTERY).toUByte()
        host.requireProtocolBridge().setBatteryLevel(level)
        return null

    }

    fun setBatteryState(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val level = requireInt(map, "level").coerceIn(Limits.MIN_BATTERY, Limits.MAX_BATTERY).toUByte()
        host.requireProtocolBridge().setBatteryState(level, map["isCharging"] as? Boolean ?: false)
        return null

    }

    fun setRelayPriority(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val priority = InitialRuntimeConfigApplier.normalizeRelayPriority(requireString(map, "priorityString"))
            ?: throw IllegalArgumentException("Invalid relay priority")
        host.requireProtocolBridge().setRelayPriority(priority)
        return null

    }

    fun shouldEscalateToWifi(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? =
        host.requireProtocolBridge().shouldEscalateToWifi()

    fun signData(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val sig = host.requireProtocolBridge().signData(ubyteList(map, "data"))
        return sig.map { it.toInt() }

    }

    fun signDataBase64(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val data = Base64.decode(requireString(map, "dataBase64"), Base64.DEFAULT)
        val sig = host.requireProtocolBridge().signData(data.map { it.toUByte() })
        return Base64.encodeToString(sig.map { it.toByte() }.toByteArray(), Base64.NO_WRAP)

    }

    fun unblockUser(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().unblockUser(requireString(map, "userId"))
        return null
    }

    fun unregisterService(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {
        val map = mapFromAny(arguments)
        return host.requireMeshServicesBridge().unregisterService(requireString(map, "serviceId"))
    }

    fun updateAckConfig(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val json = JSONObject(configJsonString(arguments))
        host.protocol?.updateAckConfig(
            AckConfig(
                defaultTimeoutMs = json.optLong("defaultTimeoutMs", 10000).toULong(),
                maxPendingAcks = json.optLong("maxPendingAcks", 1000).toULong(),
            ),
        )
        return null

    }

    fun updateDedupConfig(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val json = JSONObject(configJsonString(arguments))
        host.protocol?.updateDedupConfig(
            DedupConfig(
                maxTrackedMessages = json.optLong("maxTrackedMessages", 5000).toULong(),
                retentionTimeSecs = json.optLong("retentionTimeSecs", 604800).toULong(),
            ),
        )
        return null

    }

    fun updateDorsConfig(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val json = JSONObject(configJsonString(arguments))
        val proto = host.requireProtocolBridge()
        val current = proto.getDorsConfig()
        val dorsConfig = DorsConfig(
            preferOnline = json.optBoolean("preferOnline", current.preferOnline),
            switchHysteresis = json.optDouble("switchHysteresis", current.switchHysteresis.toDouble()).toFloat().coerceAtLeast(0f),
            switchCooldownSecs = json.optLong("switchCooldownSecs", current.switchCooldownSecs.toLong()).coerceAtLeast(0).toULong(),
            bleToWifiRetryThreshold = json.optInt("bleToWifiRetryThreshold", current.bleToWifiRetryThreshold.toInt()).toUInt(),
            minSuccessRateBeforeEscalation = json.optDouble("minSuccessRateBeforeEscalation", current.minSuccessRateBeforeEscalation.toDouble()).toFloat().coerceIn(0f, 1f),
            minBleSamplesBeforeSuccessRateEscalation = json.optLong("minBleSamplesBeforeSuccessRateEscalation", current.minBleSamplesBeforeSuccessRateEscalation.toLong()).coerceAtLeast(0).toULong(),
            rssiSwitchThreshold = json.optInt("rssiSwitchThreshold", current.rssiSwitchThreshold.toInt()).toShort(),
            congestionQueueThreshold = json.optLong("congestionQueueThreshold", current.congestionQueueThreshold.toLong()).toULong(),
            stabilityWindowSecs = json.optLong("stabilityWindowSecs", current.stabilityWindowSecs.toLong()).toULong(),
            poorSignalDurationSecs = json.optLong("poorSignalDurationSecs", current.poorSignalDurationSecs.toLong()).toULong(),
            ttlEscalationThreshold = json.optInt("ttlEscalationThreshold", current.ttlEscalationThreshold.toInt()).toUByte(),
            congestionDurationSecs = json.optLong("congestionDurationSecs", current.congestionDurationSecs.toLong()).coerceAtLeast(0).toULong(),
            ttlEscalationHoldSecs = json.optLong("ttlEscalationHoldSecs", current.ttlEscalationHoldSecs.toLong()).coerceAtLeast(1).toULong(),
            historyWindowSize = json.optLong("historyWindowSize", current.historyWindowSize.toLong()).let {
                max(Limits.MIN_HISTORY, min(Limits.MAX_HISTORY, it))
            }.toULong(),
            queueRecoveryRatio = json.optDouble("queueRecoveryRatio", current.queueRecoveryRatio.toDouble()).toFloat().coerceIn(0f, 1f),
            lowBatteryThreshold = json.optInt("lowBatteryThreshold", current.lowBatteryThreshold.toInt())
                .coerceIn(Limits.MIN_BATTERY, Limits.MAX_BATTERY).toUByte(),
            relayMinBatteryLevel = json.optInt("relayMinBatteryLevel", current.relayMinBatteryLevel.toInt())
                .coerceIn(Limits.MIN_BATTERY, Limits.MAX_BATTERY).toUByte(),
            relayOptimalConnectionCount = json.optInt(
                "relayOptimalConnectionCount",
                current.relayOptimalConnectionCount.toInt(),
            ).coerceIn(Limits.MIN_BATTERY, Limits.MAX_BATTERY).toUByte(),
        )
        proto.updateDorsConfig(dorsConfig)
        return null

    }

    fun updateRelayConfig(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val jsonStr = configJsonString(arguments)
        val json = JSONObject(jsonStr)
        val proto = host.requireProtocolBridge()
        val current = proto.getRelayConfig()
        val priority = InitialRuntimeConfigApplier.normalizeRelayPriority(
            json.optString("relayPriority", json.optString("relay_priority")),
        ) ?: current.relayPriority
        proto.updateRelayConfig(
            RelayConfig(
                minBatteryForRelay = json.optIntCompat("minBatteryForRelay", "min_battery_for_relay")
                    ?.coerceIn(Limits.MIN_BATTERY, Limits.MAX_BATTERY)?.toUByte()
                    ?: current.minBatteryForRelay,
                allowRelay = json.optBooleanCompat("allowRelay", "allow_relay") ?: current.allowRelay,
                relayPriority = priority,
            ),
        )
        return null

    }

    fun updateRetryConfig(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val json = JSONObject(configJsonString(arguments))
        host.protocol?.updateRetryConfig(
            RetryConfig(
                maxRetries = json.optInt("maxRetries", 10).toUInt(),
                initialDelayMs = json.optLong("initialDelayMs", 1000).toULong(),
                maxDelayMs = json.optLong("maxDelayMs", 300000).toULong(),
                backoffMultiplier = json.optDouble("backoffMultiplier", 2.0).toFloat(),
                outboxMaxLifetimeMs = json.optLong("outboxMaxLifetimeMs", 604800000).toULong(),
                pendingMessageMaxLifetimeMs = json.optLong("pendingMessageMaxLifetimeMs", 604800000).toULong(),
                edgeDrivenUnreachableDm = json.optBoolean("edgeDrivenUnreachableDm", false),
            ),
        )
        return null

    }

    fun verifySignature(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().verifySignature(
            ubyteList(map, "publicKey"),
            ubyteList(map, "data"),
            ubyteList(map, "signature"),
        )

    }

    fun verifySignatureBase64(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val pub = Base64.decode(requireString(map, "publicKeyBase64"), Base64.DEFAULT)
        val data = Base64.decode(requireString(map, "dataBase64"), Base64.DEFAULT)
        val sig = Base64.decode(requireString(map, "signatureBase64"), Base64.DEFAULT)
        return host.requireProtocolBridge().verifySignature(
            pub.map { it.toUByte() },
            data.map { it.toUByte() },
            sig.map { it.toUByte() },
        )

    }

    fun wifiDirectGetNextMessage(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val message = host.requireProtocolBridge().wifiDirectGetNextMessage() ?: return null
        return mapOf(
            "recipientId" to message.recipientId,
            "data" to message.data.map { it.toInt() },
        )

    }

    fun wifiDirectMessageReceived(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().wifiDirectMessageReceived(
            requireString(map, "senderId"),
            ubyteList(map, "data"),
        )
        return null

    }

    fun wifiDirectPeerConnected(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().wifiDirectPeerConnected(requireString(map, "peerId"))
        return null

    }

    fun wifiDirectPeerDisconnected(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().wifiDirectPeerDisconnected(requireString(map, "peerId"))
        return null

    }

    fun wifiDirectStatusChanged(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        host.requireProtocolBridge().wifiDirectStatusChanged(map["isConnected"] as? Boolean ?: false)
        return null

    }

    fun wipePersistedState(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {

        val map = mapFromAny(arguments)
        val appId = requireString(map, "appId")
        val profile = requireString(map, "profile")
        val context = ctx ?: throw IllegalStateException("Context required")
        val namespace = StorageNamespace.account(appId, profile)
        val live = host.currentConfig
        if (live != null && StorageNamespace.account(live.appId, live.profile) == namespace) {
            throw IllegalStateException("Refusing to wipe storage for the running account; call destroy() first")
        }
        var firstError: Exception? = null
        try {
            MlsSecureStorage.wipeAccount(context, namespace)
        } catch (e: Exception) {
            firstError = e
        }
        try {
            AppContainerProtocolStateStorage.wipeAccount(context, namespace)
        } catch (e: Exception) {
            if (firstError == null) firstError = e
        }
        firstError?.let { throw it }
        return null

    }

}
