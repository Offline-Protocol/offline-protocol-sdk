package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeNostrMessage implements FfiConverterRustBuffer<NostrMessage> {
  INSTANCE;

  @Override
  public NostrMessage read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeNostrMessage.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(NostrMessage value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeNostrMessage.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(NostrMessage value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeNostrMessage.INSTANCE.write(value, buf);
    }
  }
}
