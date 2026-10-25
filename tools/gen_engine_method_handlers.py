#!/usr/bin/env python3
"""Generate EngineMethodHandlers.kt for the Flutter Android plugin."""

from __future__ import annotations

import pathlib
import textwrap

REPO = pathlib.Path(__file__).resolve().parents[1]
METHODS = (
    REPO / "bindings/flutter/offline_protocol/method_channel_methods.txt"
).read_text().splitlines()
METHODS = [m.strip() for m in METHODS if m.strip()]

OUT = (
    REPO
    / "bindings/flutter/offline_protocol/android/src/main/kotlin/com/offlineprotocol/flutter/EngineMethodHandlers.kt"
)

HEADER = textwrap.dedent(
    """
    package com.offlineprotocol.flutter

    import com.offlineprotocol.ProtocolEngineHost
    import org.json.JSONObject

    /**
     * Per-method bodies for [FlutterEngineChannel]. Arguments are the Dart-side
     * maps from `mesh_sdk.dart` (named keys, camelCase).
     */
    internal object EngineMethodHandlers {
    """
)

FOOTER = "}\n"


def fn(name: str, body: str) -> str:
    indented = textwrap.indent(textwrap.dedent(body).strip(), "        ")
    return f"\n    @JvmStatic\n    fun {name}(\n        host: ProtocolEngineHost,\n        env: FlutterEngineChannel.Environment,\n        arguments: Any?,\n    ): Any? {{\n{indented}\n    }}\n"


# Hand-written implementations (production logic shared with RN module).
CUSTOM: dict[str, str] = {}

CUSTOM["create"] = """
    val configJson = arguments as? String
        ?: throw IllegalArgumentException("create expects a JSON string")
    env.createOnEngine(configJson)
    return null
"""

CUSTOM["destroy"] = """
    env.tearDownProtocol()
    return null
"""

CUSTOM["start"] = """
    env.startOnEngine()
    return null
"""

CUSTOM["stop"] = """
    env.stopOnEngine()
    return null
"""

CUSTOM["initializeMlsWithSecureStorage"] = """
    host.initializeMlsWithSecureStorage(env.context)
    return null
"""

CUSTOM["replayEvents"] = """
    val map = arguments as? Map<*, *>
        ?: throw IllegalArgumentException("replayEvents expects a map")
    val fromSeq = FlutterBridgeArgs.requireInt(map, "fromSeq")
    val toSeq = FlutterBridgeArgs.requireInt(map, "toSeq")
    return env.replayEvents(fromSeq, toSeq)
"""

CUSTOM["enableTransport"] = """
    val map = arguments as? Map<*, *>
        ?: throw IllegalArgumentException("enableTransport expects a map")
    val type = FlutterBridgeArgs.requireString(map, "type")
    val configJson = map["config"] as? String
    val json = configJson?.let { JSONObject(it) }
    host.enableTransport(env.context, type, json, env.internetHandlers) {
        env.discardOneShotEvent(com.offlineprotocol.SupersededLatchPolicy.EVENT_TYPE)
    }
    return null
"""

CUSTOM["sendMessage"] = """
    val map = arguments as? Map<*, *>
        ?: throw IllegalArgumentException("sendMessage expects a map")
    val recipient = FlutterBridgeArgs.requireString(map, "recipient")
    val content = FlutterBridgeArgs.requireString(map, "content")
    val priority = FlutterBridgeArgs.optionalInt(map, "priority") ?: 1
    val replyTo = map["replyToMsg"] as? String
    return host.sendMessage(recipient, content, priority, replyTo)
"""

CUSTOM["localAddress"] = """
    return host.protocol?.localAddress()
"""

CUSTOM["enableTelemetry"] = """
    @Suppress("UNCHECKED_CAST")
    val config = arguments as? Map<String, Any?>
    host.enableTelemetry(config, env.seedAppState())
    return null
"""

CUSTOM["telemetryStats"] = """
    val stats = host.telemetryStats() ?: return null
    return host.telemetryStatsMap(stats)
"""

CUSTOM["telemetryInstallId"] = """
    return host.telemetryInstallId()
"""

CUSTOM["disableTelemetry"] = """
    host.disableTelemetry()
    return null
"""

CUSTOM["flushTelemetry"] = """
    host.flushTelemetry()
    return null
"""

