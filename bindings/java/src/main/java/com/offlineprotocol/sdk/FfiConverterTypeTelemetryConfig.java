package com.offlineprotocol.sdk;


public enum FfiConverterTypeTelemetryConfig implements FfiConverterRustBuffer<TelemetryConfig> {
  INSTANCE;

  @Override
  public TelemetryConfig read(java.nio.ByteBuffer buf) {
    return new TelemetryConfig(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterTypeTelemetryOs.INSTANCE.read(buf),
      FfiConverterShort.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalBoolean.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalInteger.INSTANCE.read(buf),
      FfiConverterOptionalBoolean.INSTANCE.read(buf),
      FfiConverterOptionalBoolean.INSTANCE.read(buf),
      FfiConverterOptionalTypeMlsVerbosity.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalBoolean.INSTANCE.read(buf),
      FfiConverterOptionalBoolean.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(TelemetryConfig value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.apiKey()) +
            FfiConverterString.INSTANCE.allocationSize(value.appId()) +
            FfiConverterTypeTelemetryOs.INSTANCE.allocationSize(value.os()) +
            FfiConverterShort.INSTANCE.allocationSize(value.osMajor()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.appVersion()) +
            FfiConverterOptionalBoolean.INSTANCE.allocationSize(value.debug()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.flushIntervalMs()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.maxBatchBytes()) +
            FfiConverterOptionalInteger.INSTANCE.allocationSize(value.maxBufferedRecords()) +
            FfiConverterOptionalBoolean.INSTANCE.allocationSize(value.includeDeviceId()) +
            FfiConverterOptionalBoolean.INSTANCE.allocationSize(value.scrubIds()) +
            FfiConverterOptionalTypeMlsVerbosity.INSTANCE.allocationSize(value.mlsVerbosity()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.metricsCadenceMs()) +
            FfiConverterOptionalBoolean.INSTANCE.allocationSize(value.routingDiagnostic()) +
            FfiConverterOptionalBoolean.INSTANCE.allocationSize(value.mlsSamplingBypass())
      );
  }

  @Override
  public void write(TelemetryConfig value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.apiKey(), buf);
      FfiConverterString.INSTANCE.write(value.appId(), buf);
      FfiConverterTypeTelemetryOs.INSTANCE.write(value.os(), buf);
      FfiConverterShort.INSTANCE.write(value.osMajor(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.appVersion(), buf);
      FfiConverterOptionalBoolean.INSTANCE.write(value.debug(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.flushIntervalMs(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.maxBatchBytes(), buf);
      FfiConverterOptionalInteger.INSTANCE.write(value.maxBufferedRecords(), buf);
      FfiConverterOptionalBoolean.INSTANCE.write(value.includeDeviceId(), buf);
      FfiConverterOptionalBoolean.INSTANCE.write(value.scrubIds(), buf);
      FfiConverterOptionalTypeMlsVerbosity.INSTANCE.write(value.mlsVerbosity(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.metricsCadenceMs(), buf);
      FfiConverterOptionalBoolean.INSTANCE.write(value.routingDiagnostic(), buf);
      FfiConverterOptionalBoolean.INSTANCE.write(value.mlsSamplingBypass(), buf);
  }
}
