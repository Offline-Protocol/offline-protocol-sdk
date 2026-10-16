package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeOverflowPolicy implements FfiConverterRustBuffer<OverflowPolicy> {
  INSTANCE;

  @Override
  public OverflowPolicy read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeOverflowPolicy.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(OverflowPolicy value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeOverflowPolicy.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(OverflowPolicy value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeOverflowPolicy.INSTANCE.write(value, buf);
    }
  }
}
