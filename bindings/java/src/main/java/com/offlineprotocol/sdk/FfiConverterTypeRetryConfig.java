package com.offlineprotocol.sdk;


public enum FfiConverterTypeRetryConfig implements FfiConverterRustBuffer<RetryConfig> {
  INSTANCE;

  @Override
  public RetryConfig read(java.nio.ByteBuffer buf) {
    return new RetryConfig(
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(RetryConfig value) {
      return (
            FfiConverterInteger.INSTANCE.allocationSize(value.maxRetries()) +
            FfiConverterLong.INSTANCE.allocationSize(value.initialDelayMs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.maxDelayMs()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.backoffMultiplier()) +
            FfiConverterLong.INSTANCE.allocationSize(value.outboxMaxLifetimeMs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.pendingMessageMaxLifetimeMs()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.edgeDrivenUnreachableDm())
      );
  }

  @Override
  public void write(RetryConfig value, java.nio.ByteBuffer buf) {
      FfiConverterInteger.INSTANCE.write(value.maxRetries(), buf);
      FfiConverterLong.INSTANCE.write(value.initialDelayMs(), buf);
      FfiConverterLong.INSTANCE.write(value.maxDelayMs(), buf);
      FfiConverterFloat.INSTANCE.write(value.backoffMultiplier(), buf);
      FfiConverterLong.INSTANCE.write(value.outboxMaxLifetimeMs(), buf);
      FfiConverterLong.INSTANCE.write(value.pendingMessageMaxLifetimeMs(), buf);
      FfiConverterBoolean.INSTANCE.write(value.edgeDrivenUnreachableDm(), buf);
  }
}
