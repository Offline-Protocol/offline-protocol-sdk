package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeContentType implements FfiConverterRustBuffer<ContentType> {
  INSTANCE;

  @Override
  public ContentType read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeContentType.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(ContentType value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeContentType.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(ContentType value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeContentType.INSTANCE.write(value, buf);
    }
  }
}
