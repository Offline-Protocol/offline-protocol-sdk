package com.offlineprotocol

import android.content.Context
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.bridge.ReadableMap

/**
 * React Native adapters for [ProtocolEngineHostBridge]. Every method listed in
 * `tools/gen_protocol_engine_host_bridge.py` delegates here so Flutter and RN
 * share one implementation.
 */
internal object ProtocolEngineHostBridgeReactNative {

    private fun run(
        module: OfflineProtocolModule,
        promise: Promise,
        errorCode: String,
        errorMessage: String,
        block: (ProtocolEngineHost, Context) -> Any?,
    ) {
        try {
            val host = module.engineHost
            val context = module.reactApplicationContext
            val result = block(host, context)
            promise.resolve(BridgeResults.toReactNativeJsonStringOrPassThrough(result))
        } catch (e: Exception) {
            module.rejectBridgePromise(promise, e, errorCode, errorMessage)
        }
    }

fun acceptConnectionRequest(module: OfflineProtocolModule, recipient: String, accepterName: String, keyPackage: ReadableArray?, promise: Promise) {
    run(module, promise, "ERROR_CONNECTION_REQUEST", "Failed to accept connection request") { host, context ->
        ProtocolEngineHostBridge.acceptConnectionRequest(host, context, mapOf("recipient" to recipient, "accepterName" to accepterName, "keyPackage" to BridgeResults.readableArrayToIntList(keyPackage)))
    }
}

fun bleGetDiagnostics(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BLE", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.bleGetDiagnostics(host, context, null)
    }
}

fun bleGetPeerCount(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BLE", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.bleGetPeerCount(host, context, null)
    }
}

fun blockUser(module: OfflineProtocolModule, userId: String, promise: Promise) {
    run(module, promise, "ERROR_BLOCK_USER", "Failed to block user") { host, context ->
        ProtocolEngineHostBridge.blockUser(host, context, mapOf("userId" to userId))
    }
}

fun cancelConnectionRequest(module: OfflineProtocolModule, recipient: String, promise: Promise) {
    run(module, promise, "ERROR_CONNECTION_REQUEST", "Failed to cancel connection request") { host, context ->
        ProtocolEngineHostBridge.cancelConnectionRequest(host, context, mapOf("recipient" to recipient))
    }
}

fun cancelFileTransfer(module: OfflineProtocolModule, fileId: String, promise: Promise) {
    run(module, promise, "ERROR_FILE_CANCEL", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.cancelFileTransfer(host, context, mapOf("fileId" to fileId))
    }
}

fun checkInternetPresence(module: OfflineProtocolModule, userId: String, options: ReadableMap, promise: Promise) {
    run(module, promise, "ERROR_INTERNET", "Failed to query presence: Internet transport not initialized") { host, context ->
        ProtocolEngineHostBridge.checkInternetPresence(host, context, mapOf("userId" to userId, "options" to BridgeResults.readableMapToArgumentMap(options)))
    }
}

fun createInvite(module: OfflineProtocolModule, petname: String?, signed: Boolean, promise: Promise) {
    run(module, promise, "ERROR_NOT_INITIALIZED", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.createInvite(host, context, mapOf("petname" to petname, "signed" to signed))
    }
}

fun deriveAddress(module: OfflineProtocolModule, publicKey: ReadableArray, promise: Promise) {
    run(module, promise, "ERROR_CRYPTO", "Failed to derive address") { host, context ->
        ProtocolEngineHostBridge.deriveAddress(host, context, mapOf("publicKey" to BridgeResults.readableArrayToIntList(publicKey)))
    }
}

fun deriveUserIdFromPublicKey(module: OfflineProtocolModule, publicKey: ReadableArray, promise: Promise) {
    run(module, promise, "ERROR_NOT_INITIALIZED", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.deriveUserIdFromPublicKey(host, context, mapOf("publicKey" to BridgeResults.readableArrayToIntList(publicKey)))
    }
}

