package com.offlineprotocol.sdk;


public enum FfiConverterTypePendingQueueConfig implements FfiConverterRustBuffer<PendingQueueConfig> {
  INSTANCE;

  @Override
  public PendingQueueConfig read(java.nio.ByteBuffer buf) {
    return new PendingQueueConfig(
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterTypeOverflowPolicy.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(PendingQueueConfig value) {
      return (
            FfiConverterLong.INSTANCE.allocationSize(value.maxPendingPerPeer()) +
            FfiConverterLong.INSTANCE.allocationSize(value.maxPendingGlobal()) +
            FfiConverterLong.INSTANCE.allocationSize(value.pendingTtlMs()) +
            FfiConverterTypeOverflowPolicy.INSTANCE.allocationSize(value.overflowPolicy())
      );
  }

  @Override
  public void write(PendingQueueConfig value, java.nio.ByteBuffer buf) {
      FfiConverterLong.INSTANCE.write(value.maxPendingPerPeer(), buf);
      FfiConverterLong.INSTANCE.write(value.maxPendingGlobal(), buf);
      FfiConverterLong.INSTANCE.write(value.pendingTtlMs(), buf);
      FfiConverterTypeOverflowPolicy.INSTANCE.write(value.overflowPolicy(), buf);
  }
}
