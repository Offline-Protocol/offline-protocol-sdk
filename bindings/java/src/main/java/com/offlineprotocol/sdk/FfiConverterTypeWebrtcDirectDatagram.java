package com.offlineprotocol.sdk;


public enum FfiConverterTypeWebrtcDirectDatagram implements FfiConverterRustBuffer<WebrtcDirectDatagram> {
  INSTANCE;

  @Override
  public WebrtcDirectDatagram read(java.nio.ByteBuffer buf) {
    return new WebrtcDirectDatagram(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterShort.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(WebrtcDirectDatagram value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.destinationIp()) +
            FfiConverterShort.INSTANCE.allocationSize(value.destinationPort()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.data())
      );
  }

  @Override
  public void write(WebrtcDirectDatagram value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.destinationIp(), buf);
      FfiConverterShort.INSTANCE.write(value.destinationPort(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.data(), buf);
  }
}
