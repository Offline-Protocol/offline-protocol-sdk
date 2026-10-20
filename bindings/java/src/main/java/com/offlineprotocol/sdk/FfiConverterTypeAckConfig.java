package com.offlineprotocol.sdk;


public enum FfiConverterTypeAckConfig implements FfiConverterRustBuffer<AckConfig> {
  INSTANCE;

  @Override
  public AckConfig read(java.nio.ByteBuffer buf) {
    return new AckConfig(
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(AckConfig value) {
      return (
            FfiConverterLong.INSTANCE.allocationSize(value.defaultTimeoutMs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.maxPendingAcks())
      );
  }

  @Override
  public void write(AckConfig value, java.nio.ByteBuffer buf) {
      FfiConverterLong.INSTANCE.write(value.defaultTimeoutMs(), buf);
      FfiConverterLong.INSTANCE.write(value.maxPendingAcks(), buf);
  }
}
