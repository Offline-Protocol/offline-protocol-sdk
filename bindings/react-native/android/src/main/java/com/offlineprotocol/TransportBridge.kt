package com.offlineprotocol

import org.json.JSONArray
import org.json.JSONObject
import uniffi.offline_protocol.MessageStats
import uniffi.offline_protocol.NetworkTopology
import uniffi.offline_protocol.TransportType

/** Shared transport naming and topology JSON encoding for native bridges. */
object TransportBridge {
    private const val MS_PER_SEC = 1000L

    fun mapTransportType(type: String): TransportType = when (type.lowercase()) {
        "internet" -> TransportType.INTERNET
        "ble" -> TransportType.BLE
        "wifidirect", "wifi_direct" -> TransportType.WI_FI_DIRECT
        "reticulum" -> TransportType.RETICULUM
        "nostr" -> TransportType.NOSTR
        else -> throw IllegalArgumentException("Unsupported transport type: $type")
    }

    fun buildTopologyJson(topology: NetworkTopology, localAddress: String?): String {
        val linksArray = JSONArray()
        val nodesArray = JSONArray()
        val connectionCounts = mutableMapOf<String, Int>()
        val transportsByNode = mutableMapOf<String, MutableSet<String>>()

        topology.links.forEach { link ->
            val transportName = normalizeTransportName(link.transport)
            val linkObj = JSONObject().apply {
                put("from", link.sourceId)
                put("to", link.targetId)
                put("quality", link.quality.toDouble())
                put("transport", transportName)
                put("rssi", JSONObject.NULL)
            }
            linksArray.put(linkObj)

            transportsByNode.getOrPut(link.sourceId) { mutableSetOf() }.add(transportName)
            transportsByNode.getOrPut(link.targetId) { mutableSetOf() }.add(transportName)

            connectionCounts[link.sourceId] = (connectionCounts[link.sourceId] ?: 0) + 1
            connectionCounts[link.targetId] = (connectionCounts[link.targetId] ?: 0) + 1
        }

        topology.nodes.forEach { node ->
            val transportsArray = JSONArray()
            transportsByNode[node.nodeId]?.forEach { transportsArray.put(it) }

            val reportedConnections = node.connectionCount.toInt()
            val nodeObj = JSONObject().apply {
                put("user_id", node.nodeId)
                put("role", node.role.lowercase())
                put(
                    "connection_count",
                    if (reportedConnections > 0) reportedConnections else (connectionCounts[node.nodeId] ?: 0),
                )
                put("battery_level", node.batteryLevel?.toInt() ?: JSONObject.NULL)
                put("last_seen", node.lastSeenMs.toLong() / MS_PER_SEC)
                put("transports", transportsArray)
            }
            nodesArray.put(nodeObj)
        }

        val avgQuality = if (topology.links.isNotEmpty()) {
            topology.links.map { it.quality }.average()
        } else {
            0.0
        }

        val statsObj = JSONObject().apply {
            put("total_nodes", topology.nodes.size)
            put("relay_nodes", topology.nodes.count { it.role.equals("relay", true) })
            put("total_connections", topology.links.size)
            put("avg_link_quality", avgQuality)
            put("network_diameter", JSONObject.NULL)
        }

        val root = JSONObject().apply {
            put("timestamp", System.currentTimeMillis() / MS_PER_SEC)
            put("local_user_id", localAddress ?: "")
            put("nodes", nodesArray)
            put("links", linksArray)
            put("stats", statsObj)
        }

        return root.toString()
    }

    fun buildMessageStatsJson(stats: List<MessageStats>): String {
        val array = JSONArray()
        stats.forEach { stat ->
            val obj = JSONObject().apply {
                put("message_id", stat.messageId)
                put("sender", JSONObject.NULL)
                put("recipient", JSONObject.NULL)
                put("sent_at", stat.sentAtMs.toLong())
                if (stat.deliveredAtMs != null) {
                    put("delivered_at", stat.deliveredAtMs!!.toLong())
                    put("latency_ms", (stat.deliveredAtMs!!.toLong() - stat.sentAtMs.toLong()).coerceAtLeast(0))
                } else {
                    put("delivered_at", JSONObject.NULL)
                    put("latency_ms", JSONObject.NULL)
                }
                put("hop_count", stat.hopCount.toInt())
                put("transport", JSONObject.NULL)
                put("retry_count", 0)
                put("status", stat.status)
            }
            array.put(obj)
        }
        return array.toString()
    }

    private fun normalizeTransportName(name: String): String = when (name.lowercase()) {
        "ble" -> "ble"
        "internet" -> "internet"
        "wifi_direct", "wifidirect", "wi_fi_direct" -> "wifiDirect"
        else -> name.lowercase()
    }
}
