//
// TransportBridge.swift
//
// Shared transport naming and topology JSON encoding for native bridges.
// Mirrors android/ TransportBridge.kt: keep the JSON keys, the transport name
// normalisation and the field fallbacks in sync. Used by Flutter and React
// Native through ProtocolEngineHostBridge. No React import: the Swift package
// assembly refuses one.
//

import Foundation

enum TransportBridge {
    private static let msPerSec: UInt64 = 1000

    static func mapTransportType(_ type: String) throws -> TransportType {
        switch type.lowercased() {
        case "internet": return .internet
        case "ble": return .ble
        case "wifidirect", "wifi_direct": return .wiFiDirect
        case "reticulum": return .reticulum
        case "nostr": return .nostr
        default: throw HostError.invalidArgument("Unsupported transport type: \(type)")
        }
    }

    static func buildTopologyJson(_ topology: NetworkTopology, localAddress: String?) throws -> String {
        var connectionCounts: [String: Int] = [:]
        var transportsByNode: [String: [String]] = [:]

        func note(_ transport: String, for node: String) {
            var names = transportsByNode[node] ?? []
            if !names.contains(transport) {
                names.append(transport)
            }
            transportsByNode[node] = names
        }

        var links: [[String: Any]] = []
        var qualitySum = 0.0
        for link in topology.links {
            let transportName = normalizeTransportName(link.transport)
            links.append([
                "from": link.sourceId,
                "to": link.targetId,
                "quality": Double(link.quality),
                "transport": transportName,
                "rssi": NSNull(),
            ])
            qualitySum += Double(link.quality)
            note(transportName, for: link.sourceId)
            note(transportName, for: link.targetId)
            connectionCounts[link.sourceId, default: 0] += 1
            connectionCounts[link.targetId, default: 0] += 1
        }

        let nodes: [[String: Any]] = topology.nodes.map { node in
            let reported = Int(node.connectionCount)
            return [
                "user_id": node.nodeId,
                "role": node.role.lowercased(),
                "connection_count": reported > 0 ? reported : (connectionCounts[node.nodeId] ?? 0),
                "battery_level": node.batteryLevel.map { Int($0) } ?? NSNull(),
                "last_seen": Int(node.lastSeenMs / msPerSec),
                "transports": transportsByNode[node.nodeId] ?? [],
            ]
        }

        let averageQuality = topology.links.isEmpty ? 0.0 : qualitySum / Double(topology.links.count)
        let stats: [String: Any] = [
            "total_nodes": topology.nodes.count,
            "relay_nodes": topology.nodes.filter { $0.role.lowercased() == "relay" }.count,
            "total_connections": topology.links.count,
            "avg_link_quality": averageQuality,
            "network_diameter": NSNull(),
        ]
        let root: [String: Any] = [
            "timestamp": Int(Date().timeIntervalSince1970),
            "local_user_id": localAddress ?? "",
            "nodes": nodes,
            "links": links,
            "stats": stats,
        ]
        return try encode(root)
    }

    static func buildMessageStatsJson(_ stats: [MessageStats]) throws -> String {
        let array: [[String: Any]] = stats.map { stat in
            var entry: [String: Any] = [
                "message_id": stat.messageId,
                "sender": NSNull(),
                "recipient": NSNull(),
                "sent_at": Int(clamping: stat.sentAtMs),
                "hop_count": Int(stat.hopCount),
                "transport": NSNull(),
                "retry_count": 0,
                "status": stat.status,
            ]
            if let delivered = stat.deliveredAtMs {
                entry["delivered_at"] = Int(clamping: delivered)
                entry["latency_ms"] = Int(clamping: delivered >= stat.sentAtMs ? delivered - stat.sentAtMs : 0)
            } else {
                entry["delivered_at"] = NSNull()
                entry["latency_ms"] = NSNull()
            }
            return entry
        }
        return try encode(array)
    }

    static func normalizeTransportName(_ name: String) -> String {
        switch name.lowercased() {
        case "ble": return "ble"
        case "internet": return "internet"
        case "wifi_direct", "wifidirect", "wi_fi_direct": return "wifiDirect"
        default: return name.lowercased()
        }
    }

    private static func encode(_ object: Any) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: object, options: [])
        guard let json = String(data: data, encoding: .utf8) else {
            throw HostError.illegalState("Failed to encode JSON")
        }
        return json
    }
}
