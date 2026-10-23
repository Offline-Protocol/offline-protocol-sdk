package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeNostrQuery implements FfiConverterRustBuffer<NostrQuery> {
  INSTANCE;

  @Override
  public NostrQuery read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeNostrQuery.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(NostrQuery value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeNostrQuery.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(NostrQuery value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeNostrQuery.INSTANCE.write(value, buf);
    }
  }
}
