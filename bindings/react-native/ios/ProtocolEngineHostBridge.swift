import CoreBluetooth
import Foundation


/// Per-method bodies for `FlutterEngineChannel`. The Swift counterpart of
/// Android's `ProtocolEngineHostBridge.kt` and `EngineMethodHandlers.kt`:
/// arguments are the Dart-side maps from `mesh_sdk.dart` (named keys,
/// camelCase), results are plain values the standard codec carries.
///
/// One `static func` per name in `method_channel_methods.txt`, with the one
/// signature, so the dispatcher and `tools/check_swift_protocol_engine_host_bridge.py`
/// can hold the two to each other. Every method does its work; none
/// is a placeholder. Methods are called on the plugin's engine queue
/// and may block it (a few wait for a transport's completion), never main.
///
/// Result shapes follow the generated UniFFI records the iOS bridge compiles
/// against, as `OfflineProtocolModule.swift` does, and the Android bridge's
/// choices where the platforms agree: 64-bit counters as doubles, bytes as
/// lists of ints, JSON-valued results as strings.
enum ProtocolEngineHostBridge {
    private typealias A = BridgeArgumentMaps

    /// How long a call waits for a transport to report whether it wrote a
    /// frame. A forced presence check parks for up to eight seconds inside the
    /// manager; this is the margin beyond that, after which the answer is "no".
    private static let completionWaitSeconds: TimeInterval = 12

    // MARK: - Connection requests and invites

