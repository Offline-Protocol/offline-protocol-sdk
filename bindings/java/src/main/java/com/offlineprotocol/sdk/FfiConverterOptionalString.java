package com.offlineprotocol.sdk;

public enum FfiConverterOptionalString implements FfiConverterRustBuffer<java.lang.String> {
  INSTANCE;

  @Override
  public java.lang.String read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterString.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(java.lang.String value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterString.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(java.lang.String value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterString.INSTANCE.write(value, buf);
    }
  }
}
