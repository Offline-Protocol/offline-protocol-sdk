package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeInternetMessage implements FfiConverterRustBuffer<InternetMessage> {
  INSTANCE;

  @Override
  public InternetMessage read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeInternetMessage.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(InternetMessage value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeInternetMessage.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(InternetMessage value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeInternetMessage.INSTANCE.write(value, buf);
    }
  }
}
