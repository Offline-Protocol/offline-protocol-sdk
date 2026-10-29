# Supplemental ProtocolEngineHostBridge method bodies (merged by gen script).
SUPPLEMENT_IMPL: dict[str, str] = {}

def _m(name: str, body: str) -> None:
    SUPPLEMENT_IMPL[name] = f"""
    fun {name}(host: ProtocolEngineHost, ctx: Context?, arguments: Any?): Any? {{
{body}
    }}
"""

_m("getMessageStats", """
        val stats = host.requireProtocolBridge().getMessageStats()
        return TransportBridge.buildMessageStatsJson(stats)
""")

_m("getDeliverySuccessRate", """
        return host.requireProtocolBridge().getDeliverySuccessRate().toDouble()
""")

_m("getMedianLatency", """
        val latency = host.requireProtocolBridge().getMedianLatency().toLong()
        return if (latency == 0L) null else latency
""")

_m("getMedianHops", """
        val hops = host.requireProtocolBridge().getMedianHops().toInt()
        return if (hops == 0) null else hops
""")

_m("sendFileFromPath", """
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().sendFileFromPath(
            requireString(map, "recipient"),
            requireString(map, "filePath"),
            requireString(map, "fileName"),
        )
""")

_m("sendMedia", """
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
""")

_m("sendMediaRich", """
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
""")

_m("getFileProgress", """
        val map = mapFromAny(arguments)
        val progress = host.requireProtocolBridge().getFileProgress(requireString(map, "fileId"))
            ?: return null
        return mapOf(
            "file_id" to progress.fileId,
            "chunks_sent" to progress.chunksSent.toInt(),
            "total_chunks" to progress.totalChunks.toInt(),
            "percentage" to progress.percentage.toInt(),
        )
""")

_m("cancelFileTransfer", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().cancelFileTransfer(requireString(map, "fileId"))
        return null
""")

_m("processFileChunk", """
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
""")

_m("finalizeFile", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().finalizeFile(requireString(map, "fileId"))
        return null
""")

_m("setBatteryLevel", """
        val map = mapFromAny(arguments)
        val level = requireInt(map, "level").coerceIn(Limits.MIN_BATTERY, Limits.MAX_BATTERY).toUByte()
        host.requireProtocolBridge().setBatteryLevel(level)
        return null
""")

_m("setBatteryState", """
        val map = mapFromAny(arguments)
        val level = requireInt(map, "level").coerceIn(Limits.MIN_BATTERY, Limits.MAX_BATTERY).toUByte()
        host.requireProtocolBridge().setBatteryState(level, map["isCharging"] as? Boolean ?: false)
        return null
""")

_m("getBatteryLevel", """
        return host.protocol?.getBatteryLevel()?.toInt()
""")

_m("getIsCharging", """
        return host.protocol?.getIsCharging() ?: false
""")

_m("setRelayPriority", """
        val map = mapFromAny(arguments)
        val priority = InitialRuntimeConfigApplier.normalizeRelayPriority(requireString(map, "priorityString"))
            ?: throw IllegalArgumentException("Invalid relay priority")
        host.requireProtocolBridge().setRelayPriority(priority)
        return null
""")

_m("getRelayPriority", """
        return InitialRuntimeConfigApplier.relayPriorityName(host.protocol?.getRelayPriority())
""")

_m("updateRelayConfig", """
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
""")

_m("getRelayConfig", """
        val config = host.requireProtocolBridge().getRelayConfig()
        return JSONObject().apply {
            put("minBatteryForRelay", config.minBatteryForRelay.toInt())
            put("allowRelay", config.allowRelay)
            put("relayPriority", InitialRuntimeConfigApplier.relayPriorityName(config.relayPriority))
        }.toString()
""")

_m("updateDorsConfig", """
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
""")

_m("getDorsConfig", """
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
""")

_m("updateAckConfig", """
        val json = JSONObject(configJsonString(arguments))
        host.protocol?.updateAckConfig(
            AckConfig(
                defaultTimeoutMs = json.optLong("defaultTimeoutMs", 10000).toULong(),
                maxPendingAcks = json.optLong("maxPendingAcks", 1000).toULong(),
            ),
        )
        return null
""")

_m("updateRetryConfig", """
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
""")

_m("updateDedupConfig", """
        val json = JSONObject(configJsonString(arguments))
        host.protocol?.updateDedupConfig(
            DedupConfig(
                maxTrackedMessages = json.optLong("maxTrackedMessages", 5000).toULong(),
                retentionTimeSecs = json.optLong("retentionTimeSecs", 604800).toULong(),
            ),
        )
        return null
""")

_m("getDedupStats", """
        val stats = host.protocol?.getDedupStats() ?: return null
        return mapOf(
            "totalTracked" to stats.totalTracked.toDouble(),
            "recentTracked" to stats.recentTracked.toDouble(),
            "capacityUsedPercent" to stats.capacityUsedPercent.toInt(),
            "mode" to stats.mode,
        )
""")