CUSTOM["endTelemetrySession"] = """
    host.endTelemetrySession()
    return null
"""

CUSTOM["setTelemetryEnabled"] = """
    val enabled = when (arguments) {
        is Boolean -> arguments
        is Map<*, *> -> FlutterBridgeArgs.requireBoolean(arguments, "enabled")
        else -> throw IllegalArgumentException("setTelemetryEnabled expects boolean or map")
    }
    host.setTelemetryEnabled(enabled)
    return null
"""

CUSTOM["emitTestEvent"] = """
    host.requireProtocolBridge().emitTestEvent()
    return null
"""

CUSTOM["requestEnableBluetooth"] = """
    val activity = env.activityProvider()
        ?: throw IllegalStateException("No activity available to show Bluetooth enable dialog")
    return com.offlineprotocol.ProtocolEngineHostBridge.requestEnableBluetooth(host, activity, arguments)
"""

CUSTOM["isBluetoothEnabled"] = """
    return com.offlineprotocol.ProtocolEngineHostBridge.isBluetoothEnabled(host, env.context, arguments)
"""

CUSTOM["wipePersistedState"] = """
    return com.offlineprotocol.ProtocolEngineHostBridge.wipePersistedState(host, env.context, arguments)
"""

CUSTOM["pause"] = """
    host.pause()
    return null
"""

CUSTOM["resume"] = """
    host.resume()
    return null
"""

CUSTOM["receiveMessage"] = """
    return host.receiveMessage()
"""

# Data layer: map keys spaceId, docId, collection, and so on.
DATA_METHODS = {
    "dataCreateDoc": "host.dataCreateDoc(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"))",
    "dataDeleteDoc": "host.dataDeleteDoc(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"))",
    "dataRemoveDoc": "host.dataRemoveDoc(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"))",
    "dataRemoveSpace": "host.dataRemoveSpace(FlutterBridgeArgs.requireString(map, \"spaceId\"))",
    "dataFetchAttachmentFrom": "host.dataFetchAttachmentFrom(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"peerId\"), FlutterBridgeArgs.requireString(map, \"hash\"))",
    "dataSetInterest": "host.dataSetInterest(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.stringList(map, \"patterns\"))",
    "dataListDocs": "return host.dataListDocs(FlutterBridgeArgs.requireString(map, \"spaceId\"))",
    "dataListSpaces": "return host.dataListSpaces()",
    "dataMapSet": "host.dataMapSet(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"), FlutterBridgeArgs.requireString(map, \"key\"), FlutterBridgeArgs.requireString(map, \"valueJson\"))",
    "dataMapDelete": "host.dataMapDelete(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"), FlutterBridgeArgs.requireString(map, \"key\"))",
    "dataMapGetJson": "return host.dataMapGetJson(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"), FlutterBridgeArgs.requireString(map, \"key\"))",
    "dataListPush": "host.dataListPush(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"), FlutterBridgeArgs.requireString(map, \"valueJson\"))",
    "dataListDelete": "host.dataListDelete(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"), FlutterBridgeArgs.requireDouble(map, \"index\"), FlutterBridgeArgs.requireDouble(map, \"count\"))",
    "dataListLen": "return host.dataListLen(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"))",
    "dataTextInsert": "host.dataTextInsert(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"), FlutterBridgeArgs.requireDouble(map, \"position\"), FlutterBridgeArgs.requireString(map, \"text\"))",
    "dataTextDelete": "host.dataTextDelete(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"), FlutterBridgeArgs.requireDouble(map, \"position\"), FlutterBridgeArgs.requireDouble(map, \"count\"))",
    "dataTextValue": "return host.dataTextValue(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"))",
    "dataCounterIncrement": "host.dataCounterIncrement(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"), FlutterBridgeArgs.requireDouble(map, \"amount\"))",
    "dataCounterValue": "return host.dataCounterValue(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"), FlutterBridgeArgs.requireString(map, \"collection\"))",
    "dataDocJson": "return host.dataDocJson(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"))",
    "dataExportRaw": "return host.dataExportRaw(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"))",
    "dataFlush": "host.dataFlush(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"))",
    "dataFlushAll": "host.dataFlushAll()",
    "dataDocSize": "return host.dataDocSize(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"docId\"))",
    "dataAttachmentHash": "return host.dataAttachmentHash(FlutterBridgeArgs.requireString(map, \"bytesBase64\"))",
    "dataFetchAttachment": "host.dataFetchAttachment(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"hash\"))",
    "dataProvideAttachment": "host.dataProvideAttachment(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"peerId\"), FlutterBridgeArgs.requireString(map, \"hash\"), FlutterBridgeArgs.requireString(map, \"bytesBase64\"))",
    "dataDeclineAttachment": "host.dataDeclineAttachment(FlutterBridgeArgs.requireString(map, \"spaceId\"), FlutterBridgeArgs.requireString(map, \"peerId\"), FlutterBridgeArgs.requireString(map, \"hash\"))",
    "dataWipeAll": "host.dataWipeAll()",
}

