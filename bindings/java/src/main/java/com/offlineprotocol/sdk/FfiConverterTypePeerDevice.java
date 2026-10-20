package com.offlineprotocol.sdk;


public enum FfiConverterTypePeerDevice implements FfiConverterRustBuffer<PeerDevice> {
  INSTANCE;

  @Override
  public PeerDevice read(java.nio.ByteBuffer buf) {
    return new PeerDevice(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterShort.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(PeerDevice value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.peerId()) +
            FfiConverterShort.INSTANCE.allocationSize(value.rssi()) +
            FfiConverterLong.INSTANCE.allocationSize(value.lastSeenMs())
      );
  }

  @Override
  public void write(PeerDevice value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.peerId(), buf);
      FfiConverterShort.INSTANCE.write(value.rssi(), buf);
      FfiConverterLong.INSTANCE.write(value.lastSeenMs(), buf);
  }
}
