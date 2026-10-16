package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeForwardInfo implements FfiConverterRustBuffer<ForwardInfo> {
  INSTANCE;

  @Override
  public ForwardInfo read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeForwardInfo.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(ForwardInfo value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeForwardInfo.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(ForwardInfo value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeForwardInfo.INSTANCE.write(value, buf);
    }
  }
}