for name, call in DATA_METHODS.items():
    if call.startswith("return"):
        CUSTOM[name] = f"""
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("{name} expects a map")
    {call}
"""
    else:
        CUSTOM[name] = f"""
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("{name} expects a map")
    {call}
    return null
"""

MESH = {
    "meshCreateGroup": "return host.meshCreateGroup(FlutterBridgeArgs.requireString(map, \"name\"))",
    "meshInviteToGroup": "host.meshInviteToGroup(FlutterBridgeArgs.requireString(map, \"groupId\"), FlutterBridgeArgs.requireString(map, \"inviteeUserId\"))",
    "meshSendGroupMessage": "return host.meshSendGroupMessage(FlutterBridgeArgs.requireString(map, \"groupId\"), FlutterBridgeArgs.requireString(map, \"content\"), map[\"priority\"] as? String, map[\"replyToMsg\"] as? String)",
    "meshForwardMessageToGroup": "return host.meshForwardMessageToGroup(FlutterBridgeArgs.requireString(map, \"originalMessageJson\"), FlutterBridgeArgs.requireString(map, \"groupId\"), map[\"priority\"] as? String)",
    "meshRemoveFromGroup": "host.meshRemoveFromGroup(FlutterBridgeArgs.requireString(map, \"groupId\"), FlutterBridgeArgs.requireString(map, \"memberId\"))",
    "meshLeaveGroup": "host.meshLeaveGroup(FlutterBridgeArgs.requireString(map, \"groupId\"))",
    "meshListGroups": "return host.meshListGroups()",
    "meshGetGroupInfo": "return host.meshGetGroupInfo(FlutterBridgeArgs.requireString(map, \"groupId\"))",
    "meshGroupRichReadiness": "return host.meshGroupRichReadiness(FlutterBridgeArgs.requireString(map, \"groupId\"))",
    "meshGroupRelaySyncState": "return host.meshGroupRelaySyncState(FlutterBridgeArgs.requireString(map, \"groupId\"))",
    "meshRequestGroupRelayRegistration": "return host.meshRequestGroupRelayRegistration(FlutterBridgeArgs.requireString(map, \"groupId\"))",
    "meshSetMemberRole": "host.meshSetMemberRole(FlutterBridgeArgs.requireString(map, \"groupId\"), FlutterBridgeArgs.requireString(map, \"userId\"), FlutterBridgeArgs.requireString(map, \"role\"))",
    "meshGetMemberRole": "return host.meshGetMemberRole(FlutterBridgeArgs.requireString(map, \"groupId\"), FlutterBridgeArgs.requireString(map, \"userId\"))",
    "meshGetGroupRoles": "return host.meshGetGroupRoles(FlutterBridgeArgs.requireString(map, \"groupId\"))",
    "meshRenameGroup": "host.meshRenameGroup(FlutterBridgeArgs.requireString(map, \"groupId\"), FlutterBridgeArgs.requireString(map, \"newName\"))",
}

for name, call in MESH.items():
    if call.startswith("return"):
        CUSTOM[name] = f"""
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("{name} expects a map")
    {call}
"""
    else:
        CUSTOM[name] = f"""
    val map = arguments as? Map<*, *> ?: throw IllegalArgumentException("{name} expects a map")
    {call}
    return null
"""

