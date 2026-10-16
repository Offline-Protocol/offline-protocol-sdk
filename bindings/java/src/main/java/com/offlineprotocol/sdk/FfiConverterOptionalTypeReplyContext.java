package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeReplyContext implements FfiConverterRustBuffer<ReplyContext> {
  INSTANCE;

  @Override
  public ReplyContext read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeReplyContext.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(ReplyContext value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeReplyContext.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(ReplyContext value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeReplyContext.INSTANCE.write(value, buf);
    }
  }
}
