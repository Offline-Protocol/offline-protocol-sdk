package com.offlineprotocol.sdk;


public enum FfiConverterTypeNetworkNode implements FfiConverterRustBuffer<NetworkNode> {
  INSTANCE;

  @Override
  public NetworkNode read(java.nio.ByteBuffer buf) {
    return new NetworkNode(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterOptionalShort.INSTANCE.read(buf),
      FfiConverterOptionalByte.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(NetworkNode value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.nodeId()) +
            FfiConverterString.INSTANCE.allocationSize(value.role()) +
            FfiConverterOptionalShort.INSTANCE.allocationSize(value.rssi()) +
            FfiConverterOptionalByte.INSTANCE.allocationSize(value.batteryLevel()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.connectionCount()) +
            FfiConverterLong.INSTANCE.allocationSize(value.lastSeenMs())
      );
  }

  @Override
  public void write(NetworkNode value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.nodeId(), buf);
      FfiConverterString.INSTANCE.write(value.role(), buf);
      FfiConverterOptionalShort.INSTANCE.write(value.rssi(), buf);
      FfiConverterOptionalByte.INSTANCE.write(value.batteryLevel(), buf);
      FfiConverterInteger.INSTANCE.write(value.connectionCount(), buf);
      FfiConverterLong.INSTANCE.write(value.lastSeenMs(), buf);
  }
}
