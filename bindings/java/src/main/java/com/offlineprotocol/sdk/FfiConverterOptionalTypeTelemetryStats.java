package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeTelemetryStats implements FfiConverterRustBuffer<TelemetryStats> {
  INSTANCE;

  @Override
  public TelemetryStats read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeTelemetryStats.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(TelemetryStats value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeTelemetryStats.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(TelemetryStats value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeTelemetryStats.INSTANCE.write(value, buf);
    }
  }
}
