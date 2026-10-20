package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeTransportMetrics implements FfiConverterRustBuffer<TransportMetrics> {
  INSTANCE;

  @Override
  public TransportMetrics read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeTransportMetrics.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(TransportMetrics value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeTransportMetrics.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(TransportMetrics value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeTransportMetrics.INSTANCE.write(value, buf);
    }
  }
}