    static func acceptConnectionRequest(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "acceptConnectionRequest")
        let keyPackage = try A.byteList(m, "keyPackage")
        return try host.requireProtocol().acceptConnectionRequest(
            recipient: try A.requireString(m, "recipient"),
            accepterName: try A.requireString(m, "accepterName"),
            keyPackage: keyPackage.isEmpty ? nil : keyPackage
        )
    }

    static func cancelConnectionRequest(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "cancelConnectionRequest")
        return try host.requireProtocol().cancelConnectionRequest(recipient: try A.requireString(m, "recipient"))
    }

    static func rejectConnectionRequest(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "rejectConnectionRequest")
        return try host.requireProtocol().rejectConnectionRequest(recipient: try A.requireString(m, "recipient"))
    }

    static func sendConnectionRequest(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "sendConnectionRequest")
        let keyPackage = try A.byteList(m, "keyPackage")
        return try host.requireProtocol().sendConnectionRequest(
            recipient: try A.requireString(m, "recipient"),
            senderName: try A.requireString(m, "senderName"),
            keyPackage: keyPackage.isEmpty ? nil : keyPackage,
            initialMessage: A.optionalString(m, "initialMessage")
        )
    }

    static func createInvite(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "createInvite")
        return try host.requireProtocol().createInvite(
            petname: A.optionalString(m, "petname"),
            sign: A.optionalBool(m, "signed") ?? false
        )
    }

    static func parseInvite(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "parseInvite")
        let invite = try uniffiParseInvite(blob: try A.requireString(m, "blob"))
        return [
            "address": invite.address,
            "public_key": A.ints(invite.publicKey),
            "petname": invite.petname ?? NSNull(),
            "signed": invite.signed,
        ] as [String: Any]
    }

    static func resolveUsername(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "resolveUsername")
        return try host.requireProtocol().resolveUsername(username: try A.requireString(m, "username"))
    }

    // MARK: - BLE

    static func bleGetDiagnostics(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let proto = host.protocolInstance
        return [
            "fragmentFallbacks": Double(proto?.bleFragmentFallbackCount() ?? 0),
            "recipientNotAmongPeers": Double(proto?.bleRecipientNotAmongPeersCount() ?? 0),
            "undersizedMtuReports": Double(proto?.bleUndersizedMtuReports() ?? 0),
        ] as [String: Any]
    }

    static func bleGetPeerCount(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        Int(host.protocolInstance?.bleGetPeerCount() ?? 0)
    }

    static func isBluetoothEnabled(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        // iOS shows its own prompt when BLE is first used, and a manager that
        // has not asked yet reports `unknown`: treated as enabled, as the
        // React Native module does, so the app does not show a manual prompt
        // for a radio the system has not been asked about.
        switch host.bleManager?.bluetoothState ?? .unknown {
        case .poweredOff, .unauthorized, .unsupported:
            return false
        default:
            return true
        }
    }

    static func requestEnableBluetooth(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        // iOS never enables Bluetooth programmatically. False tells the app to
        // show its own prompt; the system dialog appears when BLE is used.
        false
    }

    // MARK: - Blocking, files, presence

    static func blockUser(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "blockUser")
        try host.requireProtocol().blockUser(userId: try A.requireString(m, "userId"))
        return nil
    }

    static func unblockUser(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "unblockUser")
        try host.requireProtocol().unblockUser(userId: try A.requireString(m, "userId"))
        return nil
    }

    static func getBlockedUsers(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.requireProtocol().getBlockedUsers()
    }

    static func isUserBlocked(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "isUserBlocked")
        return try host.requireProtocol().isUserBlocked(userId: try A.requireString(m, "userId"))
    }

    static func cancelFileTransfer(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "cancelFileTransfer")
        try host.requireProtocol().cancelFileTransfer(fileId: try A.requireString(m, "fileId"))
        return nil
    }

    static func finalizeFile(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "finalizeFile")
        try host.requireProtocol().finalizeFile(fileId: try A.requireString(m, "fileId"))
        return nil
    }

    static func getFileProgress(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "getFileProgress")
        guard let progress = try host.requireProtocol().getFileProgress(fileId: try A.requireString(m, "fileId")) else {
            return nil
        }
        return [
            "file_id": progress.fileId,
            "chunks_sent": Int(progress.chunksSent),
            "total_chunks": Int(progress.totalChunks),
            "percentage": Int(progress.percentage),
        ] as [String: Any]
    }

    static func processFileChunk(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "processFileChunk")
        try host.requireProtocol().processFileChunk(
            fileId: try A.requireString(m, "fileId"),
            chunkIndex: try A.requireUInt32(m, "chunkIndex"),
            totalChunks: try A.requireUInt32(m, "totalChunks"),
            fileSize: try A.requireUInt64(m, "fileSize"),
            fileName: try A.requireString(m, "fileName"),
            fileChecksum: try A.requireString(m, "fileChecksum"),
            data: try A.byteList(m, "data")
        )
        return nil
    }

    static func sendFileFromPath(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "sendFileFromPath")
        return try host.requireProtocol().sendFileFromPath(
            recipient: try A.requireString(m, "recipient"),
            filePath: try A.requireString(m, "filePath"),
            fileName: try A.requireString(m, "fileName")
        )
    }

    static func sendMedia(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "sendMedia")
        let bytes = try A.requireBase64(m, "fileData")
        let fileName = try A.requireString(m, "fileName")
        // The legacy surface: only the seven original metadata fields. The
        // cloud and sticker fields travel on `sendMediaRich`.
        let metadata = (m["mediaMetadata"] as? [String: Any]).map { it in
            MediaMetadata(
                mimeType: it["mime_type"] as? String ?? "",
                fileName: it["file_name"] as? String ?? fileName,
                fileSize: nonNegativeUInt64(it["file_size"]) ?? 0,
                durationMs: nonNegativeUInt64(it["duration_ms"]),
                width: nonNegativeUInt32(it["width"]),
                height: nonNegativeUInt32(it["height"]),
                thumbnailBase64: it["thumbnail_base64"] as? String
            )
        }
        return try host.requireProtocol().sendMedia(
            recipient: try A.requireString(m, "recipient"),
            fileData: bytes,
            fileName: fileName,
            contentType: parseContentType(try A.requireString(m, "contentType")),
            mediaMetadata: metadata
        )
    }

    static func sendMediaRich(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "sendMediaRich")
        let bytes = try A.requireBase64(m, "fileData")
        let options = m["options"] as? [String: Any]
        let sendOptions = MediaSendOptions(
            mediaMetadata: richMediaMetadata(options?["media_metadata"] as? [String: Any]),
            caption: options?["caption"] as? String,
            replyToMsg: options?["reply_to_msg"] as? String,
            replyContext: replyContext(options?["reply_context"] as? [String: Any]),
            forwardInfo: forwardInfo(options?["forward_info"] as? [String: Any]),
            fileId: options?["file_id"] as? String,
            appId: options?["app_id"] as? String
        )
        return try host.requireProtocol().sendMediaRich(
            recipient: try A.requireString(m, "recipient"),
            fileData: bytes,
            fileName: try A.requireString(m, "fileName"),
            contentType: parseContentType(try A.requireString(m, "contentType")),
            options: sendOptions
        )
    }

    static func sendPresenceUpdate(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "sendPresenceUpdate")
        let status: PresenceStatus
        switch try A.requireInt(m, "status") {
        case 1: status = .away
        case 2: status = .offline
        default: status = .online
        }
        return try host.requireProtocol().sendPresenceUpdate(recipient: try A.requireString(m, "recipient"), status: status)
    }

    static func sendReadReceipt(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "sendReadReceipt")
        let ids = try A.stringList(m, "messageIds")
        guard !ids.isEmpty else {
            throw HostError.invalidArgument("No valid message IDs provided")
        }
        return try host.requireProtocol().sendReadReceipt(recipient: try A.requireString(m, "recipient"), messageIds: ids)
    }

    static func sendTypingIndicator(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "sendTypingIndicator")
        return try host.requireProtocol().sendTypingIndicator(
            recipient: try A.requireString(m, "recipient"),
            conversationId: try A.requireString(m, "conversationId"),
            isTyping: A.optionalBool(m, "isTyping") ?? false
        )
    }

    // MARK: - Messaging

    static func sendMessage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "sendMessage")
        return try host.sendMessage(
            recipient: try A.requireString(m, "recipient"),
            content: try A.requireString(m, "content"),
            priority: try A.optionalInt(m, "priority") ?? 1,
            replyToMsg: m["replyToMsg"] as? String
        )
    }

    static func sendMessageRich(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "sendMessageRich")
        let options = m["options"] as? [String: Any]
        let sendOptions = SendMessageOptions(
            priority: ProtocolEngineHost.messagePriority(fromInt: try A.requireInt(m, "priority")),
            replyToMsg: A.optionalString(m, "replyToMsg"),
            contentType: (options?["content_type"] as? String).map { parseContentType($0) },
            replyContext: replyContext(options?["reply_context"] as? [String: Any]),
            mediaMetadata: richMediaMetadata(options?["media_metadata"] as? [String: Any]),
            forwardInfo: forwardInfo(options?["forward_info"] as? [String: Any]),
            appId: options?["app_id"] as? String
        )
        return try host.sendMessageRich(
            recipient: try A.requireString(m, "recipient"),
            content: try A.requireString(m, "content"),
            options: sendOptions
        )
    }

    static func forwardMessage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "forwardMessage")
        return try host.requireProtocol().forwardMessage(
            originalMessageJson: try A.requireString(m, "originalMessageJson"),
            newRecipient: try A.requireString(m, "newRecipient"),
            priority: ProtocolEngineHost.messagePriority(fromInt: try A.requireInt(m, "priority"))
        )
    }

    static func receiveMessage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.receiveMessage()
    }

    // MARK: - Lifecycle

    static func create(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let configJson = arguments as? String else {
            throw HostError.invalidArgument("create expects a JSON string")
        }
        try env.createOnEngine(configJson)
        return nil
    }

    static func destroy(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        env.tearDownProtocol()
        return nil
    }

    static func start(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try env.startOnEngine()
        return nil
    }

    static func stop(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try env.stopOnEngine()
        return nil
    }

    static func pause(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.pause()
        return nil
    }

    static func resume(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.resume()
        return nil
    }

    static func replayEvents(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "replayEvents")
        return env.replayEvents(try A.requireInt(m, "fromSeq"), try A.requireInt(m, "toSeq"))
    }

    static func getState(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let proto = host.protocolInstance else { return "Stopped" }
        switch proto.getState() {
        case .stopped: return "Stopped"
        case .running: return "Running"
        case .paused: return "Paused"
        // "Stopped", as Android's `else` branch is: an unrecognised state is
        // not a member of the exported `ProtocolState` enum, and treating it as
        // stopped is the safe direction for a reconcile loop.
        @unknown default: return "Stopped"
        }
    }

    static func localAddress(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.protocolInstance?.localAddress()
    }

    static func wipePersistedState(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "wipePersistedState")
        let appId = try A.requireString(m, "appId")
        let profile = try A.requireString(m, "profile")
        let namespace = StorageNamespace.account(appId: appId, profile: profile)
        if let live = host.currentConfig,
           StorageNamespace.account(appId: live.appId, profile: live.profile) == namespace {
            throw HostError.illegalState("Refusing to wipe storage for the running account; call destroy() first")
        }
        // Secure storage first: it holds the key every sealed protocol-state
        // record is written under, so an interrupted wipe leaves the remainder
        // as ciphertext nobody can open rather than readable state.
        var firstError: Error?
        do {
            try MlsSecureStorage.wipeAccount(accountNamespace: namespace)
        } catch {
            firstError = error
        }
        do {
            try AppContainerProtocolStateStorage.wipeAccount(accountNamespace: namespace)
        } catch {
            firstError = firstError ?? error
        }
        if let firstError = firstError { throw firstError }
        return nil
    }

    // MARK: - Transports

    static func enableTransport(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "enableTransport")
        let type = try A.requireString(m, "type")
        if let object = m["config"] as? [String: Any] {
            try host.enableTransport(type: type, config: object)
        } else {
            try host.enableTransport(type: type, configJson: m["config"] as? String)
        }
        return nil
    }

    static func disableTransport(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "disableTransport")
        let type = try A.requireString(m, "type")
        let proto = try host.requireProtocol()
        // The manager stops and the core is told the transport is gone. The
        // transport is not removed from the core, which would prevent
        // re-enabling it. A failure to tell the core is not a failure to stop.
        switch type.lowercased() {
        case "internet":
            host.internetManager?.stop()
            try? proto.internetStatusChanged(isConnected: false)
        case "wifidirect", "wifi_direct":
            host.wifiDirectManager?.stop()
        case "ble":
            host.bleManager?.stop()
            try? proto.bleStatusChanged(isAvailable: false)
        case "reticulum":
            host.reticulumManager?.stop()
            try? proto.reticulumStatusChanged(isConnected: false)
        case "nostr":
            host.nostrManager?.stop()
            try? proto.nostrStatusChanged(isConnected: false)
        default:
            throw HostError.invalidArgument("Unsupported transport type: \(type)")
        }
        return nil
    }

    static func forceTransport(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "forceTransport")
        try host.requireProtocol().forceTransport(
            transportType: try TransportBridge.mapTransportType(try A.requireString(m, "transportType"))
        )
        return nil
    }

    static func releaseTransportLock(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.protocolInstance?.releaseTransportLock()
        return nil
    }

    static func getActiveTransports(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.protocolInstance?.getActiveTransports() ?? [String]()
    }

    static func getTransportMetrics(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "getTransportMetrics")
        let type = try TransportBridge.mapTransportType(try A.requireString(m, "transportType"))
        guard let metrics = host.protocolInstance?.getTransportMetrics(transportType: type) else { return nil }
        return [
            "packetsSent": Int(metrics.packetsSent),
            "packetsReceived": Int(metrics.packetsReceived),
            "bytesSent": Int(metrics.bytesSent),
            "bytesReceived": Int(metrics.bytesReceived),
            "errorRate": Double(metrics.errorRate),
            "avgLatencyMs": Int(metrics.avgLatencyMs),
        ] as [String: Any]
    }

    static func shouldEscalateToWifi(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.requireProtocol().shouldEscalateToWifi()
    }

    // MARK: - Internet transport

    static func checkInternetPresence(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "checkInternetPresence")
        guard let manager = host.internetManager else {
            throw HostError.illegalState("Internet transport not initialized")
        }
        let userId = try A.requireString(m, "userId")
        let force = A.optionalBool(m, "force") ?? false
        return awaitCompletion { done in
            manager.checkPresence(userId: userId, force: force, completion: done)
        }
    }

    static func internetConfirmSent(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "internetConfirmSent")
        try host.requireProtocol().internetConfirmSent(messageId: try A.requireString(m, "messageId"))
        return nil
    }

    static func internetForceReconnect(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let manager = host.internetManager else { return false }
        manager.forceReconnect()
        return true
    }

    static func internetGetNextMessage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let message = try host.requireProtocol().internetGetNextMessage() else { return nil }
        return [
            "messageId": message.messageId,
            "recipientId": message.recipientId,
            "data": A.ints(message.data),
        ] as [String: Any]
    }

    static func internetIsReady(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.internetManager?.isReady() ?? false
    }

    static func internetIsSuperseded(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.internetManager?.isSessionSuperseded() ?? false
    }

    static func internetMessageReceived(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "internetMessageReceived")
        try host.requireProtocol().internetMessageReceived(
            senderId: try A.requireString(m, "senderId"),
            data: try A.byteList(m, "data")
        )
        return nil
    }

    static func internetSendFailed(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "internetSendFailed")
        try host.requireProtocol().internetSendFailed(messageId: try A.requireString(m, "messageId"))
        return nil
    }

    static func internetSendRawCommand(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "internetSendRawCommand")
        guard let manager = host.internetManager else {
            throw HostError.illegalState("Internet transport not initialized")
        }
        let json = try A.requireString(m, "json")
        return awaitCompletion { done in
            manager.sendRawCommand(json: json, completion: done)
        }
    }

    static func internetStatusChanged(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "internetStatusChanged")
        try host.requireProtocol().internetStatusChanged(isConnected: A.optionalBool(m, "isConnected") ?? false)
        return nil
    }

    // MARK: - Wi-Fi Direct

    static func wifiDirectGetNextMessage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let message = try host.requireProtocol().wifiDirectGetNextMessage() else { return nil }
        return [
            "recipientId": message.recipientId,
            "data": A.ints(message.data),
        ] as [String: Any]
    }

    static func wifiDirectMessageReceived(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "wifiDirectMessageReceived")
        try host.requireProtocol().wifiDirectMessageReceived(
            senderId: try A.requireString(m, "senderId"),
            data: try A.byteList(m, "data")
        )
        return nil
    }

    static func wifiDirectPeerConnected(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "wifiDirectPeerConnected")
        try host.requireProtocol().wifiDirectPeerConnected(peerId: try A.requireString(m, "peerId"))
        return nil
    }

    static func wifiDirectPeerDisconnected(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "wifiDirectPeerDisconnected")
        try host.requireProtocol().wifiDirectPeerDisconnected(peerId: try A.requireString(m, "peerId"))
        return nil
    }

    static func wifiDirectStatusChanged(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "wifiDirectStatusChanged")
        try host.requireProtocol().wifiDirectStatusChanged(isConnected: A.optionalBool(m, "isConnected") ?? false)
        return nil
    }

    // MARK: - Telemetry

    static func enableTelemetry(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.enableTelemetry(config: arguments as? [String: Any], initialAppState: env.appState())
        return nil
    }

    static func disableTelemetry(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.disableTelemetry()
        return nil
    }

    static func flushTelemetry(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.flushTelemetry()
        return nil
    }

    static func endTelemetrySession(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.endTelemetrySession()
        return nil
    }

    static func setTelemetryEnabled(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let enabled: Bool
        if let flag = arguments as? NSNumber {
            enabled = flag.boolValue
        } else if arguments is [String: Any] {
            enabled = try A.requireBool(try A.map(arguments, for: "setTelemetryEnabled"), "enabled")
        } else {
            throw HostError.invalidArgument("setTelemetryEnabled expects boolean or map")
        }
        host.setTelemetryEnabled(enabled)
        return nil
    }

    static func telemetryInstallId(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.telemetryInstallId()
    }

    static func telemetryStats(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let stats = host.telemetryStats() else { return nil }
        return host.telemetryStatsMap(stats)
    }

    static func emitTestEvent(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.requireProtocol().emitTestEvent()
        return nil
    }

    // MARK: - Statistics and configuration

    static func getBatteryLevel(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.protocolInstance?.getBatteryLevel().map { Int($0) }
    }

    static func getIsCharging(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.protocolInstance?.getIsCharging() ?? false
    }

    static func setBatteryLevel(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "setBatteryLevel")
        try host.requireProtocol().setBatteryLevel(level: batteryPercent(try A.requireInt(m, "level")))
        return nil
    }

    static func setBatteryState(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "setBatteryState")
        try host.requireProtocol().setBatteryState(
            level: batteryPercent(try A.requireInt(m, "level")),
            isCharging: A.optionalBool(m, "isCharging") ?? false
        )
        return nil
    }

    static func getCustodyStats(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let stats = host.protocolInstance?.getCustodyStats() else { return nil }
        return [
            "held": Double(stats.held),
            "heldBytes": Double(stats.heldBytes),
            "accepted": Double(stats.accepted),
            "delivered": Double(stats.delivered),
            "reOriginated": Double(stats.reOriginated),
            "expired": Double(stats.expired),
            "duplicates": Double(stats.duplicates),
            "evicted": Double(stats.evicted),
            "receiptsSent": Double(stats.receiptsSent),
            "receiptsDropped": Double(stats.receiptsDropped),
            "receiptsReceived": Double(stats.receiptsReceived),
            "receiptsIgnored": Double(stats.receiptsIgnored),
            "refusedDisabled": Double(stats.refusedDisabled),
            "refusedNoRequest": Double(stats.refusedNoRequest),
            "refusedUnknownClass": Double(stats.refusedUnknownClass),
            "refusedNotSealed": Double(stats.refusedNotSealed),
            "refusedUnprovenPeer": Double(stats.refusedUnprovenPeer),
            "refusedNotDepositor": Double(stats.refusedNotDepositor),
            "refusedStranger": Double(stats.refusedStranger),
            "refusedDepositorFull": Double(stats.refusedDepositorFull),
            "refusedStoreFull": Double(stats.refusedStoreFull),
            "refusedBattery": Double(stats.refusedBattery),
        ] as [String: Any]
    }

    static func eraseCustody(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.requireProtocol().eraseCustody()
        return nil
    }

    static func getDedupStats(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let stats = host.protocolInstance?.getDedupStats() else { return nil }
        return [
            "totalTracked": Double(stats.totalTracked),
            "recentTracked": Double(stats.recentTracked),
            "capacityUsedPercent": Int(stats.capacityUsedPercent),
            "mode": stats.mode,
        ] as [String: Any]
    }

    static func getDeliverySuccessRate(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        Double(try host.requireProtocol().getDeliverySuccessRate())
    }

    static func getMedianHops(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let hops = Int(try host.requireProtocol().getMedianHops())
        return hops == 0 ? nil : hops
    }

    static func getMedianLatency(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let latency = try host.requireProtocol().getMedianLatency()
        // An integer, as Android's `toLong()`: Dart sees `int` on both
        // platforms, where a Double here would be a `double` on iOS only.
        return latency == 0 ? nil : Int64(truncatingIfNeeded: latency)
    }

    static func getMeshRelayStats(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let stats = host.protocolInstance?.getMeshRelayStats() else { return nil }
        return [
            "forwarded": Double(stats.forwarded),
            "transmissions": Double(stats.transmissions),
            "queued": Double(stats.queued),
            "awaitingTransmission": Double(stats.awaitingTransmission),
            "duplicatesSuppressed": Double(stats.duplicatesSuppressed),
            "coveredByANeighbor": Double(stats.coveredByANeighbor),
            "peerRateLimited": Double(stats.peerRateLimited),
            "refusedQueueFull": Double(stats.refusedQueueFull),
            "rateDeferred": Double(stats.rateDeferred),
            "abandonedOverdue": Double(stats.abandonedOverdue),
            "hopLimitReached": Double(stats.hopLimitReached),
            "reachClamped": Double(stats.reachClamped),
            "droppedForCapacity": Double(stats.droppedForCapacity),
        ] as [String: Any]
    }

    static func getMeshRelayTunables(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let tunables = host.protocolInstance?.getMeshRelayTunables() else { return nil }
        return [
            "maxTtl": Int(tunables.maxTtl),
            "denseMaxTtl": Int(tunables.denseMaxTtl),
            "denseDegree": Double(tunables.denseDegree),
            "fanout": Double(tunables.fanout),
            "jitterMinMs": Double(tunables.jitterMinMs),
            "jitterMaxMs": Double(tunables.jitterMaxMs),
            "ratePerSec": Double(tunables.ratePerSec),
            "burst": Double(tunables.burst),
            "peerRatePerSec": Double(tunables.peerRatePerSec),
            "peerBurst": Double(tunables.peerBurst),
            "queueCapacity": Double(tunables.queueCapacity),
            "biasMinScale": Double(tunables.biasMinScale),
            "biasMaxHandicapMs": Double(tunables.biasMaxHandicapMs),
            "activityWindowMs": Double(tunables.activityWindowMs),
            "activityMinForwards": Double(tunables.activityMinForwards),
            "activityIdleWindows": Double(tunables.activityIdleWindows),
        ] as [String: Any]
    }

    static func getMessageStats(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try TransportBridge.buildMessageStatsJson(try host.requireProtocol().getMessageStats())
    }

    static func getTopology(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let proto = try host.requireProtocol()
        return try TransportBridge.buildTopologyJson(try proto.getTopology(), localAddress: proto.localAddress())
    }

    static func getPendingAckCount(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        Double(host.protocolInstance?.getPendingAckCount() ?? 0)
    }

    static func getRetryQueueSize(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        Double(host.protocolInstance?.getRetryQueueSize() ?? 0)
    }

    static func isRelay(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.protocolInstance?.isRelay() ?? false
    }

    static func getRelayPriority(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        guard let proto = host.protocolInstance else { return "auto" }
        return InitialRuntimeConfigApplier.relayPriorityName(proto.getRelayPriority())
    }

    static func setRelayPriority(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "setRelayPriority")
        guard let priority = InitialRuntimeConfigApplier.normalizeRelayPriority(try A.requireString(m, "priorityString")) else {
            throw HostError.invalidArgument("Invalid relay priority")
        }
        try host.requireProtocol().setRelayPriority(priority: priority)
        return nil
    }

    static func getRelayConfig(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let config = try host.requireProtocol().getRelayConfig()
        return try jsonString([
            "minBatteryForRelay": Int(config.minBatteryForRelay),
            "allowRelay": config.allowRelay,
            "relayPriority": InitialRuntimeConfigApplier.relayPriorityName(config.relayPriority),
        ] as [String: Any])
    }

    static func updateRelayConfig(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let json = try A.jsonObject(try configJsonString(arguments), "relay config")
        let proto = try host.requireProtocol()
        try proto.updateRelayConfig(
            config: InitialRuntimeConfigApplier.mergedRelayConfig(current: proto.getRelayConfig(), from: json)
        )
        return nil
    }

    static func getDorsConfig(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let c = try host.requireProtocol().getDorsConfig()
        return try jsonString([
            "preferOnline": c.preferOnline,
            "switchHysteresis": Double(c.switchHysteresis),
            "switchCooldownSecs": Double(c.switchCooldownSecs),
            "bleToWifiRetryThreshold": Int(c.bleToWifiRetryThreshold),
            "minSuccessRateBeforeEscalation": Double(c.minSuccessRateBeforeEscalation),
            "minBleSamplesBeforeSuccessRateEscalation": Double(c.minBleSamplesBeforeSuccessRateEscalation),
            "rssiSwitchThreshold": Int(c.rssiSwitchThreshold),
            "congestionQueueThreshold": Double(c.congestionQueueThreshold),
            "stabilityWindowSecs": Double(c.stabilityWindowSecs),
            "poorSignalDurationSecs": Double(c.poorSignalDurationSecs),
            "ttlEscalationThreshold": Int(c.ttlEscalationThreshold),
            "congestionDurationSecs": Double(c.congestionDurationSecs),
            "ttlEscalationHoldSecs": Double(c.ttlEscalationHoldSecs),
            "historyWindowSize": Double(c.historyWindowSize),
            "queueRecoveryRatio": Double(c.queueRecoveryRatio),
            "lowBatteryThreshold": Int(c.lowBatteryThreshold),
            "relayMinBatteryLevel": Int(c.relayMinBatteryLevel),
            "relayOptimalConnectionCount": Int(c.relayOptimalConnectionCount),
        ] as [String: Any])
    }

    static func updateDorsConfig(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let j = try A.jsonObject(try configJsonString(arguments), "DORS config")
        let proto = try host.requireProtocol()
        let current = proto.getDorsConfig()
        let config = DorsConfig(
            preferOnline: j["preferOnline"] as? Bool ?? current.preferOnline,
            switchHysteresis: Float(max(number(j, "switchHysteresis") ?? Double(current.switchHysteresis), 0)),
            switchCooldownSecs: unsigned64(j, "switchCooldownSecs", current.switchCooldownSecs),
            bleToWifiRetryThreshold: unsigned32(j, "bleToWifiRetryThreshold", current.bleToWifiRetryThreshold),
            minSuccessRateBeforeEscalation: Float(min(max(number(j, "minSuccessRateBeforeEscalation") ?? Double(current.minSuccessRateBeforeEscalation), 0), 1)),
            minBleSamplesBeforeSuccessRateEscalation: unsigned64(j, "minBleSamplesBeforeSuccessRateEscalation", current.minBleSamplesBeforeSuccessRateEscalation),
            rssiSwitchThreshold: Int16(clamping: Int(clampingDouble: number(j, "rssiSwitchThreshold")) ?? Int(current.rssiSwitchThreshold)),
            congestionQueueThreshold: unsigned64(j, "congestionQueueThreshold", current.congestionQueueThreshold),
            stabilityWindowSecs: unsigned64(j, "stabilityWindowSecs", current.stabilityWindowSecs),
            poorSignalDurationSecs: unsigned64(j, "poorSignalDurationSecs", current.poorSignalDurationSecs),
            ttlEscalationThreshold: UInt8(clamping: Int(clampingDouble: number(j, "ttlEscalationThreshold")) ?? Int(current.ttlEscalationThreshold)),
            congestionDurationSecs: unsigned64(j, "congestionDurationSecs", current.congestionDurationSecs),
            ttlEscalationHoldSecs: max(unsigned64(j, "ttlEscalationHoldSecs", current.ttlEscalationHoldSecs), 1),
            // Clamped in the unsigned domain, never through `Int`: a negative
            // value reaching a narrowing conversion traps and aborts the app.
            historyWindowSize: max(1, min(100, unsigned64(j, "historyWindowSize", current.historyWindowSize))),
            queueRecoveryRatio: Float(min(max(number(j, "queueRecoveryRatio") ?? Double(current.queueRecoveryRatio), 0), 1)),
            lowBatteryThreshold: batteryPercent(Int(clampingDouble: number(j, "lowBatteryThreshold")) ?? Int(current.lowBatteryThreshold)),
            relayMinBatteryLevel: batteryPercent(Int(clampingDouble: number(j, "relayMinBatteryLevel")) ?? Int(current.relayMinBatteryLevel)),
            relayOptimalConnectionCount: UInt8(clamping: Int(clampingDouble: number(j, "relayOptimalConnectionCount")) ?? Int(current.relayOptimalConnectionCount))
        )
        try proto.updateDorsConfig(config: config)
        return nil
    }

    static func updateAckConfig(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let j = try A.jsonObject(try configJsonString(arguments), "ACK config")
        try host.protocolInstance?.updateAckConfig(
            config: AckConfig(
                defaultTimeoutMs: unsigned64(j, "defaultTimeoutMs", 10_000),
                maxPendingAcks: unsigned64(j, "maxPendingAcks", 1_000)
            )
        )
        return nil
    }

    static func updateDedupConfig(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let j = try A.jsonObject(try configJsonString(arguments), "dedup config")
        try host.protocolInstance?.updateDedupConfig(
            config: DedupConfig(
                maxTrackedMessages: unsigned64(j, "maxTrackedMessages", 5_000),
                retentionTimeSecs: unsigned64(j, "retentionTimeSecs", 604_800)
            )
        )
        return nil
    }

    static func updateRetryConfig(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let j = try A.jsonObject(try configJsonString(arguments), "retry config")
        try host.protocolInstance?.updateRetryConfig(
            config: RetryConfig(
                maxRetries: unsigned32(j, "maxRetries", 10),
                initialDelayMs: unsigned64(j, "initialDelayMs", 1_000),
                maxDelayMs: unsigned64(j, "maxDelayMs", 300_000),
                backoffMultiplier: Float(number(j, "backoffMultiplier") ?? 2.0),
                outboxMaxLifetimeMs: unsigned64(j, "outboxMaxLifetimeMs", 604_800_000),
                pendingMessageMaxLifetimeMs: unsigned64(j, "pendingMessageMaxLifetimeMs", 604_800_000),
                edgeDrivenUnreachableDm: j["edgeDrivenUnreachableDm"] as? Bool ?? false
            )
        )
        return nil
    }

    // MARK: - Identity and signatures

    static func deriveAddress(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "deriveAddress")
        return try uniffiDeriveAddress(publicKey: try A.byteList(m, "publicKey"))
    }

    static func deriveUserIdFromPublicKey(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "deriveUserIdFromPublicKey")
        return try host.requireProtocol().deriveUserIdFromPublicKey(publicKey: try A.byteList(m, "publicKey"))
    }

    static func getIdentityPublicKey(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        A.ints(try host.requireProtocol().getIdentityPublicKey())
    }

    static func signData(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "signData")
        return A.ints(try host.requireProtocol().signData(data: try A.byteList(m, "data")))
    }

    static func signDataBase64(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "signDataBase64")
        let signature = try host.requireProtocol().signData(data: try A.requireBase64(m, "dataBase64"))
        return Data(signature).base64EncodedString()
    }

    static func verifySignature(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "verifySignature")
        return try host.requireProtocol().verifySignature(
            publicKey: try A.byteList(m, "publicKey"),
            data: try A.byteList(m, "data"),
            signature: try A.byteList(m, "signature")
        )
    }

    static func verifySignatureBase64(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "verifySignatureBase64")
        return try host.requireProtocol().verifySignature(
            publicKey: try A.requireBase64(m, "publicKeyBase64"),
            data: try A.requireBase64(m, "dataBase64"),
            signature: try A.requireBase64(m, "signatureBase64")
        )
    }

    // MARK: - MLS

    static func initializeMlsWithSecureStorage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.initializeMlsWithSecureStorage()
        return nil
    }

    static func isMlsInitialized(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.protocolInstance?.isMlsInitialized() ?? false
    }

    static func hasPendingKeyPackage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "hasPendingKeyPackage")
        return host.protocolInstance?.hasPendingKeyPackage(peerId: try A.requireString(m, "peerId")) ?? false
    }

    static func getEstablishmentState(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "getEstablishmentState")
        guard let proto = host.protocolInstance else { return "NoKeyPackage" }
        switch try proto.getEstablishmentState(peerId: try A.requireString(m, "peerId")) {
        case .noKeyPackage: return "NoKeyPackage"
        case .haveKeyPackage: return "HaveKeyPackage"
        case .sessionPending: return "SessionPending"
        case .sessionConfirmed: return "SessionConfirmed"
        }
    }

    static func establishSecureSession(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "establishSecureSession")
        let welcome = try host.requireProtocol().establishSecureSession(peerId: try A.requireString(m, "peerId"))
        return welcome.map { welcomeMap($0) }
    }

    static func rekeySession(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "rekeySession")
        return try host.requireProtocol().rekeySession(peerId: try A.requireString(m, "peerId"))
    }

    static func mlsCreateSession(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "mlsCreateSession")
        return welcomeMap(try host.requireProtocol().mlsCreateSession(otherUserId: try A.requireString(m, "otherUserId")))
    }

    static func mlsJoinSession(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "mlsJoinSession")
        let welcome = try welcomeFromJson(try A.requireString(m, "welcomeJson"))
        return ProtocolEngineHost.groupInfoMap(try host.requireProtocol().mlsJoinSession(welcome: welcome))
    }

    static func mlsProcessWelcome(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "mlsProcessWelcome")
        let welcome = try welcomeFromJson(try A.requireString(m, "welcomeJson"))
        return ProtocolEngineHost.groupInfoMap(try host.requireProtocol().mlsProcessWelcome(welcome: welcome))
    }

    static func mlsDecrypt(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "mlsDecrypt")
        let encrypted = try encryptedFromJson(try A.requireString(m, "encryptedJson"))
        return try host.requireProtocol().mlsDecrypt(encrypted: encrypted).map { A.ints($0) }
    }

    static func mlsDecryptFromUser(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "mlsDecryptFromUser")
        let encrypted = try encryptedFromJson(try A.requireString(m, "encryptedJson"))
        return try host.requireProtocol().mlsDecryptFromUser(encrypted: encrypted).map { A.ints($0) }
    }

    static func mlsEncryptForUser(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "mlsEncryptForUser")
        let encrypted = try host.requireProtocol().mlsEncryptForUser(
            otherUserId: try A.requireString(m, "otherUserId"),
            plaintext: try A.byteList(m, "plaintext")
        )
        return [
            "groupId": encrypted.groupId,
            "messageType": encrypted.messageType,
            "epoch": Double(encrypted.epoch),
            "ciphertext": A.ints(encrypted.ciphertext),
            "senderId": encrypted.senderId,
            "timestampMs": Double(encrypted.timestampMs),
        ] as [String: Any]
    }

    static func mlsDeleteSession(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "mlsDeleteSession")
        try host.requireProtocol().mlsDeleteSession(otherUserId: try A.requireString(m, "otherUserId"))
        return nil
    }

    static func mlsHasSession(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "mlsHasSession")
        return host.protocolInstance?.mlsHasSession(otherUserId: try A.requireString(m, "otherUserId")) ?? false
    }

    static func mlsListSessions(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        host.protocolInstance?.mlsListSessions() ?? [String]()
    }

    static func mlsGenerateKeyPackage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        keyPackageMap(try host.requireProtocol().mlsGenerateKeyPackage())
    }

    static func mlsGetOrCreateKeyPackage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        keyPackageMap(try host.requireProtocol().mlsGetOrCreateKeyPackage())
    }

    static func mlsGetPendingKeyPackages(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.requireProtocol().mlsGetPendingKeyPackages().map { keyPackageMap($0) }
    }

    static func mlsImportKeyPackage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "mlsImportKeyPackage")
        try host.requireProtocol().mlsImportKeyPackage(
            userId: try A.requireString(m, "userId"),
            keyPackageData: try A.requireByteList(m, "keyPackageData")
        )
        return nil
    }

    static func mlsMarkKeyPackageSynced(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "mlsMarkKeyPackageSynced")
        try host.requireProtocol().mlsMarkKeyPackageSynced(packageId: try A.requireString(m, "packageId"))
        return nil
    }

    // MARK: - Mesh groups

    static func meshCreateGroup(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshCreateGroup")
        return try host.meshCreateGroup(name: try A.requireString(m, "name"))
    }

    static func meshForwardMessageToGroup(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshForwardMessageToGroup")
        return try host.meshForwardMessageToGroup(
            originalMessageJson: try A.requireString(m, "originalMessageJson"),
            groupId: try A.requireString(m, "groupId"),
            priority: m["priority"] as? String
        )
    }

    static func meshGetGroupInfo(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshGetGroupInfo")
        return try host.meshGetGroupInfo(groupId: try A.requireString(m, "groupId"))
    }

    static func meshGetGroupRoles(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshGetGroupRoles")
        return try host.meshGetGroupRoles(groupId: try A.requireString(m, "groupId"))
    }

    static func meshGetMemberRole(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshGetMemberRole")
        return try host.meshGetMemberRole(
            groupId: try A.requireString(m, "groupId"),
            userId: try A.requireString(m, "userId")
        )
    }

    static func meshGroupRelaySyncState(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshGroupRelaySyncState")
        return try host.meshGroupRelaySyncState(groupId: try A.requireString(m, "groupId"))
    }

    static func meshGroupRichReadiness(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshGroupRichReadiness")
        return try host.meshGroupRichReadiness(groupId: try A.requireString(m, "groupId"))
    }

    static func meshInviteToGroup(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshInviteToGroup")
        try host.meshInviteToGroup(
            groupId: try A.requireString(m, "groupId"),
            inviteeUserId: try A.requireString(m, "inviteeUserId")
        )
        return nil
    }

    static func meshLeaveGroup(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshLeaveGroup")
        try host.meshLeaveGroup(groupId: try A.requireString(m, "groupId"))
        return nil
    }

    static func meshListGroups(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.meshListGroups()
    }

    static func meshRemoveFromGroup(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshRemoveFromGroup")
        try host.meshRemoveFromGroup(
            groupId: try A.requireString(m, "groupId"),
            memberId: try A.requireString(m, "memberId")
        )
        return nil
    }

    static func meshRenameGroup(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshRenameGroup")
        try host.meshRenameGroup(
            groupId: try A.requireString(m, "groupId"),
            newName: try A.requireString(m, "newName")
        )
        return nil
    }

    static func meshRequestGroupRelayRegistration(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshRequestGroupRelayRegistration")
        return try host.meshRequestGroupRelayRegistration(groupId: try A.requireString(m, "groupId"))
    }

    static func meshSendGroupMessage(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshSendGroupMessage")
        return try host.meshSendGroupMessage(
            groupId: try A.requireString(m, "groupId"),
            content: try A.requireString(m, "content"),
            priority: m["priority"] as? String,
            replyToMsg: m["replyToMsg"] as? String
        )
    }

    static func meshSetMemberRole(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "meshSetMemberRole")
        try host.meshSetMemberRole(
            groupId: try A.requireString(m, "groupId"),
            userId: try A.requireString(m, "userId"),
            role: try A.requireString(m, "role")
        )
        return nil
    }

    // MARK: - Mesh services

    static func registerService(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "registerService")
        var capabilities: [String: String] = [:]
        // Capabilities the Dart side cannot encode as a string map are dropped,
        // as Android drops them: the registration still stands.
        if let text = m["capabilitiesJson"] as? String,
           let object = try? A.jsonObject(text, "capabilities") {
            for (key, value) in object {
                if let string = value as? String {
                    capabilities[key] = string
                }
            }
        }
        try host.requireMeshServices().registerService(
            serviceId: try A.requireString(m, "serviceId"),
            version: try A.requireString(m, "version"),
            capabilities: capabilities
        )
        return nil
    }

    static func unregisterService(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "unregisterService")
        return try host.requireMeshServices().unregisterService(serviceId: try A.requireString(m, "serviceId"))
    }

    static func discoverServices(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "discoverServices")
        return try host.requireMeshServices().discoverServices(serviceId: A.optionalString(m, "serviceId"))
    }

    static func sendServiceRequest(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "sendServiceRequest")
        return try host.requireMeshServices().sendServiceRequest(
            provider: try A.requireString(m, "provider"),
            serviceId: try A.requireString(m, "serviceId"),
            method: try A.requireString(m, "method"),
            body: try A.requireString(m, "body")
        )
    }

    static func respondToServiceRequest(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "respondToServiceRequest")
        return try host.requireMeshServices().respondToServiceRequest(
            requestId: try A.requireString(m, "requestId"),
            requester: try A.requireString(m, "requester"),
            serviceId: try A.requireString(m, "serviceId"),
            status: try A.requireString(m, "status"),
            body: try A.requireString(m, "body")
        )
    }

    // MARK: - Data store

    static func dataCreateDoc(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataCreateDoc")
        try host.dataCreateDoc(spaceId: try A.requireString(m, "spaceId"), docId: try A.requireString(m, "docId"))
        return nil
    }

    static func dataDeleteDoc(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataDeleteDoc")
        try host.dataDeleteDoc(spaceId: try A.requireString(m, "spaceId"), docId: try A.requireString(m, "docId"))
        return nil
    }

    static func dataRemoveDoc(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataRemoveDoc")
        try host.dataRemoveDoc(spaceId: try A.requireString(m, "spaceId"), docId: try A.requireString(m, "docId"))
        return nil
    }

    static func dataRemoveSpace(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataRemoveSpace")
        try host.dataRemoveSpace(spaceId: try A.requireString(m, "spaceId"))
        return nil
    }

    static func dataFetchAttachmentFrom(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataFetchAttachmentFrom")
        try host.dataFetchAttachmentFrom(
            spaceId: try A.requireString(m, "spaceId"),
            peerId: try A.requireString(m, "peerId"),
            hash: try A.requireString(m, "hash")
        )
        return nil
    }

    static func dataSetInterest(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataSetInterest")
        try host.dataSetInterest(spaceId: try A.requireString(m, "spaceId"), patterns: try A.stringList(m, "patterns"))
        return nil
    }

    static func dataListDocs(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataListDocs")
        return try host.dataListDocs(spaceId: try A.requireString(m, "spaceId"))
    }

    static func dataListSpaces(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.dataListSpaces()
    }

    static func dataMapSet(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataMapSet")
        try host.dataMapSet(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection"),
            key: try A.requireString(m, "key"),
            valueJson: try A.requireString(m, "valueJson")
        )
        return nil
    }

    static func dataMapDelete(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataMapDelete")
        try host.dataMapDelete(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection"),
            key: try A.requireString(m, "key")
        )
        return nil
    }

    static func dataMapGetJson(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataMapGetJson")
        return try host.dataMapGetJson(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection"),
            key: try A.requireString(m, "key")
        )
    }

    static func dataListPush(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataListPush")
        try host.dataListPush(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection"),
            valueJson: try A.requireString(m, "valueJson")
        )
        return nil
    }

    static func dataListDelete(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataListDelete")
        try host.dataListDelete(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection"),
            index: try A.requireDouble(m, "index"),
            count: try A.requireDouble(m, "count")
        )
        return nil
    }

    static func dataListLen(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataListLen")
        return try host.dataListLen(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection")
        )
    }

    static func dataTextInsert(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataTextInsert")
        try host.dataTextInsert(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection"),
            position: try A.requireDouble(m, "position"),
            text: try A.requireString(m, "text")
        )
        return nil
    }

    static func dataTextDelete(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataTextDelete")
        try host.dataTextDelete(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection"),
            position: try A.requireDouble(m, "position"),
            count: try A.requireDouble(m, "count")
        )
        return nil
    }

    static func dataTextValue(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataTextValue")
        return try host.dataTextValue(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection")
        )
    }

    static func dataCounterIncrement(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataCounterIncrement")
        try host.dataCounterIncrement(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection"),
            amount: try A.requireDouble(m, "amount")
        )
        return nil
    }

    static func dataCounterValue(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataCounterValue")
        return try host.dataCounterValue(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId"),
            collection: try A.requireString(m, "collection")
        )
    }

    static func dataDocJson(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataDocJson")
        return try host.dataDocJson(spaceId: try A.requireString(m, "spaceId"), docId: try A.requireString(m, "docId"))
    }

    static func dataExportRaw(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataExportRaw")
        return try host.dataExportRaw(
            spaceId: try A.requireString(m, "spaceId"),
            docId: try A.requireString(m, "docId")
        )
    }

    static func dataFlush(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataFlush")
        try host.dataFlush(spaceId: try A.requireString(m, "spaceId"), docId: try A.requireString(m, "docId"))
        return nil
    }

    static func dataFlushAll(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.dataFlushAll()
        return nil
    }

    static func dataDocSize(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataDocSize")
        return try host.dataDocSize(spaceId: try A.requireString(m, "spaceId"), docId: try A.requireString(m, "docId"))
    }

    static func dataAttachmentHash(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataAttachmentHash")
        return try host.dataAttachmentHash(bytesBase64: try A.requireString(m, "bytesBase64"))
    }

    static func dataFetchAttachment(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataFetchAttachment")
        try host.dataFetchAttachment(spaceId: try A.requireString(m, "spaceId"), hash: try A.requireString(m, "hash"))
        return nil
    }

    static func dataProvideAttachment(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataProvideAttachment")
        try host.dataProvideAttachment(
            spaceId: try A.requireString(m, "spaceId"),
            peerId: try A.requireString(m, "peerId"),
            hash: try A.requireString(m, "hash"),
            bytesBase64: try A.requireString(m, "bytesBase64")
        )
        return nil
    }

    static func dataDeclineAttachment(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments, for: "dataDeclineAttachment")
        try host.dataDeclineAttachment(
            spaceId: try A.requireString(m, "spaceId"),
            peerId: try A.requireString(m, "peerId"),
            hash: try A.requireString(m, "hash")
        )
        return nil
    }

    static func dataWipeAll(host: ProtocolEngineHost, env: EngineBridgeEnvironment, arguments: Any?) throws -> Any? {
        try host.dataWipeAll()
        return nil
    }

    // MARK: - Helpers

    /// Runs a transport call that reports through a completion and waits for
    /// it, bounded. The completions fire on the manager's own queues, never on
    /// the engine queue this blocks, and a transport that never answers reads
    /// as "not written" rather than hanging the channel.
    private static func awaitCompletion(_ start: (@escaping (Bool) -> Void) -> Void) -> Bool {
        let outcome = CompletionBox()
        let signal = DispatchSemaphore(value: 0)
        start { written in
            outcome.set(written)
            signal.signal()
        }
        _ = signal.wait(timeout: .now() + completionWaitSeconds)
        return outcome.value
    }

    private final class CompletionBox {
        private let lock = NSLock()
        private var stored = false

        var value: Bool {
            lock.lock()
            defer { lock.unlock() }
            return stored
        }

        func set(_ newValue: Bool) {
            lock.lock()
            stored = newValue
            lock.unlock()
        }
    }

    private static func configJsonString(_ arguments: Any?) throws -> String {
        if let text = arguments as? String {
            return text
        }
        return try A.requireString(try A.map(arguments, for: "config"), "configJson")
    }

    private static func jsonString(_ object: Any) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: object, options: [])
        guard let json = String(data: data, encoding: .utf8) else {
            throw HostError.illegalState("Failed to encode JSON")
        }
        return json
    }

    /// A finite JSON number, else nil.
    private static func number(_ json: [String: Any], _ key: String) -> Double? {
        guard let value = (json[key] as? NSNumber)?.doubleValue, value.isFinite else { return nil }
        return value
    }

    /// An unsigned 64-bit config value; a negative one reads as zero rather
    /// than wrapping to a huge value, and an absent one as `fallback`.
    private static func unsigned64(_ json: [String: Any], _ key: String, _ fallback: UInt64) -> UInt64 {
        guard let value = number(json, key) else { return fallback }
        return UInt64(min(max(value, 0), Double(UInt64(1) << 62)))
    }

    private static func unsigned32(_ json: [String: Any], _ key: String, _ fallback: UInt32) -> UInt32 {
        guard let value = number(json, key) else { return fallback }
        return UInt32(min(max(value, 0), Double(UInt32.max)))
    }

    private static func nonNegativeUInt64(_ value: Any?) -> UInt64? {
        guard let number = (value as? NSNumber)?.doubleValue, number.isFinite, number >= 0 else { return nil }
        return UInt64(min(number, Double(UInt64(1) << 62)))
    }

    private static func nonNegativeUInt32(_ value: Any?) -> UInt32? {
        guard let number = (value as? NSNumber)?.doubleValue, number.isFinite, number >= 0 else { return nil }
        return UInt32(min(number, Double(UInt32.max)))
    }

    /// A battery percentage clamped to 0...100, never narrowed unchecked.
    private static func batteryPercent(_ level: Int) -> UInt8 {
        UInt8(min(100, max(0, level)))
    }

    private static func parseContentType(_ value: String) -> ContentType {
        switch value.lowercased() {
        case "text": return .text
        case "image": return .image
        case "video": return .video
        case "audio": return .audio
        case "voice_note": return .voiceNote
        case "video_note": return .videoNote
        case "file": return .file
        case "file_chunk": return .fileChunk
        case "poll": return .poll
        default: return .file
        }
    }

    private static func replyContext(_ map: [String: Any]?) -> ReplyContext? {
        guard let map = map else { return nil }
        return ReplyContext(
            sender: map["sender"] as? String ?? "",
            text: map["text"] as? String ?? "",
            timestamp: (map["timestamp"] as? NSNumber)?.int64Value,
            replyMediaLabel: map["reply_media_label"] as? String,
            replyContentType: map["reply_content_type"] as? String
        )
    }

    private static func forwardInfo(_ map: [String: Any]?) -> ForwardInfo? {
        guard let map = map else { return nil }
        return ForwardInfo(
            originalSender: map["original_sender"] as? String ?? "",
            originalMessageId: map["original_message_id"] as? String ?? "",
            originalTimestamp: (map["original_timestamp"] as? NSNumber)?.int64Value ?? 0,
            forwardCount: nonNegativeUInt32(map["forward_count"]) ?? 1
        )
    }

    /// The full metadata parser for the rich send surface: unlike the legacy
    /// `sendMedia` mapping it carries the cloud and sticker fields, which only
    /// ever travel MLS-sealed on this path.
    private static func richMediaMetadata(_ map: [String: Any]?) -> MediaMetadata? {
        guard let map = map else { return nil }
        return MediaMetadata(
            mimeType: map["mime_type"] as? String ?? "",
            fileName: map["file_name"] as? String ?? "",
            fileSize: nonNegativeUInt64(map["file_size"]) ?? 0,
            durationMs: nonNegativeUInt64(map["duration_ms"]),
            width: nonNegativeUInt32(map["width"]),
            height: nonNegativeUInt32(map["height"]),
            thumbnailBase64: map["thumbnail_base64"] as? String,
            mediaId: map["media_id"] as? String,
            downloadUrl: map["download_url"] as? String,
            thumbnailUrl: map["thumbnail_url"] as? String,
            encryptionKey: map["encryption_key"] as? String,
            iv: map["iv"] as? String,
            ciphertextHash: map["ciphertext_hash"] as? String,
            stickerProvider: map["sticker_provider"] as? String,
            stickerRemoteId: map["sticker_remote_id"] as? String,
            stickerKind: map["sticker_kind"] as? String
        )
    }

    private static func welcomeFromJson(_ text: String) throws -> MlsWelcomeMessage {
        let json = try A.jsonObject(text, "welcome")
        return MlsWelcomeMessage(
            groupId: json["groupId"] as? String ?? "",
            welcomeData: try A.byteList(json, "welcomeData"),
            inviterId: json["inviterId"] as? String ?? "",
            groupName: json["groupName"] as? String,
            timestampMs: nonNegativeUInt64(json["timestampMs"]) ?? 0
        )
    }

    private static func encryptedFromJson(_ text: String) throws -> MlsEncryptedMessage {
        let json = try A.jsonObject(text, "encrypted message")
        return MlsEncryptedMessage(
            groupId: json["groupId"] as? String ?? "",
            messageType: json["messageType"] as? String ?? "Application",
            epoch: nonNegativeUInt64(json["epoch"]) ?? 0,
            ciphertext: try A.byteList(json, "ciphertext"),
            senderId: json["senderId"] as? String ?? "",
            timestampMs: nonNegativeUInt64(json["timestampMs"]) ?? 0
        )
    }

    private static func welcomeMap(_ welcome: MlsWelcomeMessage) -> [String: Any] {
        [
            "groupId": welcome.groupId,
            "welcomeData": A.ints(welcome.welcomeData),
            "inviterId": welcome.inviterId,
            "groupName": welcome.groupName ?? NSNull(),
            "timestampMs": Double(welcome.timestampMs),
        ]
    }

    private static func keyPackageMap(_ bundle: MlsKeyPackageBundle) -> [String: Any] {
        [
            "packageId": bundle.packageId,
            "userId": bundle.userId,
            "keyPackageData": A.ints(bundle.keyPackageData),
            "createdAtMs": Double(bundle.createdAtMs),
            "expiresAtMs": Double(bundle.expiresAtMs),
            "synced": bundle.synced,
        ]
    }
}

// MARK: - UniFFI free functions

// The generated `deriveAddress` and `parseInvite` are free functions. Inside
// `ProtocolEngineHostBridge` a static member of the same name shadows them, so
// a call there would resolve to the member and fail to compile; these file-scope
// wrappers are the one place that names the generated functions.

private func uniffiDeriveAddress(publicKey: [UInt8]) throws -> String {
    try deriveAddress(publicKey: publicKey)
}

private func uniffiParseInvite(blob: String) throws -> InviteInfo {
    try parseInvite(blob: blob)
}

private extension Int {
    /// An `Int` from an optional config double, saturating rather than trapping.
    init?(clampingDouble value: Double?) {
        guard let value = value else { return nil }
        self = Int(Swift.min(Swift.max(value, -1.0e15), 1.0e15))
    }
}
