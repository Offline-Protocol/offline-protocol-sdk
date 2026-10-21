import Foundation

/// Shared Internet transport wiring for host wrappers (Flutter). ADR 0025.
///
/// React Native still inlines `configureAndStartInternet` in
/// `OfflineProtocolModule.swift`; a follow-up should delegate here. CI guards:
/// `internet_transport_bootstrap_swift_keeps_rn_configure_invariants` and the
/// Android counterpart in `offline-protocol-uniffi`.
enum InternetTransportBootstrap {
    static func attachEmitters(
        manager: InternetManager,
        emitEventJson: @escaping (String) -> Void
    ) {
        manager.serverMessageEmitter = { rawJson in
            let payload: [String: Any] = [
                "type": "internet_server_message",
                "json": rawJson,
            ]
            if let data = try? JSONSerialization.data(withJSONObject: payload),
               let json = String(data: data, encoding: .utf8) {
                emitEventJson(json)
            }
        }
        manager.connectionStatusEmitter = { connected, authenticated in
            let payload: [String: Any] = [
                "type": "internet_status_changed",
                "connected": connected,
                "authenticated": authenticated,
            ]
            if let data = try? JSONSerialization.data(withJSONObject: payload),
               let json = String(data: data, encoding: .utf8) {
                emitEventJson(json)
            }
        }
        manager.supersededEmitter = { reason in
            emitEventJson(SupersededLatchPolicy.eventJson(reason: reason))
        }
    }

    static func configureAndStart(manager: InternetManager, config: [String: Any]) throws {
        guard let serverAddress = (config["serverAddress"] as? String)
            ?? (config["server_address"] as? String)
            ?? (config["server_url"] as? String),
            !serverAddress.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw NSError(
                domain: "OfflineProtocol",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Internet transport requires a serverAddress"]
            )
        }

        var wsUrl = serverAddress.trimmingCharacters(in: .whitespacesAndNewlines)
        if !wsUrl.hasPrefix("ws://") && !wsUrl.hasPrefix("wss://") {
            wsUrl = "wss://\(wsUrl)"
        }

        if let port = config["port"] as? NSNumber ?? config["serverPort"] as? NSNumber,
           port.intValue > 0,
           let url = URL(string: wsUrl),
           url.port == nil {
            wsUrl = "\(wsUrl):\(port.intValue)"
        }

        let autoReconnect = (config["autoReconnect"] as? Bool)
            ?? (config["auto_reconnect"] as? Bool)
            ?? true
        let maxRetries = (config["maxReconnectAttempts"] as? NSNumber)?.intValue ?? 0

        if let token = config["authToken"] as? String ?? config["auth_token"] as? String {
            manager.setAuthToken(token)
        }

        if manager.state == .running || manager.state == .starting {
            manager.stop()
        }

        try manager.configure(serverUrl: wsUrl, autoReconnect: autoReconnect, maxReconnectAttempts: maxRetries)
        try manager.start()
    }
}
