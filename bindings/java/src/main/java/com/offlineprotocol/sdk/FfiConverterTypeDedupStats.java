package com.offlineprotocol.sdk;


public enum FfiConverterTypeDedupStats implements FfiConverterRustBuffer<DedupStats> {
  INSTANCE;

  @Override
  public DedupStats read(java.nio.ByteBuffer buf) {
    return new DedupStats(
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterByte.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(DedupStats value) {
      return (
            FfiConverterLong.INSTANCE.allocationSize(value.totalTracked()) +
            FfiConverterLong.INSTANCE.allocationSize(value.recentTracked()) +
            FfiConverterByte.INSTANCE.allocationSize(value.capacityUsedPercent()) +
            FfiConverterString.INSTANCE.allocationSize(value.mode())
      );
  }

  @Override
  public void write(DedupStats value, java.nio.ByteBuffer buf) {
      FfiConverterLong.INSTANCE.write(value.totalTracked(), buf);
      FfiConverterLong.INSTANCE.write(value.recentTracked(), buf);
      FfiConverterByte.INSTANCE.write(value.capacityUsedPercent(), buf);
      FfiConverterString.INSTANCE.write(value.mode(), buf);
  }
}
