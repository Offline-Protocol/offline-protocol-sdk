package com.offlineprotocol.sdk;


public enum FfiConverterTypeMeshRelayConfig implements FfiConverterRustBuffer<MeshRelayConfig> {
  INSTANCE;

  @Override
  public MeshRelayConfig read(java.nio.ByteBuffer buf) {
    return new MeshRelayConfig(
      FfiConverterOptionalByte.INSTANCE.read(buf),
      FfiConverterOptionalByte.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalFloat.INSTANCE.read(buf),
      FfiConverterOptionalFloat.INSTANCE.read(buf),
      FfiConverterOptionalFloat.INSTANCE.read(buf),
      FfiConverterOptionalFloat.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalFloat.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalInteger.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MeshRelayConfig value) {
      return (
            FfiConverterOptionalByte.INSTANCE.allocationSize(value.maxTtl()) +
            FfiConverterOptionalByte.INSTANCE.allocationSize(value.denseMaxTtl()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.denseDegree()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.fanout()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.jitterMinMs()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.jitterMaxMs()) +
            FfiConverterOptionalFloat.INSTANCE.allocationSize(value.ratePerSec()) +
            FfiConverterOptionalFloat.INSTANCE.allocationSize(value.burst()) +
            FfiConverterOptionalFloat.INSTANCE.allocationSize(value.peerRatePerSec()) +
            FfiConverterOptionalFloat.INSTANCE.allocationSize(value.peerBurst()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.queueCapacity()) +
            FfiConverterOptionalFloat.INSTANCE.allocationSize(value.biasMinScale()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.biasMaxHandicapMs()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.activityWindowMs()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.activityMinForwards()) +
            FfiConverterOptionalInteger.INSTANCE.allocationSize(value.activityIdleWindows())
      );
  }

  @Override
  public void write(MeshRelayConfig value, java.nio.ByteBuffer buf) {
      FfiConverterOptionalByte.INSTANCE.write(value.maxTtl(), buf);
      FfiConverterOptionalByte.INSTANCE.write(value.denseMaxTtl(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.denseDegree(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.fanout(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.jitterMinMs(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.jitterMaxMs(), buf);
      FfiConverterOptionalFloat.INSTANCE.write(value.ratePerSec(), buf);
      FfiConverterOptionalFloat.INSTANCE.write(value.burst(), buf);
      FfiConverterOptionalFloat.INSTANCE.write(value.peerRatePerSec(), buf);
      FfiConverterOptionalFloat.INSTANCE.write(value.peerBurst(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.queueCapacity(), buf);
      FfiConverterOptionalFloat.INSTANCE.write(value.biasMinScale(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.biasMaxHandicapMs(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.activityWindowMs(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.activityMinForwards(), buf);
      FfiConverterOptionalInteger.INSTANCE.write(value.activityIdleWindows(), buf);
  }
}
