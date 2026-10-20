package com.offlineprotocol.sdk;

public enum FfiConverterOptionalFloat implements FfiConverterRustBuffer<java.lang.Float> {
  INSTANCE;

  @Override
  public java.lang.Float read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterFloat.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(java.lang.Float value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterFloat.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(java.lang.Float value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterFloat.INSTANCE.write(value, buf);
    }
  }
}
