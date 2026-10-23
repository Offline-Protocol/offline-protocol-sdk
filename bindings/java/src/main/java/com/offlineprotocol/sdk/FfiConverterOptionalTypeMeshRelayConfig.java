package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeMeshRelayConfig implements FfiConverterRustBuffer<MeshRelayConfig> {
  INSTANCE;

  @Override
  public MeshRelayConfig read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeMeshRelayConfig.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(MeshRelayConfig value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeMeshRelayConfig.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(MeshRelayConfig value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeMeshRelayConfig.INSTANCE.write(value, buf);
    }
  }
}