_m("getMeshRelayStats", """
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
""")

_m("getMeshRelayTunables", """
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
""")

_m("getCustodyStats", """
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
""")

_m("eraseCustody", """
        host.requireProtocolBridge().eraseCustody()
        return null
""")

_m("wifiDirectStatusChanged", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().wifiDirectStatusChanged(map["isConnected"] as? Boolean ?: false)
        return null
""")

_m("wifiDirectMessageReceived", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().wifiDirectMessageReceived(
            requireString(map, "senderId"),
            ubyteList(map, "data"),
        )
        return null
""")

_m("wifiDirectGetNextMessage", """
        val message = host.requireProtocolBridge().wifiDirectGetNextMessage() ?: return null
        return mapOf(
            "recipientId" to message.recipientId,
            "data" to message.data.map { it.toInt() },
        )
""")

_m("wifiDirectPeerConnected", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().wifiDirectPeerConnected(requireString(map, "peerId"))
        return null
""")

_m("wifiDirectPeerDisconnected", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().wifiDirectPeerDisconnected(requireString(map, "peerId"))
        return null
""")

_m("internetStatusChanged", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().internetStatusChanged(map["isConnected"] as? Boolean ?: false)
        return null
""")

_m("internetMessageReceived", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().internetMessageReceived(
            requireString(map, "senderId"),
            ubyteList(map, "data"),
        )
        return null
""")

_m("internetGetNextMessage", """
        val message = host.requireProtocolBridge().internetGetNextMessage() ?: return null
        return mapOf(
            "messageId" to message.messageId,
            "recipientId" to message.recipientId,
            "data" to message.data.map { it.toInt() },
        )
""")

_m("internetConfirmSent", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().internetConfirmSent(requireString(map, "messageId"))
        return null
""")

_m("internetSendFailed", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().internetSendFailed(requireString(map, "messageId"))
        return null
""")

_m("checkInternetPresence", """
        val map = mapFromAny(arguments)
        val manager = host.internetManager
            ?: throw IllegalStateException("Internet transport not initialized")
        // React Native nests `force` in `options`; Flutter sends it beside
        // `userId`. Reading only the top level dropped every forced check
        // from React Native in 0.31.0.
        val options = map["options"] as? Map<*, *>
        val force = map["force"] as? Boolean ?: options?.get("force") as? Boolean ?: false
        val latch = java.util.concurrent.CountDownLatch(1)
        var result = false
        manager.checkPresence(requireString(map, "userId"), force) { written ->
            result = written
            latch.countDown()
        }
        latch.await()
        return result
""")

_m("isBluetoothEnabled", """
        val context = ctx ?: throw IllegalStateException("Context required")
        val adapter = (context.getSystemService(Context.BLUETOOTH_SERVICE) as? android.bluetooth.BluetoothManager)?.adapter
        return adapter?.isEnabled == true
""")

_m("requestEnableBluetooth", """
        val context = ctx ?: throw IllegalStateException("Context required")
        val adapter = (context.getSystemService(Context.BLUETOOTH_SERVICE) as? android.bluetooth.BluetoothManager)?.adapter
            ?: throw IllegalStateException("Bluetooth not available")
        if (adapter.isEnabled) return true
        val activity = context as? android.app.Activity
            ?: throw IllegalStateException("No activity available to show Bluetooth enable dialog")
        @Suppress("DEPRECATION")
        activity.startActivityForResult(Intent(BluetoothAdapter.ACTION_REQUEST_ENABLE), 1001)
        return false
""")

_m("deriveAddress", """
        val map = mapFromAny(arguments)
        return uniffi.offline_protocol.deriveAddress(ubyteList(map, "publicKey"))
""")

_m("deriveUserIdFromPublicKey", """
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().deriveUserIdFromPublicKey(ubyteList(map, "publicKey"))
""")

_m("signData", """
        val map = mapFromAny(arguments)
        val sig = host.requireProtocolBridge().signData(ubyteList(map, "data"))
        return sig.map { it.toInt() }
""")

_m("signDataBase64", """
        val map = mapFromAny(arguments)
        val data = Base64.decode(requireString(map, "dataBase64"), Base64.DEFAULT)
        val sig = host.requireProtocolBridge().signData(data.map { it.toUByte() })
        return Base64.encodeToString(sig.map { it.toByte() }.toByteArray(), Base64.NO_WRAP)
""")

_m("verifySignature", """
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().verifySignature(
            ubyteList(map, "publicKey"),
            ubyteList(map, "data"),
            ubyteList(map, "signature"),
        )
""")

_m("verifySignatureBase64", """
        val map = mapFromAny(arguments)
        val pub = Base64.decode(requireString(map, "publicKeyBase64"), Base64.DEFAULT)
        val data = Base64.decode(requireString(map, "dataBase64"), Base64.DEFAULT)
        val sig = Base64.decode(requireString(map, "signatureBase64"), Base64.DEFAULT)
        return host.requireProtocolBridge().verifySignature(
            pub.map { it.toUByte() },
            data.map { it.toUByte() },
            sig.map { it.toUByte() },
        )
""")

