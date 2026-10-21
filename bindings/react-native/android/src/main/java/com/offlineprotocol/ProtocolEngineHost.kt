package com.offlineprotocol

import android.content.Context
import android.os.Build
import android.util.Log
import com.offlineprotocol.ble.BleTransportFacade
import org.json.JSONArray
import org.json.JSONObject
import uniffi.offline_protocol.*
import java.net.URI

/**
 * Non-React host for the Offline Protocol engine: shared by React Native and Flutter.
 *
 * Holds the protocol handle, transport managers, mesh services, data store, and the
 * background process scheduler. Wrapper layers supply event and diagnostic callbacks.
 */
class ProtocolEngineHost(
    private val logTag: String,
    private val onDiagnostic: (level: String, message: String, context: Map<String, Any?>) -> Unit,
) {
    @Volatile
    var protocol: OfflineProtocol? = null
    var meshServices: MeshServices? = null
    var dataStore: DataStore? = null
    var bleTransport: BleTransportFacade? = null
    var internetManager: InternetManager? = null
    @Volatile
    var wifiDirectManager: WifiDirectManager? = null
    var reticulumManager: ReticulumManager? = null
    var nostrManager: NostrManager? = null
    var currentConfig: ProtocolConfig? = null

    val backgroundProcessor = ProtocolBackgroundProcessor(
        protocolProvider = { protocol },
        onDiagnostic = onDiagnostic,
        logTag = logTag,
    )

    data class InternetTransportEventHandlers(
        val serverMessageEmitter: (String) -> Unit,
        val connectionStatusEmitter: (Boolean, Boolean) -> Unit,
        val supersededEmitter: (String?) -> Unit,
    )

    fun create(
        context: Context,
        configJson: String,
        eventCallback: EventCallback,
        internetHandlers: InternetTransportEventHandlers?,
    ): ProtocolConfig {
        val parsed = ProtocolConfigParser.parse(configJson)
        val config = parsed.coreConfig
        val proto = OfflineProtocol(config)
        currentConfig = config
        onDiagnostic(
            "info",
            "Protocol core created",
            mapOf(
                "appId" to config.appId,
                "userId" to config.profile,
                "bleEnabled" to config.bleEnabled,
                "wifiDirectEnabled" to config.wifiDirectEnabled,
                "internetEnabled" to config.internetEnabled,
                "reticulumEnabled" to config.reticulumEnabled,
            ),
        )
        proto.setEventCallback(eventCallback)
        InitialRuntimeConfigApplier.apply(proto, parsed.rawJson) { level, message, ctx ->
            onDiagnostic(level, message, ctx ?: emptyMap())
        }
        protocol = proto
        meshServices = MeshServices(proto)
        dataStore = DataStore(proto)
        if (config.bleEnabled) {
            bleTransport = BleTransportFacade(
                context,
                proto,
                config.profile,
                config.appId,
                onDiagnostic,
            ).also { manager ->
                manager.listener = transportListener(manager)
            }
            Log.i(logTag, "BLE Manager initialized for user: ${config.profile}")
            onDiagnostic("info", "BLE manager initialized", mapOf("userId" to config.profile))
        } else {
            onDiagnostic("warning", "BLE disabled in configuration", mapOf("userId" to config.profile))
        }
        if (config.internetEnabled) {
            internetManager = InternetManager(context, proto, config.profile, config.appId, onDiagnostic)
                .also { manager ->
                    internetHandlers?.let { handlers ->
                        manager.serverMessageEmitter = handlers.serverMessageEmitter
                        manager.connectionStatusEmitter = handlers.connectionStatusEmitter
                        manager.supersededEmitter = handlers.supersededEmitter
                    }
                    manager.listener = transportListener(manager)
                }
            Log.i(logTag, "Internet Manager initialized for user: ${config.profile}")
            onDiagnostic("info", "Internet manager initialized", mapOf("userId" to config.profile))
        } else {
            onDiagnostic("info", "Internet disabled in configuration", mapOf("userId" to config.profile))
        }
        if (config.reticulumEnabled) {
            reticulumManager = createReticulumManager(context, proto, config.profile)
            Log.i(logTag, "Reticulum Manager initialized for user: ${config.profile}")
            onDiagnostic("info", "Reticulum manager initialized", mapOf("userId" to config.profile))
        } else {
            onDiagnostic("info", "Reticulum disabled in configuration", mapOf("userId" to config.profile))
        }
        if (config.nostrEnabled) {
            nostrManager = createNostrManager(context, proto)
            Log.i(logTag, "Nostr Manager initialized for user: ${config.profile}")
            onDiagnostic("info", "Nostr manager initialized", mapOf("userId" to config.profile))
        } else {
            onDiagnostic("info", "Nostr disabled in configuration", mapOf("userId" to config.profile))
        }
        wireTransportCallbacks(proto)
        backgroundProcessor.startProcessScheduler()
        onDiagnostic("info", "Protocol process scheduler started", emptyMap())
        return config
    }

    @Synchronized
    fun stopTransportsAndProtocol(onStopForegroundService: () -> Unit) {
        val teardown = TeardownSequence()
        teardown.step("process scheduler") { backgroundProcessor.stopProcessScheduler() }
        teardown.step("BLE manager") {
            bleTransport?.stop()
            Log.i(logTag, "BLE Manager stopped")
            onDiagnostic("info", "BLE manager stopped", emptyMap())
        }
        teardown.step("Internet manager") {
            internetManager?.stop()
            Log.i(logTag, "Internet Manager stopped")
            onDiagnostic("info", "Internet manager stopped", emptyMap())
        }
        teardown.step("WiFi Direct manager") {
            wifiDirectManager?.stop()
            Log.i(logTag, "WiFi Direct Manager stopped")
            onDiagnostic("info", "WiFi Direct manager stopped", emptyMap())
        }
        teardown.step("Reticulum manager") {
            reticulumManager?.stop()
            Log.i(logTag, "Reticulum Manager stopped")
            onDiagnostic("info", "Reticulum manager stopped", emptyMap())
        }
        teardown.step("Nostr manager") {
            nostrManager?.stop()
            Log.i(logTag, "Nostr Manager stopped")
            onDiagnostic("info", "Nostr manager stopped", emptyMap())
        }
        teardown.step("mesh foreground service") { onStopForegroundService() }
        teardown.step("protocol core") {
            protocol?.stop()
            onDiagnostic("info", "Protocol stopped", emptyMap())
        }
        for (failure in teardown.failures) {
            Log.e(logTag, "Teardown step failed: ${failure.step}", failure.cause)
            onDiagnostic(
                "error",
                "Teardown step failed",
                mapOf(
                    "step" to failure.step,
                    "message" to (failure.cause.message ?: "unknown"),
                    "exception" to failure.cause.javaClass.simpleName,
                ),
            )
        }
        teardown.firstFailureOrNull()?.let { throw it }
    }

    fun releaseHandlesAfterStop() {
        bleTransport = null
        internetManager = null
        wifiDirectManager = null
        reticulumManager = null
        nostrManager = null
        val store = dataStore
        dataStore = null
        try {
            store?.destroy()
        } catch (e: Exception) {
            Log.w(logTag, "Releasing the data store handle failed", e)
        }
        val handle = protocol
        protocol = null
        try {
            handle?.destroy()
        } catch (e: Exception) {
            Log.w(logTag, "Releasing the protocol handle failed", e)
        }
        meshServices = null
        currentConfig = null
    }

    fun startProtocolCore() {
        requireProtocol().start()
        onDiagnostic("info", "Protocol core started", emptyMap())
    }

    fun startBleTransportIfPresent() {
        bleTransport?.start()
    }

    fun pause() {
        backgroundProcessor.stopProcessScheduler()
        val teardown = TeardownSequence()
        teardown.step("BLE manager") { bleTransport?.pause() }
        teardown.step("Internet manager") { internetManager?.pause() }
        teardown.step("WiFi Direct manager") { wifiDirectManager?.pause() }
        teardown.step("Reticulum manager") { reticulumManager?.pause() }
        teardown.step("Nostr manager") { nostrManager?.pause() }
        teardown.step("protocol") { protocol?.pause() }
        teardown.firstFailureOrNull()?.let { throw it }
    }

    fun resume() {
        val teardown = TeardownSequence()
        teardown.step("protocol") { protocol?.resume() }
        teardown.step("BLE manager") { bleTransport?.resume() }
        teardown.step("Internet manager") { internetManager?.resume() }
        teardown.step("WiFi Direct manager") { wifiDirectManager?.resume() }
        teardown.step("Reticulum manager") { reticulumManager?.resume() }
        teardown.step("Nostr manager") { nostrManager?.resume() }
        teardown.step("process scheduler") {
            if (protocol?.getState() == ProtocolState.RUNNING) {
                backgroundProcessor.startProcessScheduler()
            }
        }
        teardown.firstFailureOrNull()?.let { throw it }
    }

    fun enableTransport(
        context: Context,
        type: String,
        config: JSONObject?,
        internetHandlers: InternetTransportEventHandlers?,
        onInternetTransportEnabled: (() -> Unit)? = null,
    ) {
        val proto = requireProtocol()
        when (type.lowercase()) {
            "internet" -> {
                if (internetManager == null) {
                    val coreConfig = currentConfig
                        ?: throw IllegalStateException("Cannot enable Internet transport before initialize(config)")
                    internetManager = InternetManager(context, proto, coreConfig.profile, coreConfig.appId, onDiagnostic)
                        .also { manager ->
                            internetHandlers?.let { handlers ->
                                manager.serverMessageEmitter = handlers.serverMessageEmitter
                                manager.connectionStatusEmitter = handlers.connectionStatusEmitter
                                manager.supersededEmitter = handlers.supersededEmitter
                            }
                        }
                    onDiagnostic("info", "Internet manager created on demand", emptyMap())
                }
                val manager = internetManager ?: throw IllegalStateException("Failed to create Internet manager")
                if (manager.state == TransportState.RUNNING || manager.state == TransportState.STARTING) {
                    manager.stop()
                }
                val json = config ?: JSONObject()
                InternetTransportBootstrap.configureAndStart(manager, json)
                onInternetTransportEnabled?.invoke()
                onDiagnostic("info", "Internet transport enabled", emptyMap())
            }
            "wifidirect", "wifi_direct" -> {
                if (wifiDirectManager == null) {
                    wifiDirectManager = WifiDirectManager(
                        context,
                        proto,
                        currentConfig?.profile ?: "unknown",
                        appId = currentConfig?.appId ?: "",
                        onDiagnostic,
                    )
                    onDiagnostic("info", "WiFi Direct manager created on demand", emptyMap())
                }
                val manager = wifiDirectManager ?: throw IllegalStateException("Failed to create WiFi Direct manager")
                val autoAccept = config?.let { json ->
                    if (json.has("autoAccept") && !json.isNull("autoAccept")) json.getBoolean("autoAccept") else null
                }
                if (manager.state == TransportState.RUNNING) {
                    manager.stop()
                }
                manager.formGroups = WifiDirectGroupFormation.formGroupsAfterEnable(manager.formGroups, autoAccept)
                wireTransportCallbacks(proto)
                manager.start()
                onDiagnostic("info", "WiFi Direct transport enabled", emptyMap())
            }
            "ble" -> {
                if (bleTransport == null) {
                    bleTransport = BleTransportFacade(
                        context,
                        proto,
                        currentConfig?.profile ?: "unknown",
                        currentConfig?.appId ?: "",
                        onDiagnostic,
                    )
                    onDiagnostic("info", "BLE manager created on demand", emptyMap())
                }
                val manager = bleTransport ?: throw IllegalStateException("Failed to create BLE manager")
                wireTransportCallbacks(proto)
                if (manager.state != TransportState.RUNNING) {
                    manager.start()
                    onDiagnostic("info", "BLE transport enabled", emptyMap())
                }
            }
            "reticulum" -> {
                if (reticulumManager == null) {
                    reticulumManager = createReticulumManager(context, proto, currentConfig?.profile ?: "unknown")
                    onDiagnostic("info", "Reticulum manager created on demand", emptyMap())
                }
                val manager = reticulumManager ?: throw IllegalStateException("Failed to create Reticulum manager")
                if (manager.state == TransportState.RUNNING) {
                    manager.stop()
                }
                configureAndStartReticulum(manager, config)
                onDiagnostic("info", "Reticulum transport enabled", emptyMap())
            }
            "nostr" -> {
                if (nostrManager == null) {
                    nostrManager = createNostrManager(context, proto)
                    onDiagnostic("info", "Nostr manager created on demand", emptyMap())
                }
                val manager = nostrManager ?: throw IllegalStateException("Failed to create Nostr manager")
                if (manager.state == TransportState.RUNNING) {
                    manager.stop()
                }
                configureAndStartNostr(manager, config)
                wireTransportCallbacks(proto)
                onDiagnostic("info", "Nostr transport enabled", emptyMap())
            }
            else -> throw IllegalArgumentException("Unsupported transport type: $type")
        }
    }

    fun enableTransport(context: Context, type: String, configJson: String?) {
        val json = configJson?.let { JSONObject(it) }
        enableTransport(context, type, json, internetHandlers = null)
    }

    fun wireTransportCallbacks(proto: OfflineProtocol) {
        bleTransport?.let { manager ->
            try {
                proto.setBleTransportCallback(object : BleTransportCallback {
                    override fun onFragmentsAvailable() {
                        manager.onFragmentsAvailable()
                    }
                })
                Log.i(logTag, "BLE transport callback registered")
                onDiagnostic("info", "BLE transport callback wired", emptyMap())
            } catch (e: Throwable) {
                Log.w(logTag, "BLE transport callback not available", e)
                onDiagnostic("warning", "BLE callback wiring skipped (regenerate UniFFI bindings)", emptyMap())
            }
        }
        if (currentConfig?.wifiDirectEnabled == true) {
            try {
                proto.setWifiDirectTransportCallback(object : uniffi.offline_protocol.WifiDirectTransportCallback {
                    override fun onMessagesAvailable() {
                        wifiDirectManager?.onMessagesAvailable()
                    }
                })
                onDiagnostic("info", "WiFi Direct transport callback registered", emptyMap())
            } catch (e: Throwable) {
                Log.w(logTag, "WiFi Direct transport callback not available", e)
                onDiagnostic("warning", "WiFi Direct callback wiring skipped (regenerate UniFFI bindings)", emptyMap())
            }
        }
        reticulumManager?.let { manager ->
            try {
                proto.setReticulumTransportCallback(object : ReticulumTransportCallback {
                    override fun onMessagesAvailable() {
                        manager.onMessagesAvailable()
                    }
                })
                onDiagnostic("info", "Reticulum transport callback wired", emptyMap())
            } catch (e: Throwable) {
                Log.w(logTag, "Reticulum transport callback not available", e)
                onDiagnostic("warning", "Reticulum callback wiring skipped (regenerate UniFFI bindings)", emptyMap())
            }
        }
        nostrManager?.let { manager ->
            try {
                proto.setNostrTransportCallback(object : NostrTransportCallback {
                    override fun onMessagesAvailable() {
                        manager.onMessagesAvailable()
                    }
                })
                onDiagnostic("info", "Nostr transport callback wired", emptyMap())
            } catch (e: Throwable) {
                Log.w(logTag, "Nostr transport callback not available", e)
                onDiagnostic("warning", "Nostr callback wiring skipped (regenerate UniFFI bindings)", emptyMap())
            }
        }
    }

    // --- Telemetry ---

    fun enableTelemetry(config: Map<String, Any?>?, initialAppState: AppState) {
        requireProtocol().enableTelemetry(parseTelemetryConfig(config), initialAppState)
    }

    fun disableTelemetry() {
        requireProtocol().disableTelemetry()
    }

    fun flushTelemetry() {
        protocol?.flushTelemetry()
    }

    fun telemetryStats(): TelemetryStats? = protocol?.telemetryStats()

    fun endTelemetrySession() {
        protocol?.endTelemetrySession()
    }

    fun setTelemetryEnabled(enabled: Boolean) {
        protocol?.setTelemetryEnabled(enabled)
    }

    fun telemetryInstallId(): String? = requireProtocol().telemetryInstallId()

    fun parseTelemetryConfig(map: Map<String, Any?>?): TelemetryConfig {
        val m = map ?: emptyMap()
        val verbosity = (m["mlsVerbosity"] as? String)?.let { raw ->
            when (raw.lowercase()) {
                "off" -> MlsVerbosity.OFF
                "diagnostic" -> MlsVerbosity.DIAGNOSTIC
                "lifecycle" -> MlsVerbosity.LIFECYCLE
                else -> {
                    Log.w(
                        logTag,
                        "telemetry: unknown mlsVerbosity '$raw' — expected 'off', 'lifecycle', or 'diagnostic'.",
                    )
                    null
                }
            }
        }
        return TelemetryConfig(
            apiKey = m["apiKey"] as? String ?: "",
            appId = m["appId"] as? String ?: "",
            os = TelemetryOs.ANDROID,
            osMajor = Build.VERSION.SDK_INT.toUShort(),
            appVersion = m["appVersion"] as? String,
            debug = m["debug"] as? Boolean,
            flushIntervalMs = optionalNonNegativeLong(m["flushIntervalMs"])?.toULong(),
            maxBatchBytes = optionalNonNegativeLong(m["maxBatchBytes"])?.toULong(),
            maxBufferedRecords = optionalNonNegativeLong(m["maxBufferedRecords"])
                ?.coerceAtMost(UInt.MAX_VALUE.toLong())
                ?.toUInt(),
            includeDeviceId = m["includeDeviceId"] as? Boolean,
            scrubIds = m["scrubIds"] as? Boolean,
            mlsVerbosity = verbosity,
            metricsCadenceMs = optionalNonNegativeLong(m["metricsCadenceMs"])?.toULong(),
            routingDiagnostic = m["routingDiagnostic"] as? Boolean,
            mlsSamplingBypass = m["mlsSamplingBypass"] as? Boolean,
        )
    }

    fun telemetryStatsMap(stats: TelemetryStats): Map<String, Any?> = mapOf(
        "buffered" to stats.buffered.toLong().toDouble(),
        "sentEvents" to stats.sentEvents.toLong().toDouble(),
        "acceptedEvents" to stats.acceptedEvents.toLong().toDouble(),
        "dropped" to stats.dropped.toLong().toDouble(),
        "sessionId" to stats.sessionId,
        "lastError" to stats.lastError,
        "lastFlushAtMs" to stats.lastFlushAtMs?.toDouble(),
    )

    // --- Messaging ---

    fun sendMessage(recipient: String, content: String, priority: Int, replyToMsg: String?): String {
        val msgPriority = messagePriorityFromInt(priority)
        return requireProtocol().sendMessage(recipient, content, msgPriority, replyToMsg)
    }

    fun sendMessageRich(recipient: String, content: String, options: SendMessageOptions): String =
        requireProtocol().sendMessageRich(recipient, content, options)

    fun receiveMessage(): String? = protocol?.receiveMessage()

    // --- Data store ---

    private fun requireDataStore(): DataStore =
        dataStore ?: throw IllegalStateException("DataStore not initialized")

    fun dataCreateDoc(spaceId: String, docId: String) = requireDataStore().createDoc(spaceId, docId)
    fun dataDeleteDoc(spaceId: String, docId: String) = requireDataStore().deleteDoc(spaceId, docId)
    fun dataRemoveDoc(spaceId: String, docId: String) = requireDataStore().removeDoc(spaceId, docId)
    fun dataRemoveSpace(spaceId: String) = requireDataStore().removeSpace(spaceId)
    fun dataFetchAttachmentFrom(spaceId: String, peerId: String, hash: String) =
        requireDataStore().fetchAttachmentFrom(spaceId, peerId, hash)
    fun dataSetInterest(spaceId: String, patterns: List<String>) =
        requireDataStore().setInterest(spaceId, patterns)
    fun dataListDocs(spaceId: String): String = JSONArray(requireDataStore().listDocs(spaceId)).toString()
    fun dataListSpaces(): String = JSONArray(requireDataStore().listSpaces()).toString()
    fun dataMapSet(spaceId: String, docId: String, collection: String, key: String, valueJson: String) =
        requireDataStore().mapSet(spaceId, docId, collection, key, valueJson)
    fun dataMapDelete(spaceId: String, docId: String, collection: String, key: String) =
        requireDataStore().mapDelete(spaceId, docId, collection, key)
    fun dataMapGetJson(spaceId: String, docId: String, collection: String, key: String) =
        requireDataStore().mapGetJson(spaceId, docId, collection, key)
    fun dataListPush(spaceId: String, docId: String, collection: String, valueJson: String) =
        requireDataStore().listPush(spaceId, docId, collection, valueJson)
    fun dataListDelete(spaceId: String, docId: String, collection: String, index: Double, count: Double) =
        requireDataStore().listDelete(spaceId, docId, collection, index.toUInt(), count.toUInt())
    fun dataListLen(spaceId: String, docId: String, collection: String): Double =
        requireDataStore().listLen(spaceId, docId, collection).toDouble()
    fun dataTextInsert(spaceId: String, docId: String, collection: String, position: Double, text: String) =
        requireDataStore().textInsert(spaceId, docId, collection, position.toUInt(), text)
    fun dataTextDelete(spaceId: String, docId: String, collection: String, position: Double, count: Double) =
        requireDataStore().textDelete(spaceId, docId, collection, position.toUInt(), count.toUInt())
    fun dataTextValue(spaceId: String, docId: String, collection: String) =
        requireDataStore().textValue(spaceId, docId, collection)
    fun dataCounterIncrement(spaceId: String, docId: String, collection: String, amount: Double) =
        requireDataStore().counterIncrement(spaceId, docId, collection, amount)
    fun dataCounterValue(spaceId: String, docId: String, collection: String) =
        requireDataStore().counterValue(spaceId, docId, collection)
    fun dataDocJson(spaceId: String, docId: String) = requireDataStore().docJson(spaceId, docId)
    fun dataExportRaw(spaceId: String, docId: String) = requireDataStore().exportRaw(spaceId, docId)
    fun dataFlush(spaceId: String, docId: String) = requireDataStore().flush(spaceId, docId)
    fun dataFlushAll() = requireDataStore().flushAll()
    fun dataDocSize(spaceId: String, docId: String): Double =
        requireDataStore().docSize(spaceId, docId).toLong().toDouble()
    fun dataAttachmentHash(bytesBase64: String): String {
        val bytes = android.util.Base64.decode(bytesBase64, android.util.Base64.DEFAULT)
        return requireDataStore().attachmentHash(bytes)
    }
    fun dataFetchAttachment(spaceId: String, hash: String) = requireDataStore().fetchAttachment(spaceId, hash)
    fun dataProvideAttachment(spaceId: String, peerId: String, hash: String, bytesBase64: String) {
        val bytes = android.util.Base64.decode(bytesBase64, android.util.Base64.DEFAULT)
        requireDataStore().provideAttachment(spaceId, peerId, hash, bytes)
    }
    fun dataDeclineAttachment(spaceId: String, peerId: String, hash: String) =
        requireDataStore().declineAttachment(spaceId, peerId, hash)
    fun dataWipeAll() = requireDataStore().wipeAll()

    // --- Mesh / MLS groups ---

    fun meshCreateGroup(name: String): Map<String, Any?> {
        val info = requireProtocol().createGroup(name)
        return groupInfoMap(info)
    }

    fun meshInviteToGroup(groupId: String, inviteeUserId: String) =
        requireProtocol().inviteToGroup(groupId, inviteeUserId)

    fun meshSendGroupMessage(groupId: String, content: String, priority: String?, replyToMsg: String?): List<String> {
        val msgPriority = priority?.let { messagePriorityFromString(it) }
        return requireProtocol().sendGroupMessage(groupId, content, msgPriority, replyToMsg)
    }

    fun meshForwardMessageToGroup(originalMessageJson: String, groupId: String, priority: String?): List<String> {
        val msgPriority = priority?.let { messagePriorityFromString(it) }
        return requireProtocol().forwardMessageToGroup(originalMessageJson, groupId, msgPriority)
    }

    fun meshRemoveFromGroup(groupId: String, memberId: String) =
        requireProtocol().removeFromGroup(groupId, memberId)

    fun meshLeaveGroup(groupId: String) = requireProtocol().leaveGroup(groupId)

    fun meshListGroups(): List<String> = requireProtocol().listGroups()

    fun meshGetGroupInfo(groupId: String): Map<String, Any?>? =
        requireProtocol().getGroupInfo(groupId)?.let { groupInfoMap(it) }

    fun meshGroupRichReadiness(groupId: String): Map<String, Any?> {
        val readiness = requireProtocol().groupRichReadiness(groupId)
        return mapOf(
            "ready" to readiness.ready,
            "unknownMembers" to readiness.unknownMembers,
        )
    }

    fun meshGroupRelaySyncState(groupId: String): String =
        requireProtocol().groupRelaySyncState(groupId).name.lowercase()

    fun meshRequestGroupRelayRegistration(groupId: String): Boolean =
        requireProtocol().requestGroupRelayRegistration(groupId)

    fun meshSetMemberRole(groupId: String, userId: String, role: String) =
        requireProtocol().setMemberRole(groupId, userId, role)

    fun meshGetMemberRole(groupId: String, userId: String): String? =
        requireProtocol().getMemberRole(groupId, userId)

    fun meshGetGroupRoles(groupId: String): Map<String, String> = requireProtocol().getGroupRoles(groupId)

    fun meshRenameGroup(groupId: String, newName: String) = requireProtocol().renameGroup(groupId, newName)

    fun initializeMlsWithSecureStorage(context: Context) {
        val proto = requireProtocol()
        val config = currentConfig ?: throw IllegalStateException("Protocol config not initialized")
        val accountNamespace = StorageNamespace.account(config.appId, config.profile)
        val secureStorage = MlsSecureStorage(context, accountNamespace)
        val protocolStateStorage = AppContainerProtocolStateStorage(context, accountNamespace)
        proto.initializeMls(secureStorage, protocolStateStorage)
    }

    fun requireProtocolBridge(): OfflineProtocol = requireProtocol()

    fun requireMeshServicesBridge(): MeshServices =
        meshServices ?: throw IllegalStateException("MeshServices not initialized")

    private fun requireProtocol(): OfflineProtocol =
        protocol ?: throw IllegalStateException("Protocol not initialized")

    private fun groupInfoMap(info: MlsGroupInfo): Map<String, Any?> = mapOf(
        "groupId" to info.groupId,
        "name" to info.name,
        "members" to info.members,
        "epoch" to info.epoch.toDouble(),
        "isSession" to info.isSession,
        "createdAtMs" to info.createdAtMs.toDouble(),
        "lastActivityMs" to info.lastActivityMs.toDouble(),
    )

    private fun messagePriorityFromInt(priority: Int): MessagePriority = when (priority) {
        0 -> MessagePriority.LOW
        1 -> MessagePriority.MEDIUM
        2 -> MessagePriority.HIGH
        3 -> MessagePriority.CRITICAL
        else -> MessagePriority.MEDIUM
    }

    private fun messagePriorityFromString(priority: String): MessagePriority? = when (priority.lowercase()) {
        "low" -> MessagePriority.LOW
        "medium" -> MessagePriority.MEDIUM
        "high" -> MessagePriority.HIGH
        "critical" -> MessagePriority.CRITICAL
        else -> null
    }

    private fun optionalNonNegativeLong(value: Any?): Long? {
        val number = (value as? Number)?.toLong() ?: return null
        if (number < 0L) {
            Log.w(logTag, "telemetry: ignoring negative value ($number)")
            return null
        }
        return number
    }

    private fun transportListener(manager: TransportManager): TransportManagerListener =
        object : TransportManagerListener {
            override fun onTransportStateChanged(manager: TransportManager, state: TransportState) {
                onDiagnostic(
                    "info",
                    "${manager.transportId} transport state changed",
                    mapOf("transport" to manager.transportId, "state" to state.name.lowercase()),
                )
            }

            override fun onTransportError(manager: TransportManager, error: Throwable) {
                onDiagnostic(
                    "error",
                    "${manager.transportId} transport error",
                    mapOf(
                        "transport" to manager.transportId,
                        "message" to (error.message ?: "unknown"),
                        "exception" to error.javaClass.simpleName,
                    ),
                )
            }

            override fun onTransportDiagnostic(
                manager: TransportManager,
                level: String,
                message: String,
                context: Map<String, Any?>,
            ) {
                val enriched = context.toMutableMap()
                enriched["transport"] = manager.transportId
                onDiagnostic(level, message, enriched)
            }
        }

    private fun createReticulumManager(context: Context, proto: OfflineProtocol, userId: String): ReticulumManager =
        ReticulumManager(context, proto, userId, onDiagnostic).also { manager ->
            manager.listener = transportListener(manager)
        }

    private fun configureAndStartReticulum(manager: ReticulumManager, config: JSONObject?) {
        val daemonAddress = config?.optStringCompat("daemonAddress", "daemon_address") ?: "localhost:4242"
        val autoReconnect = config?.optBooleanCompat("autoReconnect", "auto_reconnect") ?: true
        val maxRetries = config?.optIntCompat("maxReconnectAttempts", "max_reconnect_attempts") ?: 0
        manager.configure(daemonAddress, autoReconnect, maxRetries)
        manager.start()
        onDiagnostic(
            "info",
            "Reticulum transport enabled",
            mapOf("daemonAddress" to daemonAddress, "autoReconnect" to autoReconnect),
        )
    }

    private fun createNostrManager(context: Context, proto: OfflineProtocol): NostrManager =
        NostrManager(context, proto, onDiagnostic).also { manager ->
            manager.listener = transportListener(manager)
        }

    private fun configureAndStartNostr(manager: NostrManager, config: JSONObject?) {
        val relayUrls = mutableListOf<String>()
        config?.optJSONArrayCompat("relayUrls", "relay_urls")?.let { arr ->
            for (i in 0 until arr.length()) {
                arr.optString(i)?.takeIf { it.isNotEmpty() }?.let { relayUrls.add(it) }
            }
        }
        if (relayUrls.isEmpty()) {
            throw IllegalArgumentException("Nostr transport requires at least one relay URL")
        }
        val autoReconnect = config?.optBooleanCompat("autoReconnect", "auto_reconnect") ?: true
        val maxRetries = config?.optIntCompat("maxReconnectAttempts", "max_reconnect_attempts") ?: 0
        manager.configure(relayUrls, autoReconnect, maxRetries)
        manager.start()
        onDiagnostic("info", "Nostr transport enabled", mapOf("relayCount" to relayUrls.size))
    }
}
