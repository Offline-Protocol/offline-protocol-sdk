package com.offlineprotocol.sdk;


public enum FfiConverterTypeWebrtcDirectEvent implements FfiConverterRustBuffer<WebrtcDirectEvent> {
  INSTANCE;

  @Override
  public WebrtcDirectEvent read(java.nio.ByteBuffer buf) {
    return new WebrtcDirectEvent(
      FfiConverterTypeWebrtcDirectEventKind.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalShort.INSTANCE.read(buf),
      FfiConverterOptionalSequenceByte.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(WebrtcDirectEvent value) {
      return (
            FfiConverterTypeWebrtcDirectEventKind.INSTANCE.allocationSize(value.kind()) +
            FfiConverterLong.INSTANCE.allocationSize(value.stream()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.address()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.remoteIp()) +
            FfiConverterOptionalShort.INSTANCE.allocationSize(value.remotePort()) +
            FfiConverterOptionalSequenceByte.INSTANCE.allocationSize(value.data())
      );
  }

  @Override
  public void write(WebrtcDirectEvent value, java.nio.ByteBuffer buf) {
      FfiConverterTypeWebrtcDirectEventKind.INSTANCE.write(value.kind(), buf);
      FfiConverterLong.INSTANCE.write(value.stream(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.address(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.remoteIp(), buf);
      FfiConverterOptionalShort.INSTANCE.write(value.remotePort(), buf);
      FfiConverterOptionalSequenceByte.INSTANCE.write(value.data(), buf);
  }
}
