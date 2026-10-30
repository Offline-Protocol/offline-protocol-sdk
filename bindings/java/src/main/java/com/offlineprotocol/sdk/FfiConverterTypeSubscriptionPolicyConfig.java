package com.offlineprotocol.sdk;


public enum FfiConverterTypeSubscriptionPolicyConfig implements FfiConverterRustBuffer<SubscriptionPolicyConfig> {
  INSTANCE;

  @Override
  public SubscriptionPolicyConfig read(java.nio.ByteBuffer buf) {
    return new SubscriptionPolicyConfig(
      FfiConverterTypeSubscriptionPolicyKind.INSTANCE.read(buf),
      FfiConverterSequenceString.INSTANCE.read(buf),
      FfiConverterSequenceString.INSTANCE.read(buf),
      FfiConverterTypeSubscriptionRulesFallback.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(SubscriptionPolicyConfig value) {
      return (
            FfiConverterTypeSubscriptionPolicyKind.INSTANCE.allocationSize(value.kind()) +
            FfiConverterSequenceString.INSTANCE.allocationSize(value.allowPeers()) +
            FfiConverterSequenceString.INSTANCE.allocationSize(value.allowGroups()) +
            FfiConverterTypeSubscriptionRulesFallback.INSTANCE.allocationSize(value.rulesFallback())
      );
  }

  @Override
  public void write(SubscriptionPolicyConfig value, java.nio.ByteBuffer buf) {
      FfiConverterTypeSubscriptionPolicyKind.INSTANCE.write(value.kind(), buf);
      FfiConverterSequenceString.INSTANCE.write(value.allowPeers(), buf);
      FfiConverterSequenceString.INSTANCE.write(value.allowGroups(), buf);
      FfiConverterTypeSubscriptionRulesFallback.INSTANCE.write(value.rulesFallback(), buf);
  }
}
