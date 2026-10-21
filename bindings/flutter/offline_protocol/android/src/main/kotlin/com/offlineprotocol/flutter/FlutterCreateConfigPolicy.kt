package com.offlineprotocol.flutter

import org.json.JSONObject

/**
 * Preview policy: Reticulum and Nostr managers compile with the shared bridge,
 * but the Flutter plugin does not host them yet. Refuse before tearing down a
 * running session so a bad config leaves the current engine intact.
 */
object FlutterCreateConfigPolicy {
    const val UNSUPPORTED_TRANSPORT_MESSAGE: String =
        "Reticulum and Nostr are not hosted in the Flutter plugin yet. " +
            "Disable reticulumEnabled and nostrEnabled, or use React Native."

    @JvmStatic
    fun requireCreateConfigSupported(configJson: String) {
        val json = JSONObject(configJson)
        if (json.optBoolean("reticulumEnabled", json.optBoolean("reticulum_enabled", false))) {
            throw IllegalArgumentException(UNSUPPORTED_TRANSPORT_MESSAGE)
        }
        if (json.optBoolean("nostrEnabled", json.optBoolean("nostr_enabled", false))) {
            throw IllegalArgumentException(UNSUPPORTED_TRANSPORT_MESSAGE)
        }
    }

    @JvmStatic
    fun requireTransportTypeSupported(type: String) {
        when (type.lowercase()) {
            "reticulum", "nostr" -> throw IllegalArgumentException(UNSUPPORTED_TRANSPORT_MESSAGE)
        }
    }

    @JvmStatic
    fun requireForceTransportSupported(arguments: Any?) {
        val map = arguments as? Map<*, *> ?: return
        val raw = map["transportType"] as? String ?: map["transport_type"] as? String ?: return
        requireTransportTypeSupported(raw)
    }
}
