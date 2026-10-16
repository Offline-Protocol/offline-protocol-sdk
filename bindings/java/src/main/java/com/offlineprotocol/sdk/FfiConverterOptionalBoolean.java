package com.offlineprotocol.sdk;

public enum FfiConverterOptionalBoolean implements FfiConverterRustBuffer<java.lang.Boolean> {
  INSTANCE;

  @Override
  public java.lang.Boolean read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterBoolean.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(java.lang.Boolean value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterBoolean.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(java.lang.Boolean value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterBoolean.INSTANCE.write(value, buf);
    }
  }
}
