package com.offlineprotocol.sdk;


public enum FfiConverterTypeRelayConfig implements FfiConverterRustBuffer<RelayConfig> {
  INSTANCE;

  @Override
  public RelayConfig read(java.nio.ByteBuffer buf) {
    return new RelayConfig(
      FfiConverterByte.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterTypeRelayPriority.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(RelayConfig value) {
      return (
            FfiConverterByte.INSTANCE.allocationSize(value.minBatteryForRelay()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.allowRelay()) +
            FfiConverterTypeRelayPriority.INSTANCE.allocationSize(value.relayPriority())
      );
  }

  @Override
  public void write(RelayConfig value, java.nio.ByteBuffer buf) {
      FfiConverterByte.INSTANCE.write(value.minBatteryForRelay(), buf);
      FfiConverterBoolean.INSTANCE.write(value.allowRelay(), buf);
      FfiConverterTypeRelayPriority.INSTANCE.write(value.relayPriority(), buf);
  }
}
