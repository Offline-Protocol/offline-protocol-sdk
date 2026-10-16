package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeMlsWelcomeMessage implements FfiConverterRustBuffer<MlsWelcomeMessage> {
  INSTANCE;

  @Override
  public MlsWelcomeMessage read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeMlsWelcomeMessage.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(MlsWelcomeMessage value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeMlsWelcomeMessage.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(MlsWelcomeMessage value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeMlsWelcomeMessage.INSTANCE.write(value, buf);
    }
  }
}
