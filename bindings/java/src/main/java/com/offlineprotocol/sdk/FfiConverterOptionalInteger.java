package com.offlineprotocol.sdk;

public enum FfiConverterOptionalInteger implements FfiConverterRustBuffer<java.lang.Integer> {
  INSTANCE;

  @Override
  public java.lang.Integer read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterInteger.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(java.lang.Integer value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterInteger.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(java.lang.Integer value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterInteger.INSTANCE.write(value, buf);
    }
  }
}