fun disableTransport(module: OfflineProtocolModule, type: String, promise: Promise) {
    run(module, promise, "ERROR_TRANSPORT_DISABLE", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.disableTransport(host, context, mapOf("type" to type))
    }
}

fun discoverServices(module: OfflineProtocolModule, serviceId: String?, promise: Promise) {
    run(module, promise, "ERROR_DISCOVER_SERVICES", "Failed to discover services") { host, context ->
        ProtocolEngineHostBridge.discoverServices(host, context, mapOf("serviceId" to serviceId))
    }
}

fun eraseCustody(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_ERASECUSTODY", "eraseCustody failed") { host, context ->
        ProtocolEngineHostBridge.eraseCustody(host, context, null)
    }
}

fun establishSecureSession(module: OfflineProtocolModule, peerId: String, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.establishSecureSession(host, context, mapOf("peerId" to peerId))
    }
}

fun finalizeFile(module: OfflineProtocolModule, fileId: String, promise: Promise) {
    run(module, promise, "ERROR_FILE", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.finalizeFile(host, context, mapOf("fileId" to fileId))
    }
}

fun forceTransport(module: OfflineProtocolModule, transportType: String, promise: Promise) {
    run(module, promise, "ERROR_TRANSPORT", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.forceTransport(host, context, mapOf("transportType" to transportType))
    }
}

fun forwardMessage(module: OfflineProtocolModule, originalMessageJson: String, newRecipient: String, priority: Int, promise: Promise) {
    run(module, promise, "ERROR_FORWARD", "Failed to forward message") { host, context ->
        ProtocolEngineHostBridge.forwardMessage(host, context, mapOf("originalMessageJson" to originalMessageJson, "newRecipient" to newRecipient, "priority" to priority))
    }
}

fun getActiveTransports(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_TRANSPORT", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getActiveTransports(host, context, null)
    }
}

fun getBatteryLevel(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BATTERY", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getBatteryLevel(host, context, null)
    }
}

fun getBlockedUsers(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_GET_BLOCKED", "Failed to get blocked users") { host, context ->
        ProtocolEngineHostBridge.getBlockedUsers(host, context, null)
    }
}

fun getCustodyStats(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_STATS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getCustodyStats(host, context, null)
    }
}

fun getDedupStats(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_STATS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getDedupStats(host, context, null)
    }
}

fun getDeliverySuccessRate(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_DELIVERY_RATE", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getDeliverySuccessRate(host, context, null)
    }
}

fun getDorsConfig(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_CONFIG", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getDorsConfig(host, context, null)
    }
}

fun getEstablishmentState(module: OfflineProtocolModule, peerId: String, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Failed to get establishment state") { host, context ->
        ProtocolEngineHostBridge.getEstablishmentState(host, context, mapOf("peerId" to peerId))
    }
}

fun getFileProgress(module: OfflineProtocolModule, fileId: String, promise: Promise) {
    run(module, promise, "ERROR_FILE_PROGRESS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getFileProgress(host, context, mapOf("fileId" to fileId))
    }
}

fun getIdentityPublicKey(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_NOT_INITIALIZED", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getIdentityPublicKey(host, context, null)
    }
}

fun getIsCharging(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run getIsCharging") { host, context ->
        ProtocolEngineHostBridge.getIsCharging(host, context, null)
    }
}

fun getMedianHops(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_MEDIAN_HOPS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getMedianHops(host, context, null)
    }
}

fun getMedianLatency(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_MEDIAN_LATENCY", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getMedianLatency(host, context, null)
    }
}

fun getMeshRelayStats(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_STATS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getMeshRelayStats(host, context, null)
    }
}

fun getMeshRelayTunables(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_STATS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getMeshRelayTunables(host, context, null)
    }
}

fun getMessageStats(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_MESSAGE_STATS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getMessageStats(host, context, null)
    }
}

