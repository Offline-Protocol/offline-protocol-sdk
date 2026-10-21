package com.offlineprotocol.sdk;


public enum FfiConverterTypeTransportMetrics implements FfiConverterRustBuffer<TransportMetrics> {
  INSTANCE;

  @Override
  public TransportMetrics read(java.nio.ByteBuffer buf) {
    return new TransportMetrics(
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterOptionalShort.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalFloat.INSTANCE.read(buf),
      FfiConverterOptionalInteger.INSTANCE.read(buf),
      FfiConverterOptionalByte.INSTANCE.read(buf),
      FfiConverterOptionalBoolean.INSTANCE.read(buf),
      FfiConverterOptionalByte.INSTANCE.read(buf),
      FfiConverterOptionalBoolean.INSTANCE.read(buf),
      FfiConverterOptionalFloat.INSTANCE.read(buf),
      FfiConverterOptionalFloat.INSTANCE.read(buf),
      FfiConverterOptionalFloat.INSTANCE.read(buf),
      FfiConverterOptionalFloat.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(TransportMetrics value) {
      return (
            FfiConverterInteger.INSTANCE.allocationSize(value.packetsSent()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.packetsReceived()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.bytesSent()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.bytesReceived()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.errorRate()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.avgLatencyMs()) +
            FfiConverterOptionalShort.INSTANCE.allocationSize(value.rssi()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.bandwidthBps()) +
            FfiConverterOptionalFloat.INSTANCE.allocationSize(value.congestion()) +
            FfiConverterOptionalInteger.INSTANCE.allocationSize(value.queueDepth()) +
            FfiConverterOptionalByte.INSTANCE.allocationSize(value.batteryLevel()) +
            FfiConverterOptionalBoolean.INSTANCE.allocationSize(value.isCharging()) +
            FfiConverterOptionalByte.INSTANCE.allocationSize(value.relayConnectionCount()) +
            FfiConverterOptionalBoolean.INSTANCE.allocationSize(value.isActiveRelay()) +
            FfiConverterOptionalFloat.INSTANCE.allocationSize(value.deliveryRatio()) +
            FfiConverterOptionalFloat.INSTANCE.allocationSize(value.dropRate()) +
            FfiConverterOptionalFloat.INSTANCE.allocationSize(value.averageHopCount()) +
            FfiConverterOptionalFloat.INSTANCE.allocationSize(value.energyCost())
      );
  }

  @Override
  public void write(TransportMetrics value, java.nio.ByteBuffer buf) {
      FfiConverterInteger.INSTANCE.write(value.packetsSent(), buf);
      FfiConverterInteger.INSTANCE.write(value.packetsReceived(), buf);
      FfiConverterInteger.INSTANCE.write(value.bytesSent(), buf);
      FfiConverterInteger.INSTANCE.write(value.bytesReceived(), buf);
      FfiConverterFloat.INSTANCE.write(value.errorRate(), buf);
      FfiConverterInteger.INSTANCE.write(value.avgLatencyMs(), buf);
      FfiConverterOptionalShort.INSTANCE.write(value.rssi(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.bandwidthBps(), buf);
      FfiConverterOptionalFloat.INSTANCE.write(value.congestion(), buf);
      FfiConverterOptionalInteger.INSTANCE.write(value.queueDepth(), buf);
      FfiConverterOptionalByte.INSTANCE.write(value.batteryLevel(), buf);
      FfiConverterOptionalBoolean.INSTANCE.write(value.isCharging(), buf);
      FfiConverterOptionalByte.INSTANCE.write(value.relayConnectionCount(), buf);
      FfiConverterOptionalBoolean.INSTANCE.write(value.isActiveRelay(), buf);
      FfiConverterOptionalFloat.INSTANCE.write(value.deliveryRatio(), buf);
      FfiConverterOptionalFloat.INSTANCE.write(value.dropRate(), buf);
      FfiConverterOptionalFloat.INSTANCE.write(value.averageHopCount(), buf);
      FfiConverterOptionalFloat.INSTANCE.write(value.energyCost(), buf);
  }
}
