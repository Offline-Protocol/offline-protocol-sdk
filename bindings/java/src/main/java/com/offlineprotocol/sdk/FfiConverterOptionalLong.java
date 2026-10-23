package com.offlineprotocol.sdk;

public enum FfiConverterOptionalLong implements FfiConverterRustBuffer<java.lang.Long> {
  INSTANCE;

  @Override
  public java.lang.Long read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterLong.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(java.lang.Long value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterLong.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(java.lang.Long value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterLong.INSTANCE.write(value, buf);
    }
  }
}
