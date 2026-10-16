package com.offlineprotocol.sdk;

public enum FfiConverterOptionalShort implements FfiConverterRustBuffer<java.lang.Short> {
  INSTANCE;

  @Override
  public java.lang.Short read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterShort.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(java.lang.Short value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterShort.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(java.lang.Short value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterShort.INSTANCE.write(value, buf);
    }
  }
}
