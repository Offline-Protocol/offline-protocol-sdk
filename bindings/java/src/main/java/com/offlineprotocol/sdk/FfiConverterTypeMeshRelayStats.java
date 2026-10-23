package com.offlineprotocol.sdk;


public enum FfiConverterTypeMeshRelayStats implements FfiConverterRustBuffer<MeshRelayStats> {
  INSTANCE;

  @Override
  public MeshRelayStats read(java.nio.ByteBuffer buf) {
    return new MeshRelayStats(
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MeshRelayStats value) {
      return (
            FfiConverterLong.INSTANCE.allocationSize(value.forwarded()) +
            FfiConverterLong.INSTANCE.allocationSize(value.transmissions()) +
            FfiConverterLong.INSTANCE.allocationSize(value.queued()) +
            FfiConverterLong.INSTANCE.allocationSize(value.awaitingTransmission()) +
            FfiConverterLong.INSTANCE.allocationSize(value.duplicatesSuppressed()) +
            FfiConverterLong.INSTANCE.allocationSize(value.coveredByANeighbor()) +
            FfiConverterLong.INSTANCE.allocationSize(value.peerRateLimited()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedQueueFull()) +
            FfiConverterLong.INSTANCE.allocationSize(value.rateDeferred()) +
            FfiConverterLong.INSTANCE.allocationSize(value.abandonedOverdue()) +
            FfiConverterLong.INSTANCE.allocationSize(value.hopLimitReached()) +
            FfiConverterLong.INSTANCE.allocationSize(value.reachClamped()) +
            FfiConverterLong.INSTANCE.allocationSize(value.droppedForCapacity())
      );
  }

  @Override
  public void write(MeshRelayStats value, java.nio.ByteBuffer buf) {
      FfiConverterLong.INSTANCE.write(value.forwarded(), buf);
      FfiConverterLong.INSTANCE.write(value.transmissions(), buf);
      FfiConverterLong.INSTANCE.write(value.queued(), buf);
      FfiConverterLong.INSTANCE.write(value.awaitingTransmission(), buf);
      FfiConverterLong.INSTANCE.write(value.duplicatesSuppressed(), buf);
      FfiConverterLong.INSTANCE.write(value.coveredByANeighbor(), buf);
      FfiConverterLong.INSTANCE.write(value.peerRateLimited(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedQueueFull(), buf);
      FfiConverterLong.INSTANCE.write(value.rateDeferred(), buf);
      FfiConverterLong.INSTANCE.write(value.abandonedOverdue(), buf);
      FfiConverterLong.INSTANCE.write(value.hopLimitReached(), buf);
      FfiConverterLong.INSTANCE.write(value.reachClamped(), buf);
      FfiConverterLong.INSTANCE.write(value.droppedForCapacity(), buf);
  }
}