_m("parseInvite", """
        val map = mapFromAny(arguments)
        val invite = uniffi.offline_protocol.parseInvite(requireString(map, "blob"))
        return mapOf(
            "address" to invite.address,
            "public_key" to invite.publicKey.map { it.toInt() },
            "petname" to invite.petname,
            "signed" to invite.signed,
        )
""")

_m("createInvite", """
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().createInvite(
            optionalString(map, "petname"),
            map["signed"] as? Boolean ?: false,
        )
""")

_m("resolveUsername", """
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().resolveUsername(requireString(map, "username"))
""")

_m("hasPendingKeyPackage", """
        val map = mapFromAny(arguments)
        return host.protocol?.hasPendingKeyPackage(requireString(map, "peerId")) ?: false
""")

_m("getEstablishmentState", """
        val map = mapFromAny(arguments)
        val proto = host.protocol ?: return "NoKeyPackage"
        return when (proto.getEstablishmentState(requireString(map, "peerId"))) {
            EstablishmentState.NO_KEY_PACKAGE -> "NoKeyPackage"
            EstablishmentState.HAVE_KEY_PACKAGE -> "HaveKeyPackage"
            EstablishmentState.SESSION_PENDING -> "SessionPending"
            EstablishmentState.SESSION_CONFIRMED -> "SessionConfirmed"
        }
""")

_m("establishSecureSession", """
        val map = mapFromAny(arguments)
        val welcome = host.requireProtocolBridge().establishSecureSession(requireString(map, "peerId"))
        return welcome?.let { mlsWelcomeMap(it) }
""")

_m("rekeySession", """
        val map = mapFromAny(arguments)
        return host.requireProtocolBridge().rekeySession(requireString(map, "peerId"))
""")

_m("mlsGenerateKeyPackage", """
        val bundle = host.requireProtocolBridge().mlsGenerateKeyPackage()
        return keyPackageMap(bundle)
""")

_m("mlsGetOrCreateKeyPackage", """
        val bundle = host.requireProtocolBridge().mlsGetOrCreateKeyPackage()
        return keyPackageMap(bundle)
""")

_m("mlsGetPendingKeyPackages", """
        val packages = host.requireProtocolBridge().mlsGetPendingKeyPackages()
        return packages.map { keyPackageMap(it) }
""")

_m("mlsMarkKeyPackageSynced", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().mlsMarkKeyPackageSynced(requireString(map, "packageId"))
        return null
""")

_m("mlsCreateSession", """
        val map = mapFromAny(arguments)
        return mlsWelcomeMap(host.requireProtocolBridge().mlsCreateSession(requireString(map, "otherUserId")))
""")

_m("mlsJoinSession", """
        val map = mapFromAny(arguments)
        val json = JSONObject(requireString(map, "welcomeJson"))
        val info = host.requireProtocolBridge().mlsJoinSession(mlsWelcomeFromJson(json))
        return mlsGroupInfoMap(info)
""")

_m("mlsProcessWelcome", """
        val map = mapFromAny(arguments)
        val json = JSONObject(requireString(map, "welcomeJson"))
        val info = host.requireProtocolBridge().mlsProcessWelcome(mlsWelcomeFromJson(json))
        return mlsGroupInfoMap(info)
""")

_m("mlsDeleteSession", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().mlsDeleteSession(requireString(map, "otherUserId"))
        return null
""")

_m("mlsImportKeyPackage", """
        val map = mapFromAny(arguments)
        host.requireProtocolBridge().mlsImportKeyPackage(requireString(map, "userId"), ubyteList(map, "keyPackageData"))
        return null
""")

_m("mlsHasSession", """
        val map = mapFromAny(arguments)
        return host.protocol?.mlsHasSession(requireString(map, "otherUserId")) ?: false
""")

_m("mlsEncryptForUser", """
        val map = mapFromAny(arguments)
        val encrypted = host.requireProtocolBridge().mlsEncryptForUser(
            requireString(map, "otherUserId"),
            ubyteList(map, "plaintext"),
        )
        return mlsEncryptedMap(encrypted)
""")

_m("mlsDecrypt", """
        val map = mapFromAny(arguments)
        val plaintext = host.requireProtocolBridge().mlsDecrypt(
            mlsEncryptedFromJson(JSONObject(requireString(map, "encryptedJson"))),
        )
        return plaintext?.map { it.toInt() }
""")

_m("mlsDecryptFromUser", """
        val map = mapFromAny(arguments)
        val plaintext = host.requireProtocolBridge().mlsDecryptFromUser(
            mlsEncryptedFromJson(JSONObject(requireString(map, "encryptedJson"))),
        )
        return plaintext?.map { it.toInt() }
""")

_m("mlsListSessions", """
        return host.protocol?.mlsListSessions() ?: emptyList<String>()
""")

_m("wipePersistedState", """
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
""")
