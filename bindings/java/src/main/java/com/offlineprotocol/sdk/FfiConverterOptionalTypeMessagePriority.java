package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeMessagePriority implements FfiConverterRustBuffer<MessagePriority> {
  INSTANCE;

  @Override
  public MessagePriority read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeMessagePriority.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(MessagePriority value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeMessagePriority.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(MessagePriority value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeMessagePriority.INSTANCE.write(value, buf);
    }
  }
}
