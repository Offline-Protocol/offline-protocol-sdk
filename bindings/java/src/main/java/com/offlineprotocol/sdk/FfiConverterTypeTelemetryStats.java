package com.offlineprotocol.sdk;


public enum FfiConverterTypeTelemetryStats implements FfiConverterRustBuffer<TelemetryStats> {
  INSTANCE;

  @Override
  public TelemetryStats read(java.nio.ByteBuffer buf) {
    return new TelemetryStats(
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(TelemetryStats value) {
      return (
            FfiConverterLong.INSTANCE.allocationSize(value.buffered()) +
            FfiConverterLong.INSTANCE.allocationSize(value.sentEvents()) +
            FfiConverterLong.INSTANCE.allocationSize(value.acceptedEvents()) +
            FfiConverterLong.INSTANCE.allocationSize(value.dropped()) +
            FfiConverterString.INSTANCE.allocationSize(value.sessionId()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.lastError()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.lastFlushAtMs())
      );
  }

  @Override
  public void write(TelemetryStats value, java.nio.ByteBuffer buf) {
      FfiConverterLong.INSTANCE.write(value.buffered(), buf);
      FfiConverterLong.INSTANCE.write(value.sentEvents(), buf);
      FfiConverterLong.INSTANCE.write(value.acceptedEvents(), buf);
      FfiConverterLong.INSTANCE.write(value.dropped(), buf);
      FfiConverterString.INSTANCE.write(value.sessionId(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.lastError(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.lastFlushAtMs(), buf);
  }
}
