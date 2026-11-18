package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeWebrtcDirectDatagram implements FfiConverterRustBuffer<WebrtcDirectDatagram> {
  INSTANCE;

  @Override
  public WebrtcDirectDatagram read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeWebrtcDirectDatagram.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(WebrtcDirectDatagram value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeWebrtcDirectDatagram.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(WebrtcDirectDatagram value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeWebrtcDirectDatagram.INSTANCE.write(value, buf);
    }
  }
}
