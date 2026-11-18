package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeWebrtcDirectEvent implements FfiConverterRustBuffer<WebrtcDirectEvent> {
  INSTANCE;

  @Override
  public WebrtcDirectEvent read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeWebrtcDirectEvent.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(WebrtcDirectEvent value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeWebrtcDirectEvent.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(WebrtcDirectEvent value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeWebrtcDirectEvent.INSTANCE.write(value, buf);
    }
  }
}
