package com.offlineprotocol.sdk;


public enum FfiConverterTypeDedupConfig implements FfiConverterRustBuffer<DedupConfig> {
  INSTANCE;

  @Override
  public DedupConfig read(java.nio.ByteBuffer buf) {
    return new DedupConfig(
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(DedupConfig value) {
      return (
            FfiConverterLong.INSTANCE.allocationSize(value.maxTrackedMessages()) +
            FfiConverterLong.INSTANCE.allocationSize(value.retentionTimeSecs())
      );
  }

  @Override
  public void write(DedupConfig value, java.nio.ByteBuffer buf) {
      FfiConverterLong.INSTANCE.write(value.maxTrackedMessages(), buf);
      FfiConverterLong.INSTANCE.write(value.retentionTimeSecs(), buf);
  }
}
