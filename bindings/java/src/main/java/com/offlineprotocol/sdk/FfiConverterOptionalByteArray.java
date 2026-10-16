package com.offlineprotocol.sdk;

public enum FfiConverterOptionalByteArray implements FfiConverterRustBuffer<byte[]> {
  INSTANCE;

  @Override
  public byte[] read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterByteArray.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(byte[] value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterByteArray.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(byte[] value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterByteArray.INSTANCE.write(value, buf);
    }
  }
}
