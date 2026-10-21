package com.offlineprotocol.sdk;


public enum FfiConverterTypeNetworkLink implements FfiConverterRustBuffer<NetworkLink> {
  INSTANCE;

  @Override
  public NetworkLink read(java.nio.ByteBuffer buf) {
    return new NetworkLink(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(NetworkLink value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.sourceId()) +
            FfiConverterString.INSTANCE.allocationSize(value.targetId()) +
            FfiConverterString.INSTANCE.allocationSize(value.transport()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.quality())
      );
  }

  @Override
  public void write(NetworkLink value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.sourceId(), buf);
      FfiConverterString.INSTANCE.write(value.targetId(), buf);
      FfiConverterString.INSTANCE.write(value.transport(), buf);
      FfiConverterFloat.INSTANCE.write(value.quality(), buf);
  }
}