fun getPendingAckCount(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_STATS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getPendingAckCount(host, context, null)
    }
}

fun getRelayConfig(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_RELAY", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getRelayConfig(host, context, null)
    }
}

fun getRelayPriority(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run getRelayPriority") { host, context ->
        ProtocolEngineHostBridge.getRelayPriority(host, context, null)
    }
}

fun getRetryQueueSize(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_STATS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getRetryQueueSize(host, context, null)
    }
}

fun getState(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run getState") { host, context ->
        ProtocolEngineHostBridge.getState(host, context, null)
    }
}

fun getTopology(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_TOPOLOGY", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getTopology(host, context, null)
    }
}

fun getTransportMetrics(module: OfflineProtocolModule, transportType: String, promise: Promise) {
    run(module, promise, "ERROR_METRICS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.getTransportMetrics(host, context, mapOf("transportType" to transportType))
    }
}

fun hasPendingKeyPackage(module: OfflineProtocolModule, peerId: String, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run hasPendingKeyPackage") { host, context ->
        ProtocolEngineHostBridge.hasPendingKeyPackage(host, context, mapOf("peerId" to peerId))
    }
}

fun internetConfirmSent(module: OfflineProtocolModule, messageId: String, promise: Promise) {
    run(module, promise, "ERROR_INTERNET", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.internetConfirmSent(host, context, mapOf("messageId" to messageId))
    }
}

fun internetForceReconnect(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run internetForceReconnect") { host, context ->
        ProtocolEngineHostBridge.internetForceReconnect(host, context, null)
    }
}

fun internetGetNextMessage(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_INTERNET", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.internetGetNextMessage(host, context, null)
    }
}

fun internetIsReady(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run internetIsReady") { host, context ->
        ProtocolEngineHostBridge.internetIsReady(host, context, null)
    }
}

fun internetIsSuperseded(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run internetIsSuperseded") { host, context ->
        ProtocolEngineHostBridge.internetIsSuperseded(host, context, null)
    }
}

fun internetMessageReceived(module: OfflineProtocolModule, senderId: String, data: ReadableArray, promise: Promise) {
    run(module, promise, "ERROR_INTERNET", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.internetMessageReceived(host, context, mapOf("senderId" to senderId, "data" to BridgeResults.readableArrayToIntList(data)))
    }
}

fun internetSendFailed(module: OfflineProtocolModule, messageId: String, promise: Promise) {
    run(module, promise, "ERROR_INTERNET", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.internetSendFailed(host, context, mapOf("messageId" to messageId))
    }
}

fun internetSendRawCommand(module: OfflineProtocolModule, json: String, promise: Promise) {
    run(module, promise, "ERROR_INTERNET", "Failed to send raw server command: Internet transport not initialized") { host, context ->
        ProtocolEngineHostBridge.internetSendRawCommand(host, context, mapOf("json" to json))
    }
}

fun internetStatusChanged(module: OfflineProtocolModule, isConnected: Boolean, promise: Promise) {
    run(module, promise, "ERROR_INTERNET", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.internetStatusChanged(host, context, mapOf("isConnected" to isConnected))
    }
}

fun isBluetoothEnabled(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run isBluetoothEnabled") { host, context ->
        ProtocolEngineHostBridge.isBluetoothEnabled(host, context, null)
    }
}

fun isMlsInitialized(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run isMlsInitialized") { host, context ->
        ProtocolEngineHostBridge.isMlsInitialized(host, context, null)
    }
}

fun isRelay(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run isRelay") { host, context ->
        ProtocolEngineHostBridge.isRelay(host, context, null)
    }
}

fun isUserBlocked(module: OfflineProtocolModule, userId: String, promise: Promise) {
    run(module, promise, "ERROR_IS_BLOCKED", "Failed to check blocked status") { host, context ->
        ProtocolEngineHostBridge.isUserBlocked(host, context, mapOf("userId" to userId))
    }
}

