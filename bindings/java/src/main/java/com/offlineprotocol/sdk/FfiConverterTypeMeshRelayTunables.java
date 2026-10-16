package com.offlineprotocol.sdk;


public enum FfiConverterTypeMeshRelayTunables implements FfiConverterRustBuffer<MeshRelayTunables> {
  INSTANCE;

  @Override
  public MeshRelayTunables read(java.nio.ByteBuffer buf) {
    return new MeshRelayTunables(
      FfiConverterByte.INSTANCE.read(buf),
      FfiConverterByte.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MeshRelayTunables value) {
      return (
            FfiConverterByte.INSTANCE.allocationSize(value.maxTtl()) +
            FfiConverterByte.INSTANCE.allocationSize(value.denseMaxTtl()) +
            FfiConverterLong.INSTANCE.allocationSize(value.denseDegree()) +
            FfiConverterLong.INSTANCE.allocationSize(value.fanout()) +
            FfiConverterLong.INSTANCE.allocationSize(value.jitterMinMs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.jitterMaxMs()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.ratePerSec()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.burst()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.peerRatePerSec()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.peerBurst()) +
            FfiConverterLong.INSTANCE.allocationSize(value.queueCapacity()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.biasMinScale()) +
            FfiConverterLong.INSTANCE.allocationSize(value.biasMaxHandicapMs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.activityWindowMs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.activityMinForwards()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.activityIdleWindows())
      );
  }

  @Override
  public void write(MeshRelayTunables value, java.nio.ByteBuffer buf) {
      FfiConverterByte.INSTANCE.write(value.maxTtl(), buf);
      FfiConverterByte.INSTANCE.write(value.denseMaxTtl(), buf);
      FfiConverterLong.INSTANCE.write(value.denseDegree(), buf);
      FfiConverterLong.INSTANCE.write(value.fanout(), buf);
      FfiConverterLong.INSTANCE.write(value.jitterMinMs(), buf);
      FfiConverterLong.INSTANCE.write(value.jitterMaxMs(), buf);
      FfiConverterFloat.INSTANCE.write(value.ratePerSec(), buf);
      FfiConverterFloat.INSTANCE.write(value.burst(), buf);
      FfiConverterFloat.INSTANCE.write(value.peerRatePerSec(), buf);
      FfiConverterFloat.INSTANCE.write(value.peerBurst(), buf);
      FfiConverterLong.INSTANCE.write(value.queueCapacity(), buf);
      FfiConverterFloat.INSTANCE.write(value.biasMinScale(), buf);
      FfiConverterLong.INSTANCE.write(value.biasMaxHandicapMs(), buf);
      FfiConverterLong.INSTANCE.write(value.activityWindowMs(), buf);
      FfiConverterLong.INSTANCE.write(value.activityMinForwards(), buf);
      FfiConverterInteger.INSTANCE.write(value.activityIdleWindows(), buf);
  }
}
