package com.offlineprotocol.sdk;


public enum FfiConverterTypeProtocolConfig implements FfiConverterRustBuffer<ProtocolConfig> {
  INSTANCE;

  @Override
  public ProtocolConfig read(java.nio.ByteBuffer buf) {
    return new ProtocolConfig(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterByte.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterTypeOverflowPolicy.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterOptionalTypeMeshRelayConfig.INSTANCE.read(buf),
      FfiConverterOptionalTypeCustodyConfig.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(ProtocolConfig value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.appId()) +
            FfiConverterString.INSTANCE.allocationSize(value.profile()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.bleEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.wifiDirectEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.internetEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.reticulumEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.nostrEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.preferOnline()) +
            FfiConverterByte.INSTANCE.allocationSize(value.initialTtl()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.encryptionEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.autoKeyExchange()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.storePending()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.requireEncryption()) +
            FfiConverterLong.INSTANCE.allocationSize(value.maxPendingPerPeer()) +
            FfiConverterLong.INSTANCE.allocationSize(value.maxPendingGlobal()) +
            FfiConverterLong.INSTANCE.allocationSize(value.pendingTtlMs()) +
            FfiConverterTypeOverflowPolicy.INSTANCE.allocationSize(value.overflowPolicy()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.edgeDrivenUnreachableDm()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.maxGroupMembers()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.groupRelayEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.groupRelayBroadcastEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.groupEnforceAdminCommits()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.requireTransportIdentity()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.binaryWireEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.nostrSealingEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.nostrColdContactEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.nostrUsernameDiscoveryEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.compactEnvelopeEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.richPayloadEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.cryptoRecoveryEnabled()) +
            FfiConverterOptionalTypeMeshRelayConfig.INSTANCE.allocationSize(value.meshRelay()) +
            FfiConverterOptionalTypeCustodyConfig.INSTANCE.allocationSize(value.custody()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.dataEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.controlFreshnessEnforced()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.encryptServiceMessages())
      );
  }

  @Override
  public void write(ProtocolConfig value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.appId(), buf);
      FfiConverterString.INSTANCE.write(value.profile(), buf);
      FfiConverterBoolean.INSTANCE.write(value.bleEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.wifiDirectEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.internetEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.reticulumEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.nostrEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.preferOnline(), buf);
      FfiConverterByte.INSTANCE.write(value.initialTtl(), buf);
      FfiConverterBoolean.INSTANCE.write(value.encryptionEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.autoKeyExchange(), buf);
      FfiConverterBoolean.INSTANCE.write(value.storePending(), buf);
      FfiConverterBoolean.INSTANCE.write(value.requireEncryption(), buf);
      FfiConverterLong.INSTANCE.write(value.maxPendingPerPeer(), buf);
      FfiConverterLong.INSTANCE.write(value.maxPendingGlobal(), buf);
      FfiConverterLong.INSTANCE.write(value.pendingTtlMs(), buf);
      FfiConverterTypeOverflowPolicy.INSTANCE.write(value.overflowPolicy(), buf);
      FfiConverterBoolean.INSTANCE.write(value.edgeDrivenUnreachableDm(), buf);
      FfiConverterInteger.INSTANCE.write(value.maxGroupMembers(), buf);
      FfiConverterBoolean.INSTANCE.write(value.groupRelayEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.groupRelayBroadcastEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.groupEnforceAdminCommits(), buf);
      FfiConverterBoolean.INSTANCE.write(value.requireTransportIdentity(), buf);
      FfiConverterBoolean.INSTANCE.write(value.binaryWireEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.nostrSealingEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.nostrColdContactEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.nostrUsernameDiscoveryEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.compactEnvelopeEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.richPayloadEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.cryptoRecoveryEnabled(), buf);
      FfiConverterOptionalTypeMeshRelayConfig.INSTANCE.write(value.meshRelay(), buf);
      FfiConverterOptionalTypeCustodyConfig.INSTANCE.write(value.custody(), buf);
      FfiConverterBoolean.INSTANCE.write(value.dataEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.controlFreshnessEnforced(), buf);
      FfiConverterBoolean.INSTANCE.write(value.encryptServiceMessages(), buf);
  }
}