fun mlsCreateSession(module: OfflineProtocolModule, otherUserId: String, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsCreateSession(host, context, mapOf("otherUserId" to otherUserId))
    }
}

fun mlsDecrypt(module: OfflineProtocolModule, encryptedJson: String, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsDecrypt(host, context, mapOf("encryptedJson" to encryptedJson))
    }
}

fun mlsDecryptFromUser(module: OfflineProtocolModule, encryptedJson: String, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsDecryptFromUser(host, context, mapOf("encryptedJson" to encryptedJson))
    }
}

fun mlsDeleteSession(module: OfflineProtocolModule, otherUserId: String, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsDeleteSession(host, context, mapOf("otherUserId" to otherUserId))
    }
}

fun mlsEncryptForUser(module: OfflineProtocolModule, otherUserId: String, plaintext: ReadableArray, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsEncryptForUser(host, context, mapOf("otherUserId" to otherUserId, "plaintext" to BridgeResults.readableArrayToIntList(plaintext)))
    }
}

fun mlsGenerateKeyPackage(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsGenerateKeyPackage(host, context, null)
    }
}

fun mlsGetOrCreateKeyPackage(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsGetOrCreateKeyPackage(host, context, null)
    }
}

fun mlsGetPendingKeyPackages(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsGetPendingKeyPackages(host, context, null)
    }
}

fun mlsHasSession(module: OfflineProtocolModule, otherUserId: String, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run mlsHasSession") { host, context ->
        ProtocolEngineHostBridge.mlsHasSession(host, context, mapOf("otherUserId" to otherUserId))
    }
}

fun mlsImportKeyPackage(module: OfflineProtocolModule, userId: String, keyPackageData: ReadableArray, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsImportKeyPackage(host, context, mapOf("userId" to userId, "keyPackageData" to BridgeResults.readableArrayToIntList(keyPackageData)))
    }
}

fun mlsJoinSession(module: OfflineProtocolModule, welcomeJson: String, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsJoinSession(host, context, mapOf("welcomeJson" to welcomeJson))
    }
}

fun mlsListSessions(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BRIDGE", "Failed to run mlsListSessions") { host, context ->
        ProtocolEngineHostBridge.mlsListSessions(host, context, null)
    }
}

fun mlsMarkKeyPackageSynced(module: OfflineProtocolModule, packageId: String, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsMarkKeyPackageSynced(host, context, mapOf("packageId" to packageId))
    }
}

fun mlsProcessWelcome(module: OfflineProtocolModule, welcomeJson: String, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.mlsProcessWelcome(host, context, mapOf("welcomeJson" to welcomeJson))
    }
}

fun parseInvite(module: OfflineProtocolModule, blob: String, promise: Promise) {
    run(module, promise, "ERROR_INVALID_INVITE", "Invite did not verify") { host, context ->
        ProtocolEngineHostBridge.parseInvite(host, context, mapOf("blob" to blob))
    }
}

fun processFileChunk(module: OfflineProtocolModule, fileId: String, chunkIndex: Int, totalChunks: Int, fileSize: Double, fileName: String, fileChecksum: String, data: ReadableArray, promise: Promise) {
    run(module, promise, "ERROR_FILE", "Failed to process file chunk") { host, context ->
        ProtocolEngineHostBridge.processFileChunk(host, context, mapOf("fileId" to fileId, "chunkIndex" to chunkIndex, "totalChunks" to totalChunks, "fileSize" to fileSize, "fileName" to fileName, "fileChecksum" to fileChecksum, "data" to BridgeResults.readableArrayToIntList(data)))
    }
}

fun registerService(module: OfflineProtocolModule, serviceId: String, version: String, capabilitiesJson: String, promise: Promise) {
    run(module, promise, "ERROR_REGISTER_SERVICE", "Failed to register service") { host, context ->
        ProtocolEngineHostBridge.registerService(host, context, mapOf("serviceId" to serviceId, "version" to version, "capabilitiesJson" to capabilitiesJson))
    }
}

