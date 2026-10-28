
import 'package:flutter/services.dart';

/// Typed wrappers for every native method-channel entry point (React Native parity).
class MeshSdk {
  MeshSdk._();

  static const MethodChannel _channel =
      MethodChannel('com.offlineprotocol/offline_protocol');

  static Future<T?> _invoke<T>(String method, [Object? arguments]) {
    return _channel.invokeMethod<T>(method, arguments);
  }

/// Native `acceptConnectionRequest` (see React Native `OfflineProtocolNativeModule.acceptConnectionRequest`).
static Future<dynamic> acceptConnectionRequest([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('acceptConnectionRequest', args);

/// Native `acceptServiceSubscription` (see React Native `OfflineProtocolNativeModule.acceptServiceSubscription`).
static Future<dynamic> acceptServiceSubscription([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('acceptServiceSubscription', args);

/// Native `bleGetDiagnostics` (see React Native `OfflineProtocolNativeModule.bleGetDiagnostics`).
static Future<dynamic> bleGetDiagnostics([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('bleGetDiagnostics', args);

/// Native `bleGetPeerCount` (see React Native `OfflineProtocolNativeModule.bleGetPeerCount`).
static Future<dynamic> bleGetPeerCount([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('bleGetPeerCount', args);

/// Native `blockUser` (see React Native `OfflineProtocolNativeModule.blockUser`).
static Future<dynamic> blockUser([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('blockUser', args);

/// Native `cancelConnectionRequest` (see React Native `OfflineProtocolNativeModule.cancelConnectionRequest`).
static Future<dynamic> cancelConnectionRequest([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('cancelConnectionRequest', args);

/// Native `cancelFileTransfer` (see React Native `OfflineProtocolNativeModule.cancelFileTransfer`).
static Future<dynamic> cancelFileTransfer([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('cancelFileTransfer', args);

/// Native `checkInternetPresence` (see React Native `OfflineProtocolNativeModule.checkInternetPresence`).
static Future<dynamic> checkInternetPresence([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('checkInternetPresence', args);

/// Native `clearServiceTopic` (see React Native `OfflineProtocolNativeModule.clearServiceTopic`).
static Future<dynamic> clearServiceTopic([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('clearServiceTopic', args);

/// Native `createInvite` (see React Native `OfflineProtocolNativeModule.createInvite`).
static Future<dynamic> createInvite([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('createInvite', args);

/// Native `dataAttachmentHash` (see React Native `OfflineProtocolNativeModule.dataAttachmentHash`).
static Future<dynamic> dataAttachmentHash([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataAttachmentHash', args);

/// Native `dataCounterIncrement` (see React Native `OfflineProtocolNativeModule.dataCounterIncrement`).
static Future<dynamic> dataCounterIncrement([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataCounterIncrement', args);

/// Native `dataCounterValue` (see React Native `OfflineProtocolNativeModule.dataCounterValue`).
static Future<dynamic> dataCounterValue([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataCounterValue', args);

/// Native `dataCreateDoc` (see React Native `OfflineProtocolNativeModule.dataCreateDoc`).
static Future<dynamic> dataCreateDoc([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataCreateDoc', args);

/// Native `dataDeclineAttachment` (see React Native `OfflineProtocolNativeModule.dataDeclineAttachment`).
static Future<dynamic> dataDeclineAttachment([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataDeclineAttachment', args);

/// Native `dataDeleteDoc` (see React Native `OfflineProtocolNativeModule.dataDeleteDoc`).
static Future<dynamic> dataDeleteDoc([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataDeleteDoc', args);

/// Native `dataDocJson` (see React Native `OfflineProtocolNativeModule.dataDocJson`).
static Future<dynamic> dataDocJson([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataDocJson', args);

/// Native `dataDocSize` (see React Native `OfflineProtocolNativeModule.dataDocSize`).
static Future<dynamic> dataDocSize([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataDocSize', args);

/// Native `dataExportRaw` (see React Native `OfflineProtocolNativeModule.dataExportRaw`).
static Future<dynamic> dataExportRaw([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataExportRaw', args);

/// Native `dataFetchAttachment` (see React Native `OfflineProtocolNativeModule.dataFetchAttachment`).
static Future<dynamic> dataFetchAttachment([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataFetchAttachment', args);

/// Native `dataFetchAttachmentFrom` (see React Native `OfflineProtocolNativeModule.dataFetchAttachmentFrom`).
static Future<dynamic> dataFetchAttachmentFrom([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataFetchAttachmentFrom', args);

/// Native `dataFlush` (see React Native `OfflineProtocolNativeModule.dataFlush`).
static Future<dynamic> dataFlush([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataFlush', args);

/// Native `dataFlushAll` (see React Native `OfflineProtocolNativeModule.dataFlushAll`).
static Future<dynamic> dataFlushAll([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataFlushAll', args);

/// Native `dataListDelete` (see React Native `OfflineProtocolNativeModule.dataListDelete`).
static Future<dynamic> dataListDelete([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataListDelete', args);

/// Native `dataListDocs` (see React Native `OfflineProtocolNativeModule.dataListDocs`).
static Future<dynamic> dataListDocs([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataListDocs', args);

/// Native `dataListLen` (see React Native `OfflineProtocolNativeModule.dataListLen`).
static Future<dynamic> dataListLen([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataListLen', args);

/// Native `dataListPush` (see React Native `OfflineProtocolNativeModule.dataListPush`).
static Future<dynamic> dataListPush([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataListPush', args);

/// Native `dataListSpaces` (see React Native `OfflineProtocolNativeModule.dataListSpaces`).
static Future<dynamic> dataListSpaces([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataListSpaces', args);

/// Native `dataMapDelete` (see React Native `OfflineProtocolNativeModule.dataMapDelete`).
static Future<dynamic> dataMapDelete([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataMapDelete', args);

/// Native `dataMapGetJson` (see React Native `OfflineProtocolNativeModule.dataMapGetJson`).
static Future<dynamic> dataMapGetJson([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataMapGetJson', args);

/// Native `dataMapSet` (see React Native `OfflineProtocolNativeModule.dataMapSet`).
static Future<dynamic> dataMapSet([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataMapSet', args);

/// Native `dataProvideAttachment` (see React Native `OfflineProtocolNativeModule.dataProvideAttachment`).
static Future<dynamic> dataProvideAttachment([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataProvideAttachment', args);

/// Native `dataRemoveDoc` (see React Native `OfflineProtocolNativeModule.dataRemoveDoc`).
static Future<dynamic> dataRemoveDoc([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataRemoveDoc', args);

/// Native `dataRemoveSpace` (see React Native `OfflineProtocolNativeModule.dataRemoveSpace`).
static Future<dynamic> dataRemoveSpace([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataRemoveSpace', args);

/// Native `dataSetInterest` (see React Native `OfflineProtocolNativeModule.dataSetInterest`).
static Future<dynamic> dataSetInterest([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataSetInterest', args);

/// Native `dataTextDelete` (see React Native `OfflineProtocolNativeModule.dataTextDelete`).
static Future<dynamic> dataTextDelete([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataTextDelete', args);

/// Native `dataTextInsert` (see React Native `OfflineProtocolNativeModule.dataTextInsert`).
static Future<dynamic> dataTextInsert([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataTextInsert', args);

/// Native `dataTextValue` (see React Native `OfflineProtocolNativeModule.dataTextValue`).
static Future<dynamic> dataTextValue([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataTextValue', args);

/// Native `dataWipeAll` (see React Native `OfflineProtocolNativeModule.dataWipeAll`).
static Future<dynamic> dataWipeAll([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('dataWipeAll', args);

/// Native `deriveAddress` (see React Native `OfflineProtocolNativeModule.deriveAddress`).
static Future<dynamic> deriveAddress([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('deriveAddress', args);

/// Native `deriveUserIdFromPublicKey` (see React Native `OfflineProtocolNativeModule.deriveUserIdFromPublicKey`).
static Future<dynamic> deriveUserIdFromPublicKey([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('deriveUserIdFromPublicKey', args);

/// Native `disableTelemetry` (see React Native `OfflineProtocolNativeModule.disableTelemetry`).
static Future<dynamic> disableTelemetry([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('disableTelemetry', args);

/// Native `disableTransport` (see React Native `OfflineProtocolNativeModule.disableTransport`).
static Future<dynamic> disableTransport([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('disableTransport', args);

/// Native `discoverServices` (see React Native `OfflineProtocolNativeModule.discoverServices`).
static Future<dynamic> discoverServices([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('discoverServices', args);

/// Native `emitTestEvent` (see React Native `OfflineProtocolNativeModule.emitTestEvent`).
static Future<dynamic> emitTestEvent([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('emitTestEvent', args);

/// Native `enableTelemetry` (see React Native `OfflineProtocolNativeModule.enableTelemetry`).
static Future<dynamic> enableTelemetry([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('enableTelemetry', args);

/// Native `endServiceSubscription` (see React Native `OfflineProtocolNativeModule.endServiceSubscription`).
static Future<dynamic> endServiceSubscription([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('endServiceSubscription', args);

/// Native `endTelemetrySession` (see React Native `OfflineProtocolNativeModule.endTelemetrySession`).
static Future<dynamic> endTelemetrySession([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('endTelemetrySession', args);

/// Native `eraseCustody` (see React Native `OfflineProtocolNativeModule.eraseCustody`).
static Future<dynamic> eraseCustody([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('eraseCustody', args);

/// Native `establishSecureSession` (see React Native `OfflineProtocolNativeModule.establishSecureSession`).
static Future<dynamic> establishSecureSession([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('establishSecureSession', args);

/// Native `finalizeFile` (see React Native `OfflineProtocolNativeModule.finalizeFile`).
static Future<dynamic> finalizeFile([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('finalizeFile', args);

/// Native `flushTelemetry` (see React Native `OfflineProtocolNativeModule.flushTelemetry`).
static Future<dynamic> flushTelemetry([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('flushTelemetry', args);

/// Native `forceTransport` (see React Native `OfflineProtocolNativeModule.forceTransport`).
static Future<dynamic> forceTransport([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('forceTransport', args);

/// Native `forwardMessage` (see React Native `OfflineProtocolNativeModule.forwardMessage`).
static Future<dynamic> forwardMessage([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('forwardMessage', args);

/// Native `getActiveTransports` (see React Native `OfflineProtocolNativeModule.getActiveTransports`).
static Future<dynamic> getActiveTransports([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getActiveTransports', args);

/// Native `getBatteryLevel` (see React Native `OfflineProtocolNativeModule.getBatteryLevel`).
static Future<dynamic> getBatteryLevel([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getBatteryLevel', args);

/// Native `getBlockedUsers` (see React Native `OfflineProtocolNativeModule.getBlockedUsers`).
static Future<dynamic> getBlockedUsers([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getBlockedUsers', args);

/// Native `getCustodyStats` (see React Native `OfflineProtocolNativeModule.getCustodyStats`).
static Future<dynamic> getCustodyStats([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getCustodyStats', args);

/// Native `getDedupStats` (see React Native `OfflineProtocolNativeModule.getDedupStats`).
static Future<dynamic> getDedupStats([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getDedupStats', args);

/// Native `getDeliverySuccessRate` (see React Native `OfflineProtocolNativeModule.getDeliverySuccessRate`).
static Future<dynamic> getDeliverySuccessRate([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getDeliverySuccessRate', args);

/// Native `getDorsConfig` (see React Native `OfflineProtocolNativeModule.getDorsConfig`).
static Future<dynamic> getDorsConfig([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getDorsConfig', args);

/// Native `getEstablishmentState` (see React Native `OfflineProtocolNativeModule.getEstablishmentState`).
static Future<dynamic> getEstablishmentState([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getEstablishmentState', args);

/// Native `getFileProgress` (see React Native `OfflineProtocolNativeModule.getFileProgress`).
static Future<dynamic> getFileProgress([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getFileProgress', args);

/// Native `getIdentityPublicKey` (see React Native `OfflineProtocolNativeModule.getIdentityPublicKey`).
static Future<dynamic> getIdentityPublicKey([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getIdentityPublicKey', args);

/// Native `getIsCharging` (see React Native `OfflineProtocolNativeModule.getIsCharging`).
static Future<dynamic> getIsCharging([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getIsCharging', args);

/// Native `getMedianHops` (see React Native `OfflineProtocolNativeModule.getMedianHops`).
static Future<dynamic> getMedianHops([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getMedianHops', args);

/// Native `getMedianLatency` (see React Native `OfflineProtocolNativeModule.getMedianLatency`).
static Future<dynamic> getMedianLatency([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getMedianLatency', args);

/// Native `getMeshRelayStats` (see React Native `OfflineProtocolNativeModule.getMeshRelayStats`).
static Future<dynamic> getMeshRelayStats([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getMeshRelayStats', args);

/// Native `getMeshRelayTunables` (see React Native `OfflineProtocolNativeModule.getMeshRelayTunables`).
static Future<dynamic> getMeshRelayTunables([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getMeshRelayTunables', args);

/// Native `getMessageStats` (see React Native `OfflineProtocolNativeModule.getMessageStats`).
static Future<dynamic> getMessageStats([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getMessageStats', args);

/// Native `getPendingAckCount` (see React Native `OfflineProtocolNativeModule.getPendingAckCount`).
static Future<dynamic> getPendingAckCount([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getPendingAckCount', args);

/// Native `getRelayConfig` (see React Native `OfflineProtocolNativeModule.getRelayConfig`).
static Future<dynamic> getRelayConfig([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getRelayConfig', args);

/// Native `getRelayPriority` (see React Native `OfflineProtocolNativeModule.getRelayPriority`).
static Future<dynamic> getRelayPriority([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getRelayPriority', args);

/// Native `getRetryQueueSize` (see React Native `OfflineProtocolNativeModule.getRetryQueueSize`).
static Future<dynamic> getRetryQueueSize([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getRetryQueueSize', args);

/// Native `getState` (see React Native `OfflineProtocolNativeModule.getState`).
static Future<dynamic> getState([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getState', args);

/// Native `getTopology` (see React Native `OfflineProtocolNativeModule.getTopology`).
static Future<dynamic> getTopology([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getTopology', args);

/// Native `getTransportMetrics` (see React Native `OfflineProtocolNativeModule.getTransportMetrics`).
static Future<dynamic> getTransportMetrics([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('getTransportMetrics', args);

/// Native `hasPendingKeyPackage` (see React Native `OfflineProtocolNativeModule.hasPendingKeyPackage`).
static Future<dynamic> hasPendingKeyPackage([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('hasPendingKeyPackage', args);

/// Native `internetConfirmSent` (see React Native `OfflineProtocolNativeModule.internetConfirmSent`).
static Future<dynamic> internetConfirmSent([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('internetConfirmSent', args);

/// Native `internetForceReconnect` (see React Native `OfflineProtocolNativeModule.internetForceReconnect`).
static Future<dynamic> internetForceReconnect([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('internetForceReconnect', args);

/// Native `internetGetNextMessage` (see React Native `OfflineProtocolNativeModule.internetGetNextMessage`).
static Future<dynamic> internetGetNextMessage([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('internetGetNextMessage', args);

/// Native `internetIsReady` (see React Native `OfflineProtocolNativeModule.internetIsReady`).
static Future<dynamic> internetIsReady([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('internetIsReady', args);

/// Native `internetIsSuperseded` (see React Native `OfflineProtocolNativeModule.internetIsSuperseded`).
static Future<dynamic> internetIsSuperseded([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('internetIsSuperseded', args);

/// Native `internetMessageReceived` (see React Native `OfflineProtocolNativeModule.internetMessageReceived`).
static Future<dynamic> internetMessageReceived([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('internetMessageReceived', args);

/// Native `internetSendFailed` (see React Native `OfflineProtocolNativeModule.internetSendFailed`).
static Future<dynamic> internetSendFailed([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('internetSendFailed', args);

/// Native `internetSendRawCommand` (see React Native `OfflineProtocolNativeModule.internetSendRawCommand`).
static Future<dynamic> internetSendRawCommand([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('internetSendRawCommand', args);

/// Native `internetStatusChanged` (see React Native `OfflineProtocolNativeModule.internetStatusChanged`).
static Future<dynamic> internetStatusChanged([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('internetStatusChanged', args);

/// Native `isBluetoothEnabled` (see React Native `OfflineProtocolNativeModule.isBluetoothEnabled`).
static Future<dynamic> isBluetoothEnabled([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('isBluetoothEnabled', args);

/// Native `isMlsInitialized` (see React Native `OfflineProtocolNativeModule.isMlsInitialized`).
static Future<dynamic> isMlsInitialized([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('isMlsInitialized', args);

/// Native `isRelay` (see React Native `OfflineProtocolNativeModule.isRelay`).
static Future<dynamic> isRelay([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('isRelay', args);

/// Native `isUserBlocked` (see React Native `OfflineProtocolNativeModule.isUserBlocked`).
static Future<dynamic> isUserBlocked([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('isUserBlocked', args);

/// Native `meshCreateGroup` (see React Native `OfflineProtocolNativeModule.meshCreateGroup`).
static Future<dynamic> meshCreateGroup([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshCreateGroup', args);

/// Native `meshForwardMessageToGroup` (see React Native `OfflineProtocolNativeModule.meshForwardMessageToGroup`).
static Future<dynamic> meshForwardMessageToGroup([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshForwardMessageToGroup', args);

/// Native `meshGetGroupInfo` (see React Native `OfflineProtocolNativeModule.meshGetGroupInfo`).
static Future<dynamic> meshGetGroupInfo([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshGetGroupInfo', args);

/// Native `meshGetGroupRoles` (see React Native `OfflineProtocolNativeModule.meshGetGroupRoles`).
static Future<dynamic> meshGetGroupRoles([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshGetGroupRoles', args);

/// Native `meshGetMemberRole` (see React Native `OfflineProtocolNativeModule.meshGetMemberRole`).
static Future<dynamic> meshGetMemberRole([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshGetMemberRole', args);

/// Native `meshGroupRelaySyncState` (see React Native `OfflineProtocolNativeModule.meshGroupRelaySyncState`).
static Future<dynamic> meshGroupRelaySyncState([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshGroupRelaySyncState', args);

/// Native `meshGroupRichReadiness` (see React Native `OfflineProtocolNativeModule.meshGroupRichReadiness`).
static Future<dynamic> meshGroupRichReadiness([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshGroupRichReadiness', args);

/// Native `meshInviteToGroup` (see React Native `OfflineProtocolNativeModule.meshInviteToGroup`).
static Future<dynamic> meshInviteToGroup([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshInviteToGroup', args);

/// Native `meshLeaveGroup` (see React Native `OfflineProtocolNativeModule.meshLeaveGroup`).
static Future<dynamic> meshLeaveGroup([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshLeaveGroup', args);

/// Native `meshListGroups` (see React Native `OfflineProtocolNativeModule.meshListGroups`).
static Future<dynamic> meshListGroups([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshListGroups', args);

/// Native `meshRemoveFromGroup` (see React Native `OfflineProtocolNativeModule.meshRemoveFromGroup`).
static Future<dynamic> meshRemoveFromGroup([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshRemoveFromGroup', args);

/// Native `meshRenameGroup` (see React Native `OfflineProtocolNativeModule.meshRenameGroup`).
static Future<dynamic> meshRenameGroup([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshRenameGroup', args);

/// Native `meshRequestGroupRelayRegistration` (see React Native `OfflineProtocolNativeModule.meshRequestGroupRelayRegistration`).
static Future<dynamic> meshRequestGroupRelayRegistration([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshRequestGroupRelayRegistration', args);

/// Native `meshSendGroupMessage` (see React Native `OfflineProtocolNativeModule.meshSendGroupMessage`).
static Future<dynamic> meshSendGroupMessage([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshSendGroupMessage', args);

/// Native `meshSetMemberRole` (see React Native `OfflineProtocolNativeModule.meshSetMemberRole`).
static Future<dynamic> meshSetMemberRole([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('meshSetMemberRole', args);

/// Native `mlsCreateSession` (see React Native `OfflineProtocolNativeModule.mlsCreateSession`).
static Future<dynamic> mlsCreateSession([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsCreateSession', args);

/// Native `mlsDecrypt` (see React Native `OfflineProtocolNativeModule.mlsDecrypt`).
static Future<dynamic> mlsDecrypt([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsDecrypt', args);

/// Native `mlsDecryptFromUser` (see React Native `OfflineProtocolNativeModule.mlsDecryptFromUser`).
static Future<dynamic> mlsDecryptFromUser([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsDecryptFromUser', args);

/// Native `mlsDeleteSession` (see React Native `OfflineProtocolNativeModule.mlsDeleteSession`).
static Future<dynamic> mlsDeleteSession([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsDeleteSession', args);

/// Native `mlsEncryptForUser` (see React Native `OfflineProtocolNativeModule.mlsEncryptForUser`).
static Future<dynamic> mlsEncryptForUser([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsEncryptForUser', args);

/// Native `mlsGenerateKeyPackage` (see React Native `OfflineProtocolNativeModule.mlsGenerateKeyPackage`).
static Future<dynamic> mlsGenerateKeyPackage([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsGenerateKeyPackage', args);

/// Native `mlsGetOrCreateKeyPackage` (see React Native `OfflineProtocolNativeModule.mlsGetOrCreateKeyPackage`).
static Future<dynamic> mlsGetOrCreateKeyPackage([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsGetOrCreateKeyPackage', args);

/// Native `mlsGetPendingKeyPackages` (see React Native `OfflineProtocolNativeModule.mlsGetPendingKeyPackages`).
static Future<dynamic> mlsGetPendingKeyPackages([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsGetPendingKeyPackages', args);

/// Native `mlsHasSession` (see React Native `OfflineProtocolNativeModule.mlsHasSession`).
static Future<dynamic> mlsHasSession([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsHasSession', args);

/// Native `mlsImportKeyPackage` (see React Native `OfflineProtocolNativeModule.mlsImportKeyPackage`).
static Future<dynamic> mlsImportKeyPackage([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsImportKeyPackage', args);

/// Native `mlsJoinSession` (see React Native `OfflineProtocolNativeModule.mlsJoinSession`).
static Future<dynamic> mlsJoinSession([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsJoinSession', args);

/// Native `mlsListSessions` (see React Native `OfflineProtocolNativeModule.mlsListSessions`).
static Future<dynamic> mlsListSessions([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsListSessions', args);

/// Native `mlsMarkKeyPackageSynced` (see React Native `OfflineProtocolNativeModule.mlsMarkKeyPackageSynced`).
static Future<dynamic> mlsMarkKeyPackageSynced([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsMarkKeyPackageSynced', args);

/// Native `mlsProcessWelcome` (see React Native `OfflineProtocolNativeModule.mlsProcessWelcome`).
static Future<dynamic> mlsProcessWelcome([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('mlsProcessWelcome', args);

/// Native `parseInvite` (see React Native `OfflineProtocolNativeModule.parseInvite`).
static Future<dynamic> parseInvite([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('parseInvite', args);

/// Native `pause` (see React Native `OfflineProtocolNativeModule.pause`).
static Future<dynamic> pause([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('pause', args);

/// Native `processFileChunk` (see React Native `OfflineProtocolNativeModule.processFileChunk`).
static Future<dynamic> processFileChunk([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('processFileChunk', args);

/// Native `publishServiceUpdate` (see React Native `OfflineProtocolNativeModule.publishServiceUpdate`).
static Future<dynamic> publishServiceUpdate([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('publishServiceUpdate', args);

/// Native `receiveMessage` (see React Native `OfflineProtocolNativeModule.receiveMessage`).
static Future<dynamic> receiveMessage([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('receiveMessage', args);

/// Native `registerService` (see React Native `OfflineProtocolNativeModule.registerService`).
static Future<dynamic> registerService([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('registerService', args);

/// Native `rejectConnectionRequest` (see React Native `OfflineProtocolNativeModule.rejectConnectionRequest`).
static Future<dynamic> rejectConnectionRequest([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('rejectConnectionRequest', args);

/// Native `rejectServiceSubscription` (see React Native `OfflineProtocolNativeModule.rejectServiceSubscription`).
static Future<dynamic> rejectServiceSubscription([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('rejectServiceSubscription', args);

/// Native `rekeySession` (see React Native `OfflineProtocolNativeModule.rekeySession`).
static Future<dynamic> rekeySession([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('rekeySession', args);

/// Native `releaseTransportLock` (see React Native `OfflineProtocolNativeModule.releaseTransportLock`).
static Future<dynamic> releaseTransportLock([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('releaseTransportLock', args);

/// Native `requestEnableBluetooth` (see React Native `OfflineProtocolNativeModule.requestEnableBluetooth`).
static Future<dynamic> requestEnableBluetooth([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('requestEnableBluetooth', args);

/// Native `resolveUsername` (see React Native `OfflineProtocolNativeModule.resolveUsername`).
static Future<dynamic> resolveUsername([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('resolveUsername', args);

/// Native `respondToServiceRequest` (see React Native `OfflineProtocolNativeModule.respondToServiceRequest`).
static Future<dynamic> respondToServiceRequest([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('respondToServiceRequest', args);

/// Native `resume` (see React Native `OfflineProtocolNativeModule.resume`).
static Future<dynamic> resume([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('resume', args);

/// Native `resyncServiceSubscription` (see React Native `OfflineProtocolNativeModule.resyncServiceSubscription`).
static Future<dynamic> resyncServiceSubscription([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('resyncServiceSubscription', args);

/// Native `sendConnectionRequest` (see React Native `OfflineProtocolNativeModule.sendConnectionRequest`).
static Future<dynamic> sendConnectionRequest([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('sendConnectionRequest', args);

/// Native `sendFileFromPath` (see React Native `OfflineProtocolNativeModule.sendFileFromPath`).
static Future<dynamic> sendFileFromPath([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('sendFileFromPath', args);

/// Native `sendMedia` (see React Native `OfflineProtocolNativeModule.sendMedia`).
static Future<dynamic> sendMedia([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('sendMedia', args);

/// Native `sendMediaRich` (see React Native `OfflineProtocolNativeModule.sendMediaRich`).
static Future<dynamic> sendMediaRich([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('sendMediaRich', args);

/// Native `sendMessageRich` (see React Native `OfflineProtocolNativeModule.sendMessageRich`).
static Future<dynamic> sendMessageRich([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('sendMessageRich', args);

/// Native `sendPresenceUpdate` (see React Native `OfflineProtocolNativeModule.sendPresenceUpdate`).
static Future<dynamic> sendPresenceUpdate([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('sendPresenceUpdate', args);

/// Native `sendReadReceipt` (see React Native `OfflineProtocolNativeModule.sendReadReceipt`).
static Future<dynamic> sendReadReceipt([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('sendReadReceipt', args);

/// Native `sendServiceRequest` (see React Native `OfflineProtocolNativeModule.sendServiceRequest`).
static Future<dynamic> sendServiceRequest([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('sendServiceRequest', args);

/// Native `sendTypingIndicator` (see React Native `OfflineProtocolNativeModule.sendTypingIndicator`).
static Future<dynamic> sendTypingIndicator([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('sendTypingIndicator', args);

/// Native `serviceSubscribers` (see React Native `OfflineProtocolNativeModule.serviceSubscribers`).
static Future<dynamic> serviceSubscribers([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('serviceSubscribers', args);

/// Native `serviceSubscriptions` (see React Native `OfflineProtocolNativeModule.serviceSubscriptions`).
static Future<dynamic> serviceSubscriptions([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('serviceSubscriptions', args);

/// Native `setBatteryLevel` (see React Native `OfflineProtocolNativeModule.setBatteryLevel`).
static Future<dynamic> setBatteryLevel([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('setBatteryLevel', args);

/// Native `setBatteryState` (see React Native `OfflineProtocolNativeModule.setBatteryState`).
static Future<dynamic> setBatteryState([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('setBatteryState', args);

/// Native `setRelayPriority` (see React Native `OfflineProtocolNativeModule.setRelayPriority`).
static Future<dynamic> setRelayPriority([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('setRelayPriority', args);

/// Native `setServiceSubscriptionPolicy` (see React Native `OfflineProtocolNativeModule.setServiceSubscriptionPolicy`).
static Future<dynamic> setServiceSubscriptionPolicy([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('setServiceSubscriptionPolicy', args);

/// Native `setServiceTopicCheckpoint` (see React Native `OfflineProtocolNativeModule.setServiceTopicCheckpoint`).
static Future<dynamic> setServiceTopicCheckpoint([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('setServiceTopicCheckpoint', args);

/// Native `setTelemetryEnabled` (see React Native `OfflineProtocolNativeModule.setTelemetryEnabled`).
static Future<dynamic> setTelemetryEnabled([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('setTelemetryEnabled', args);

/// Native `shouldEscalateToWifi` (see React Native `OfflineProtocolNativeModule.shouldEscalateToWifi`).
static Future<dynamic> shouldEscalateToWifi([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('shouldEscalateToWifi', args);

/// Native `signData` (see React Native `OfflineProtocolNativeModule.signData`).
static Future<dynamic> signData([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('signData', args);

/// Native `signDataBase64` (see React Native `OfflineProtocolNativeModule.signDataBase64`).
static Future<dynamic> signDataBase64([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('signDataBase64', args);

/// Native `subscribeService` (see React Native `OfflineProtocolNativeModule.subscribeService`).
static Future<dynamic> subscribeService([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('subscribeService', args);

/// Native `telemetryInstallId` (see React Native `OfflineProtocolNativeModule.telemetryInstallId`).
static Future<dynamic> telemetryInstallId([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('telemetryInstallId', args);

/// Native `telemetryStats` (see React Native `OfflineProtocolNativeModule.telemetryStats`).
static Future<dynamic> telemetryStats([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('telemetryStats', args);

/// Native `unblockUser` (see React Native `OfflineProtocolNativeModule.unblockUser`).
static Future<dynamic> unblockUser([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('unblockUser', args);

/// Native `unregisterService` (see React Native `OfflineProtocolNativeModule.unregisterService`).
static Future<dynamic> unregisterService([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('unregisterService', args);

/// Native `unsubscribeService` (see React Native `OfflineProtocolNativeModule.unsubscribeService`).
static Future<dynamic> unsubscribeService([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('unsubscribeService', args);

/// Native `updateAckConfig` (see React Native `OfflineProtocolNativeModule.updateAckConfig`).
static Future<dynamic> updateAckConfig([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('updateAckConfig', args);

/// Native `updateDedupConfig` (see React Native `OfflineProtocolNativeModule.updateDedupConfig`).
static Future<dynamic> updateDedupConfig([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('updateDedupConfig', args);

/// Native `updateDorsConfig` (see React Native `OfflineProtocolNativeModule.updateDorsConfig`).
static Future<dynamic> updateDorsConfig([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('updateDorsConfig', args);

/// Native `updateRelayConfig` (see React Native `OfflineProtocolNativeModule.updateRelayConfig`).
static Future<dynamic> updateRelayConfig([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('updateRelayConfig', args);

/// Native `updateRetryConfig` (see React Native `OfflineProtocolNativeModule.updateRetryConfig`).
static Future<dynamic> updateRetryConfig([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('updateRetryConfig', args);

/// Native `verifySignature` (see React Native `OfflineProtocolNativeModule.verifySignature`).
static Future<dynamic> verifySignature([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('verifySignature', args);

/// Native `verifySignatureBase64` (see React Native `OfflineProtocolNativeModule.verifySignatureBase64`).
static Future<dynamic> verifySignatureBase64([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('verifySignatureBase64', args);

/// Native `wifiDirectGetNextMessage` (see React Native `OfflineProtocolNativeModule.wifiDirectGetNextMessage`).
static Future<dynamic> wifiDirectGetNextMessage([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('wifiDirectGetNextMessage', args);

/// Native `wifiDirectMessageReceived` (see React Native `OfflineProtocolNativeModule.wifiDirectMessageReceived`).
static Future<dynamic> wifiDirectMessageReceived([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('wifiDirectMessageReceived', args);

/// Native `wifiDirectPeerConnected` (see React Native `OfflineProtocolNativeModule.wifiDirectPeerConnected`).
static Future<dynamic> wifiDirectPeerConnected([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('wifiDirectPeerConnected', args);

/// Native `wifiDirectPeerDisconnected` (see React Native `OfflineProtocolNativeModule.wifiDirectPeerDisconnected`).
static Future<dynamic> wifiDirectPeerDisconnected([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('wifiDirectPeerDisconnected', args);

/// Native `wifiDirectStatusChanged` (see React Native `OfflineProtocolNativeModule.wifiDirectStatusChanged`).
static Future<dynamic> wifiDirectStatusChanged([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('wifiDirectStatusChanged', args);

/// Native `wipePersistedState` (see React Native `OfflineProtocolNativeModule.wipePersistedState`).
static Future<dynamic> wipePersistedState([Map<String, dynamic>? args]) =>
    _invoke<dynamic>('wipePersistedState', args);

// --- Typed fast paths (avoid hand-built maps on hot calls) ---

/// Disables telemetry and **awaits** the engine-thread flush (may take a few
/// seconds). Use on lifecycle shutdown; do not call other engine methods until
/// this future completes if you need a clean boundary (F8).
static Future<void> disableTelemetryAndWait() => _invoke<void>('disableTelemetry');

/// Ends the telemetry session (same blocking semantics as [disableTelemetryAndWait]).
static Future<void> endTelemetrySessionAndWait() =>
    _invoke<void>('endTelemetrySession');

/// Flushes buffered telemetry on the engine thread (same blocking semantics).
static Future<void> flushTelemetryAndWait() => _invoke<void>('flushTelemetry');

/// Plain send with explicit fields (same method-channel shape as `OfflineProtocol.sendMessage`).
static Future<String> sendMessageTyped({
  required String recipient,
  required String content,
  int priority = 1,
  String? replyToMsg,
}) async {
  final id = await _invoke<dynamic>('sendMessage', {
    'recipient': recipient,
    'content': content,
    'priority': priority,
    if (replyToMsg != null) 'replyToMsg': replyToMsg,
  });
  final messageId = id?.toString() ?? '';
  if (messageId.isEmpty) {
    throw StateError('sendMessage returned no message id');
  }
  return messageId;
}

/// Sends a rich message with explicit fields instead of a nested options map.
static Future<dynamic> sendMessageRichTyped({
  required String recipient,
  required String content,
  required int priority,
  String? replyToMsg,
  Map<String, dynamic>? options,
}) =>
    _invoke<dynamic>('sendMessageRich', {
      'recipient': recipient,
      'content': content,
      'priority': priority,
      if (replyToMsg != null) 'replyToMsg': replyToMsg,
      if (options != null) 'options': options,
    });

/// Resolves protocol state without building an argument map.
static Future<String> getStateTyped() async {
  final value = await _invoke<dynamic>('getState');
  return value?.toString() ?? 'Stopped';
}

/// Whether the Internet transport reports ready (false when not initialized).
static Future<bool> internetIsReadyTyped() async {
  final value = await _invoke<dynamic>('internetIsReady');
  return value == true;
}

/// Whether the relay session was superseded elsewhere (false when not initialized).
static Future<bool> internetIsSupersededTyped() async {
  final value = await _invoke<dynamic>('internetIsSuperseded');
  return value == true;
}

/// BLE radio state query used before showing a manual enable prompt on iOS.
static Future<bool> isBluetoothEnabledTyped() async {
  final value = await _invoke<dynamic>('isBluetoothEnabled');
  return value == true;
}

/// Whether MLS material is initialized on the running protocol instance.
static Future<bool> isMlsInitializedTyped() async {
  final value = await _invoke<dynamic>('isMlsInitialized');
  return value == true;
}

/// Whether this device is acting as a mesh relay (false when not initialized).
static Future<bool> isRelayTyped() async {
  final value = await _invoke<dynamic>('isRelay');
  return value == true;
}

/// Whether a key package for [peerId] is queued locally.
static Future<bool> hasPendingKeyPackageTyped(String peerId) async {
  final value = await _invoke<dynamic>('hasPendingKeyPackage', {'peerId': peerId});
  return value == true;
}

/// Device battery level when the platform exposes it; null otherwise.
static Future<int?> getBatteryLevelTyped() async {
  final value = await _invoke<dynamic>('getBatteryLevel');
  if (value == null) return null;
  if (value is int) return value;
  if (value is num) return value.toInt();
  return int.tryParse(value.toString());
}

/// Local device address (`off1…`) without a map argument.
static Future<String> localAddressTyped() async {
  final value = await _invoke<dynamic>('localAddress');
  return value?.toString() ?? '';
}

/// Pending ACK count for reliability UI (0 when the protocol is not running).
static Future<int> getPendingAckCountTyped() async {
  final value = await _invoke<dynamic>('getPendingAckCount');
  if (value is int) return value;
  if (value is num) return value.toInt();
  return int.tryParse(value?.toString() ?? '') ?? 0;
}

/// Outbound retry queue depth (0 when the protocol is not running).
static Future<int> getRetryQueueSizeTyped() async {
  final value = await _invoke<dynamic>('getRetryQueueSize');
  if (value is int) return value;
  if (value is num) return value.toInt();
  return int.tryParse(value?.toString() ?? '') ?? 0;
}

/// Connected BLE peer count (0 when BLE is not active).
static Future<int> bleGetPeerCountTyped() async {
  final value = await _invoke<dynamic>('bleGetPeerCount');
  if (value is int) return value;
  if (value is num) return value.toInt();
  return int.tryParse(value?.toString() ?? '') ?? 0;
}

/// Active transport names from the core (empty when not initialized).
static Future<List<String>> getActiveTransportsTyped() async {
  final value = await _invoke<dynamic>('getActiveTransports');
  if (value is List) {
    return value.map((e) => e.toString()).toList();
  }
  return const [];
}
}
