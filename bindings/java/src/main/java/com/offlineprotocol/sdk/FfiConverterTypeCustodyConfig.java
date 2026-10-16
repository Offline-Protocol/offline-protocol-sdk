package com.offlineprotocol.sdk;


public enum FfiConverterTypeCustodyConfig implements FfiConverterRustBuffer<CustodyConfig> {
  INSTANCE;

  @Override
  public CustodyConfig read(java.nio.ByteBuffer buf) {
    return new CustodyConfig(
      FfiConverterOptionalBoolean.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalTypeOverflowPolicy.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(CustodyConfig value) {
      return (
            FfiConverterOptionalBoolean.INSTANCE.allocationSize(value.enabled()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.holdMs()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.maxEntriesPerDepositor()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.maxBytesPerDepositor()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.maxEntries()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.maxBytes()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.strangerMaxEntries()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.strangerMaxBytes()) +
            FfiConverterOptionalTypeOverflowPolicy.INSTANCE.allocationSize(value.overflowPolicy())
      );
  }

  @Override
  public void write(CustodyConfig value, java.nio.ByteBuffer buf) {
      FfiConverterOptionalBoolean.INSTANCE.write(value.enabled(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.holdMs(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.maxEntriesPerDepositor(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.maxBytesPerDepositor(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.maxEntries(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.maxBytes(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.strangerMaxEntries(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.strangerMaxBytes(), buf);
      FfiConverterOptionalTypeOverflowPolicy.INSTANCE.write(value.overflowPolicy(), buf);
  }
}