fun rejectConnectionRequest(module: OfflineProtocolModule, recipient: String, promise: Promise) {
    run(module, promise, "ERROR_CONNECTION_REQUEST", "Failed to reject connection request") { host, context ->
        ProtocolEngineHostBridge.rejectConnectionRequest(host, context, mapOf("recipient" to recipient))
    }
}

fun rekeySession(module: OfflineProtocolModule, peerId: String, promise: Promise) {
    run(module, promise, "ERROR_MLS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.rekeySession(host, context, mapOf("peerId" to peerId))
    }
}

fun releaseTransportLock(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_TRANSPORT", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.releaseTransportLock(host, context, null)
    }
}

fun requestEnableBluetooth(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_BLUETOOTH", "Bluetooth not available on this device") { host, context ->
        ProtocolEngineHostBridge.requestEnableBluetooth(host, context, null)
    }
}

fun resolveUsername(module: OfflineProtocolModule, username: String, promise: Promise) {
    run(module, promise, "ERROR_INVALID_ARGUMENT", "Failed to resolve username") { host, context ->
        ProtocolEngineHostBridge.resolveUsername(host, context, mapOf("username" to username))
    }
}

fun respondToServiceRequest(module: OfflineProtocolModule, requestId: String, requester: String, serviceId: String, status: String, body: String, promise: Promise) {
    run(module, promise, "ERROR_SERVICE_RESPONSE", "Failed to respond to service request") { host, context ->
        ProtocolEngineHostBridge.respondToServiceRequest(host, context, mapOf("requestId" to requestId, "requester" to requester, "serviceId" to serviceId, "status" to status, "body" to body))
    }
}

fun sendConnectionRequest(module: OfflineProtocolModule, recipient: String, senderName: String, keyPackage: ReadableArray?, initialMessage: String?, promise: Promise) {
    run(module, promise, "ERROR_CONNECTION_REQUEST", "Failed to send connection request") { host, context ->
        ProtocolEngineHostBridge.sendConnectionRequest(host, context, mapOf("recipient" to recipient, "senderName" to senderName, "keyPackage" to BridgeResults.readableArrayToIntList(keyPackage), "initialMessage" to initialMessage))
    }
}

fun sendFileFromPath(module: OfflineProtocolModule, recipient: String, filePath: String, fileName: String, promise: Promise) {
    run(module, promise, "ERROR_SEND_FILE", "Failed to send file from path") { host, context ->
        ProtocolEngineHostBridge.sendFileFromPath(host, context, mapOf("recipient" to recipient, "filePath" to filePath, "fileName" to fileName))
    }
}

fun sendMedia(module: OfflineProtocolModule, recipient: String, fileData: String, fileName: String, contentType: String, mediaMetadata: ReadableMap?, promise: Promise) {
    run(module, promise, "ERROR_SEND_MEDIA", "Failed to send media") { host, context ->
        ProtocolEngineHostBridge.sendMedia(host, context, mapOf("recipient" to recipient, "fileData" to fileData, "fileName" to fileName, "contentType" to contentType, "mediaMetadata" to BridgeResults.readableMapToArgumentMap(mediaMetadata)))
    }
}

fun sendMediaRich(module: OfflineProtocolModule, recipient: String, fileData: String, fileName: String, contentType: String, options: ReadableMap?, promise: Promise) {
    run(module, promise, "ERROR_SEND_MEDIA", "Failed to send media") { host, context ->
        ProtocolEngineHostBridge.sendMediaRich(host, context, mapOf("recipient" to recipient, "fileData" to fileData, "fileName" to fileName, "contentType" to contentType, "options" to BridgeResults.readableMapToArgumentMap(options)))
    }
}

