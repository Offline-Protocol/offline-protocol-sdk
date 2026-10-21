package com.offlineprotocol

import android.util.Log
import org.json.JSONObject
import java.net.URI

/**
 * Shared Internet transport wiring for host wrappers (Flutter, and optionally RN).
 *
 * Mirrors the enable/configure path in [OfflineProtocolModule.configureAndStartInternet].
 * ADR 0025: one home for bridge behaviour that must stay aligned across
 * wrappers. React Native still inlines its path today; a follow-up should
 * delegate here. CI guard:
 * `internet_transport_bootstrap_keeps_rn_configure_invariants` in
 * `offline-protocol-uniffi`.
 */
internal object InternetTransportBootstrap {
    private const val TAG = "InternetTransportBootstrap"

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
}
