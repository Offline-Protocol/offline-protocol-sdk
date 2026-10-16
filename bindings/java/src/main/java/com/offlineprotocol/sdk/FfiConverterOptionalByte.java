package com.offlineprotocol.sdk;

public enum FfiConverterOptionalByte implements FfiConverterRustBuffer<java.lang.Byte> {
  INSTANCE;

  @Override
  public java.lang.Byte read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterByte.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(java.lang.Byte value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterByte.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(java.lang.Byte value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterByte.INSTANCE.write(value, buf);
    }
  }
}