fun sendMessageRich(module: OfflineProtocolModule, recipient: String, content: String, priority: Int, replyToMsg: String?, options: ReadableMap?, promise: Promise) {
    run(module, promise, "ERROR_SEND", "Failed to send rich message") { host, context ->
        ProtocolEngineHostBridge.sendMessageRich(host, context, mapOf("recipient" to recipient, "content" to content, "priority" to priority, "replyToMsg" to replyToMsg, "options" to BridgeResults.readableMapToArgumentMap(options)))
    }
}

fun sendPresenceUpdate(module: OfflineProtocolModule, recipient: String, status: Int, promise: Promise) {
    run(module, promise, "ERROR_PRESENCE_UPDATE", "Failed to send presence update") { host, context ->
        ProtocolEngineHostBridge.sendPresenceUpdate(host, context, mapOf("recipient" to recipient, "status" to status))
    }
}

fun sendReadReceipt(module: OfflineProtocolModule, recipient: String, messageIds: ReadableArray, promise: Promise) {
    run(module, promise, "ERROR_READ_RECEIPT", "Failed to send read receipt") { host, context ->
        ProtocolEngineHostBridge.sendReadReceipt(host, context, mapOf("recipient" to recipient, "messageIds" to BridgeResults.readableArrayToIntList(messageIds)))
    }
}

fun sendServiceRequest(module: OfflineProtocolModule, provider: String, serviceId: String, method: String, body: String, promise: Promise) {
    run(module, promise, "ERROR_SERVICE_REQUEST", "Failed to send service request") { host, context ->
        ProtocolEngineHostBridge.sendServiceRequest(host, context, mapOf("provider" to provider, "serviceId" to serviceId, "method" to method, "body" to body))
    }
}

fun sendTypingIndicator(module: OfflineProtocolModule, recipient: String, conversationId: String, isTyping: Boolean, promise: Promise) {
    run(module, promise, "ERROR_TYPING_INDICATOR", "Failed to send typing indicator") { host, context ->
        ProtocolEngineHostBridge.sendTypingIndicator(host, context, mapOf("recipient" to recipient, "conversationId" to conversationId, "isTyping" to isTyping))
    }
}

fun setBatteryLevel(module: OfflineProtocolModule, level: Int, promise: Promise) {
    run(module, promise, "ERROR_BATTERY", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.setBatteryLevel(host, context, mapOf("level" to level))
    }
}

fun setBatteryState(module: OfflineProtocolModule, level: Int, isCharging: Boolean, promise: Promise) {
    run(module, promise, "ERROR_BATTERY", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.setBatteryState(host, context, mapOf("level" to level, "isCharging" to isCharging))
    }
}

fun setRelayPriority(module: OfflineProtocolModule, priorityString: String, promise: Promise) {
    run(module, promise, "ERROR_RELAY", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.setRelayPriority(host, context, mapOf("priorityString" to priorityString))
    }
}

fun shouldEscalateToWifi(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_DORS", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.shouldEscalateToWifi(host, context, null)
    }
}

fun signData(module: OfflineProtocolModule, data: ReadableArray, promise: Promise) {
    run(module, promise, "ERROR_NOT_INITIALIZED", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.signData(host, context, mapOf("data" to BridgeResults.readableArrayToIntList(data)))
    }
}

fun signDataBase64(module: OfflineProtocolModule, dataBase64: String, promise: Promise) {
    run(module, promise, "ERROR_NOT_INITIALIZED", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.signDataBase64(host, context, mapOf("dataBase64" to dataBase64))
    }
}

fun unblockUser(module: OfflineProtocolModule, userId: String, promise: Promise) {
    run(module, promise, "ERROR_UNBLOCK_USER", "Failed to unblock user") { host, context ->
        ProtocolEngineHostBridge.unblockUser(host, context, mapOf("userId" to userId))
    }
}

fun unregisterService(module: OfflineProtocolModule, serviceId: String, promise: Promise) {
    run(module, promise, "ERROR_UNREGISTER_SERVICE", "Failed to unregister service") { host, context ->
        ProtocolEngineHostBridge.unregisterService(host, context, mapOf("serviceId" to serviceId))
    }
}

