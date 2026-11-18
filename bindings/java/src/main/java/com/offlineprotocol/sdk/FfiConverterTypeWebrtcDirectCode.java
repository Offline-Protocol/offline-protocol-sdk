package com.offlineprotocol.sdk;


public enum FfiConverterTypeWebrtcDirectCode implements FfiConverterRustBuffer<WebrtcDirectCode> {
  INSTANCE;

  @Override
  public WebrtcDirectCode read(java.nio.ByteBuffer buf) {
    return new WebrtcDirectCode(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterShort.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(WebrtcDirectCode value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.fragment()) +
            FfiConverterString.INSTANCE.allocationSize(value.host()) +
            FfiConverterShort.INSTANCE.allocationSize(value.port()) +
            FfiConverterString.INSTANCE.allocationSize(value.address())
      );
  }

  @Override
  public void write(WebrtcDirectCode value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.fragment(), buf);
      FfiConverterString.INSTANCE.write(value.host(), buf);
      FfiConverterShort.INSTANCE.write(value.port(), buf);
      FfiConverterString.INSTANCE.write(value.address(), buf);
  }
}
