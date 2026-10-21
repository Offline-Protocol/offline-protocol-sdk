package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeCustodyConfig implements FfiConverterRustBuffer<CustodyConfig> {
  INSTANCE;

  @Override
  public CustodyConfig read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeCustodyConfig.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(CustodyConfig value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeCustodyConfig.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(CustodyConfig value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeCustodyConfig.INSTANCE.write(value, buf);
    }
  }
}