fun updateAckConfig(module: OfflineProtocolModule, configJson: String, promise: Promise) {
    run(module, promise, "ERROR_CONFIG", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.updateAckConfig(host, context, configJson)
    }
}

fun updateDedupConfig(module: OfflineProtocolModule, configJson: String, promise: Promise) {
    run(module, promise, "ERROR_CONFIG", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.updateDedupConfig(host, context, configJson)
    }
}

fun updateDorsConfig(module: OfflineProtocolModule, configJson: String, promise: Promise) {
    run(module, promise, "ERROR_CONFIG", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.updateDorsConfig(host, context, configJson)
    }
}

fun updateRelayConfig(module: OfflineProtocolModule, configJson: String, promise: Promise) {
    run(module, promise, "ERROR_RELAY", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.updateRelayConfig(host, context, configJson)
    }
}

fun updateRetryConfig(module: OfflineProtocolModule, configJson: String, promise: Promise) {
    run(module, promise, "ERROR_CONFIG", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.updateRetryConfig(host, context, configJson)
    }
}

fun verifySignature(module: OfflineProtocolModule, publicKey: ReadableArray, data: ReadableArray, signature: ReadableArray, promise: Promise) {
    run(module, promise, "ERROR_NOT_INITIALIZED", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.verifySignature(host, context, mapOf("publicKey" to BridgeResults.readableArrayToIntList(publicKey), "data" to BridgeResults.readableArrayToIntList(data), "signature" to BridgeResults.readableArrayToIntList(signature)))
    }
}

fun verifySignatureBase64(module: OfflineProtocolModule, publicKeyBase64: String, dataBase64: String, signatureBase64: String, promise: Promise) {
    run(module, promise, "ERROR_NOT_INITIALIZED", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.verifySignatureBase64(host, context, mapOf("publicKeyBase64" to publicKeyBase64, "dataBase64" to dataBase64, "signatureBase64" to signatureBase64))
    }
}

fun wifiDirectGetNextMessage(module: OfflineProtocolModule, promise: Promise) {
    run(module, promise, "ERROR_WIFI_DIRECT", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.wifiDirectGetNextMessage(host, context, null)
    }
}

fun wifiDirectMessageReceived(module: OfflineProtocolModule, senderId: String, data: ReadableArray, promise: Promise) {
    run(module, promise, "ERROR_WIFI_DIRECT", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.wifiDirectMessageReceived(host, context, mapOf("senderId" to senderId, "data" to BridgeResults.readableArrayToIntList(data)))
    }
}

fun wifiDirectPeerConnected(module: OfflineProtocolModule, peerId: String, promise: Promise) {
    run(module, promise, "ERROR_WIFI_DIRECT", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.wifiDirectPeerConnected(host, context, mapOf("peerId" to peerId))
    }
}

fun wifiDirectPeerDisconnected(module: OfflineProtocolModule, peerId: String, promise: Promise) {
    run(module, promise, "ERROR_WIFI_DIRECT", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.wifiDirectPeerDisconnected(host, context, mapOf("peerId" to peerId))
    }
}

fun wifiDirectStatusChanged(module: OfflineProtocolModule, isConnected: Boolean, promise: Promise) {
    run(module, promise, "ERROR_WIFI_DIRECT", "Protocol not initialized") { host, context ->
        ProtocolEngineHostBridge.wifiDirectStatusChanged(host, context, mapOf("isConnected" to isConnected))
    }
}

fun wipePersistedState(module: OfflineProtocolModule, appId: String, profile: String, promise: Promise) {
    run(module, promise, "ERROR_WIPE_STATE", "Refusing to wipe storage for the account this instance is running.") { host, context ->
        ProtocolEngineHostBridge.wipePersistedState(host, context, mapOf("appId" to appId, "profile" to profile))
    }
}

}
