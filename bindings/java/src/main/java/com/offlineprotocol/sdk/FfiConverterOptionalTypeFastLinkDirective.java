package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeFastLinkDirective implements FfiConverterRustBuffer<FastLinkDirective> {
  INSTANCE;

  @Override
  public FastLinkDirective read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeFastLinkDirective.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(FastLinkDirective value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeFastLinkDirective.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(FastLinkDirective value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeFastLinkDirective.INSTANCE.write(value, buf);
    }
  }
}
