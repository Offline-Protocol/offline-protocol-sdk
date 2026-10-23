package com.offlineprotocol.sdk;


public enum FfiConverterTypeDorsConfig implements FfiConverterRustBuffer<DorsConfig> {
  INSTANCE;

  @Override
  public DorsConfig read(java.nio.ByteBuffer buf) {
    return new DorsConfig(
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterShort.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterByte.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterFloat.INSTANCE.read(buf),
      FfiConverterByte.INSTANCE.read(buf),
      FfiConverterByte.INSTANCE.read(buf),
      FfiConverterByte.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(DorsConfig value) {
      return (
            FfiConverterBoolean.INSTANCE.allocationSize(value.preferOnline()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.switchHysteresis()) +
            FfiConverterLong.INSTANCE.allocationSize(value.switchCooldownSecs()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.bleToWifiRetryThreshold()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.minSuccessRateBeforeEscalation()) +
            FfiConverterLong.INSTANCE.allocationSize(value.minBleSamplesBeforeSuccessRateEscalation()) +
            FfiConverterShort.INSTANCE.allocationSize(value.rssiSwitchThreshold()) +
            FfiConverterLong.INSTANCE.allocationSize(value.congestionQueueThreshold()) +
            FfiConverterLong.INSTANCE.allocationSize(value.stabilityWindowSecs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.poorSignalDurationSecs()) +
            FfiConverterByte.INSTANCE.allocationSize(value.ttlEscalationThreshold()) +
            FfiConverterLong.INSTANCE.allocationSize(value.congestionDurationSecs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.ttlEscalationHoldSecs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.historyWindowSize()) +
            FfiConverterFloat.INSTANCE.allocationSize(value.queueRecoveryRatio()) +
            FfiConverterByte.INSTANCE.allocationSize(value.lowBatteryThreshold()) +
            FfiConverterByte.INSTANCE.allocationSize(value.relayMinBatteryLevel()) +
            FfiConverterByte.INSTANCE.allocationSize(value.relayOptimalConnectionCount())
      );
  }

  @Override
  public void write(DorsConfig value, java.nio.ByteBuffer buf) {
      FfiConverterBoolean.INSTANCE.write(value.preferOnline(), buf);
      FfiConverterFloat.INSTANCE.write(value.switchHysteresis(), buf);
      FfiConverterLong.INSTANCE.write(value.switchCooldownSecs(), buf);
      FfiConverterInteger.INSTANCE.write(value.bleToWifiRetryThreshold(), buf);
      FfiConverterFloat.INSTANCE.write(value.minSuccessRateBeforeEscalation(), buf);
      FfiConverterLong.INSTANCE.write(value.minBleSamplesBeforeSuccessRateEscalation(), buf);
      FfiConverterShort.INSTANCE.write(value.rssiSwitchThreshold(), buf);
      FfiConverterLong.INSTANCE.write(value.congestionQueueThreshold(), buf);
      FfiConverterLong.INSTANCE.write(value.stabilityWindowSecs(), buf);
      FfiConverterLong.INSTANCE.write(value.poorSignalDurationSecs(), buf);
      FfiConverterByte.INSTANCE.write(value.ttlEscalationThreshold(), buf);
      FfiConverterLong.INSTANCE.write(value.congestionDurationSecs(), buf);
      FfiConverterLong.INSTANCE.write(value.ttlEscalationHoldSecs(), buf);
      FfiConverterLong.INSTANCE.write(value.historyWindowSize(), buf);
      FfiConverterFloat.INSTANCE.write(value.queueRecoveryRatio(), buf);
      FfiConverterByte.INSTANCE.write(value.lowBatteryThreshold(), buf);
      FfiConverterByte.INSTANCE.write(value.relayMinBatteryLevel(), buf);
      FfiConverterByte.INSTANCE.write(value.relayOptimalConnectionCount(), buf);
  }
}
