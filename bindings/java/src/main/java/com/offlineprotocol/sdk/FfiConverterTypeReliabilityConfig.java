package com.offlineprotocol.sdk;


public enum FfiConverterTypeReliabilityConfig implements FfiConverterRustBuffer<ReliabilityConfig> {
  INSTANCE;

  @Override
  public ReliabilityConfig read(java.nio.ByteBuffer buf) {
    return new ReliabilityConfig(
      FfiConverterTypeAckConfig.INSTANCE.read(buf),
      FfiConverterTypeRetryConfig.INSTANCE.read(buf),
      FfiConverterTypeDedupConfig.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(ReliabilityConfig value) {
      return (
            FfiConverterTypeAckConfig.INSTANCE.allocationSize(value.ack()) +
            FfiConverterTypeRetryConfig.INSTANCE.allocationSize(value.retry()) +
            FfiConverterTypeDedupConfig.INSTANCE.allocationSize(value.dedup())
      );
  }

  @Override
  public void write(ReliabilityConfig value, java.nio.ByteBuffer buf) {
      FfiConverterTypeAckConfig.INSTANCE.write(value.ack(), buf);
      FfiConverterTypeRetryConfig.INSTANCE.write(value.retry(), buf);
      FfiConverterTypeDedupConfig.INSTANCE.write(value.dedup(), buf);
  }
}