# Remaining methods delegate to host bridge (ProtocolEngineHostBridge.kt)
BRIDGE_DELEGATE = [
    "acceptConnectionRequest",
    "bleGetDiagnostics",
    "bleGetPeerCount",
    "blockUser",
    "cancelConnectionRequest",
    "cancelFileTransfer",
    "checkInternetPresence",
    "createInvite",
    "deriveAddress",
    "deriveUserIdFromPublicKey",
    "disableTransport",
    "discoverServices",
    "enableTelemetry",
    "eraseCustody",
    "establishSecureSession",
    "finalizeFile",
    "forceTransport",
    "forwardMessage",
    "getActiveTransports",
    "getBatteryLevel",
    "getBlockedUsers",
    "getCustodyStats",
    "getDedupStats",
    "getDeliverySuccessRate",
    "getDorsConfig",
    "getEstablishmentState",
    "getFileProgress",
    "getIdentityPublicKey",
    "getIsCharging",
    "getMedianHops",
    "getMedianLatency",
    "getMeshRelayStats",
    "getMeshRelayTunables",
    "getMessageStats",
    "getPendingAckCount",
    "getRelayConfig",
    "getRelayPriority",
    "getRetryQueueSize",
    "getState",
    "getTopology",
    "getTransportMetrics",
    "hasPendingKeyPackage",
    "internetConfirmSent",
    "internetForceReconnect",
    "internetGetNextMessage",
    "internetIsReady",
    "internetIsSuperseded",
    "internetMessageReceived",
    "internetSendFailed",
    "internetSendRawCommand",
    "internetStatusChanged",
    "isBluetoothEnabled",
    "isMlsInitialized",
    "isRelay",
    "isUserBlocked",
    "mlsCreateSession",
    "mlsDecrypt",
    "mlsDecryptFromUser",
    "mlsDeleteSession",
    "mlsEncryptForUser",
    "mlsGenerateKeyPackage",
    "mlsGetOrCreateKeyPackage",
    "mlsGetPendingKeyPackages",
    "mlsHasSession",
    "mlsImportKeyPackage",
    "mlsJoinSession",
    "mlsListSessions",
    "mlsMarkKeyPackageSynced",
    "mlsProcessWelcome",
    "parseInvite",
    "processFileChunk",
    "registerService",
    "rejectConnectionRequest",
    "rekeySession",
    "releaseTransportLock",
    "requestEnableBluetooth",
    "resolveUsername",
    "respondToServiceRequest",
    "sendConnectionRequest",
    "sendFileFromPath",
    "sendMedia",
    "sendMediaRich",
    "sendMessageRich",
    "sendPresenceUpdate",
    "sendReadReceipt",
    "sendServiceRequest",
    "sendTypingIndicator",
    "setBatteryLevel",
    "setBatteryState",
    "setRelayPriority",
    "shouldEscalateToWifi",
    "signData",
    "signDataBase64",
    "unblockUser",
    "unregisterService",
    "updateAckConfig",
    "updateDedupConfig",
    "updateDorsConfig",
    "updateRelayConfig",
    "updateRetryConfig",
    "verifySignature",
    "verifySignatureBase64",
    "wifiDirectGetNextMessage",
    "wifiDirectMessageReceived",
    "wifiDirectPeerConnected",
    "wifiDirectPeerDisconnected",
    "wifiDirectStatusChanged",
    "wipePersistedState",
]

for m in BRIDGE_DELEGATE:
    if m not in CUSTOM:
        CUSTOM[m] = f"""
    return com.offlineprotocol.ProtocolEngineHostBridge.{m}(host, env.context, arguments)
"""

def render() -> str:
    parts = [HEADER]
    for m in METHODS:
        body = CUSTOM.get(m)
        if not body:
            body = f'return com.offlineprotocol.ProtocolEngineHostBridge.{m}(host, env.context, arguments)'
        parts.append(fn(m, body))
    parts.append(FOOTER)
    return "".join(parts)


def main() -> None:
    import sys

    rendered = render()
    missing = [m for m in METHODS if m not in CUSTOM]
    if "--check" in sys.argv:
        if OUT.read_text() != rendered:
            print(f"{OUT.relative_to(REPO)} is stale; run {pathlib.Path(__file__).name}", file=sys.stderr)
            raise SystemExit(1)
        print(f"EngineMethodHandlers.kt: OK ({len(METHODS)} methods, {len(missing)} default-delegate)")
        return
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(rendered)
    print(f"Wrote {OUT} ({len(METHODS)} methods, {len(missing)} default-delegate)")


if __name__ == "__main__":
    main()
