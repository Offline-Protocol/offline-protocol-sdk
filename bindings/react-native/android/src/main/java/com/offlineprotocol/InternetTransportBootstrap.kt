package com.offlineprotocol

import android.util.Log
import org.json.JSONObject
import java.net.URI

/**
 * Shared Internet transport wiring for host wrappers (Flutter, and optionally RN).
 *
 * Mirrors the enable/configure path in [OfflineProtocolModule.configureAndStartInternet]
 * and the config extraction done during create on iOS. ADR 0025: one home for
 * bridge behaviour that must stay aligned across wrappers.
 */
internal object InternetTransportBootstrap {
    private const val TAG = "InternetTransportBootstrap"

    data class StoredInternetConfig(
        val serverAddress: String?,
        val autoReconnect: Boolean = true,
    )

    /**
     * Reads `transports.internet` (and legacy flat keys) from a create() JSON blob.
     */
    fun readStoredConfig(raw: JSONObject): StoredInternetConfig {
        val internet = raw.optJSONObject("transports")?.optJSONObject("internet")
        val server = internet?.optStringCompat("serverAddress", "server_address")
            ?: raw.optStringCompat("internetServerAddress", "internet_server_address")
        val autoReconnect = internet?.optBooleanCompat("autoReconnect", "auto_reconnect")
            ?: raw.optBooleanCompat("internetAutoReconnect", "internet_auto_reconnect")
            ?: true
        return StoredInternetConfig(server?.trim()?.takeIf { it.isNotEmpty() }, autoReconnect)
    }

    fun createManager(
        context: android.content.Context,
        proto: uniffi.offline_protocol.OfflineProtocol,
        profile: String,
        appId: String,
        onDiagnostic: ((String, String, Map<String, Any?>) -> Unit)? = null,
    ): InternetManager =
        InternetManager(context, proto, profile, appId, onDiagnostic)

    /**
     * Attaches relay-side emitters that become `eventJson` payloads on the host event stream.
     */
    fun attachEmitters(manager: InternetManager, emitEventJson: (String) -> Unit) {
        manager.serverMessageEmitter = { rawJson ->
            try {
                val json = JSONObject()
                json.put("type", "internet_server_message")
                json.put("json", rawJson)
                emitEventJson(json.toString())
            } catch (e: Exception) {
                Log.e(TAG, "Failed to build server message event", e)
            }
        }
        manager.connectionStatusEmitter = { connected, authenticated ->
            try {
                val json = JSONObject()
                json.put("type", "internet_status_changed")
                json.put("connected", connected)
                json.put("authenticated", authenticated)
                emitEventJson(json.toString())
            } catch (e: Exception) {
                Log.e(TAG, "Failed to build internet status event", e)
            }
        }
        manager.supersededEmitter = { reason ->
            try {
                emitEventJson(SupersededLatchPolicy.eventJson(reason))
            } catch (e: Exception) {
                Log.e(TAG, "Failed to build internet superseded event", e)
            }
        }
    }

    /**
     * Configures and starts [manager] from an enable-transport config object (camelCase or snake_case).
     */
    fun configureAndStart(manager: InternetManager, config: JSONObject) {
        val serverAddress = config.optStringCompat("serverAddress", "server_url", "server_address")
            ?: throw IllegalArgumentException("Internet transport requires a serverAddress")

        var wsUrl = serverAddress.trim()
        if (!wsUrl.startsWith("ws://") && !wsUrl.startsWith("wss://")) {
            wsUrl = "wss://$wsUrl"
        }

        val port = when {
            config.has("port") && !config.isNull("port") -> config.optDouble("port").toInt()
            config.has("serverPort") && !config.isNull("serverPort") -> config.optDouble("serverPort").toInt()
            else -> null
        }
        if (port != null && port > 0) {
            val uri = URI(wsUrl)
            if (uri.port == -1) {
                wsUrl = "$wsUrl:$port"
            }
        }

        val autoReconnect = config.optBooleanCompat("autoReconnect", "auto_reconnect") ?: true
        val maxRetries = if (config.has("maxReconnectAttempts") && !config.isNull("maxReconnectAttempts")) {
            config.optDouble("maxReconnectAttempts").toInt()
        } else {
            0
        }

        config.optStringCompat("authToken", "auth_token")?.let { manager.setAuthToken(it) }

        if (manager.state == TransportState.RUNNING || manager.state == TransportState.STARTING) {
            manager.stop()
        }

        manager.configure(wsUrl, autoReconnect, maxRetries)
        manager.start()
    }

    /**
     * Starts Internet when [stored] carries a server URL (same gate as iOS start()).
     */
    fun startIfConfigured(manager: InternetManager, stored: StoredInternetConfig) {
        val url = stored.serverAddress ?: return
        val config = JSONObject()
        config.put("serverAddress", url)
        config.put("autoReconnect", stored.autoReconnect)
        configureAndStart(manager, config)
    }
}
