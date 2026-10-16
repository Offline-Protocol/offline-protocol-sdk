package com.offlineprotocol.sdk;


public enum FfiConverterTypeTransportConfig implements FfiConverterRustBuffer<TransportConfig> {
  INSTANCE;

  @Override
  public TransportConfig read(java.nio.ByteBuffer buf) {
    return new TransportConfig(
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(TransportConfig value) {
      return (
            FfiConverterBoolean.INSTANCE.allocationSize(value.bleEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.wifiDirectEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.internetEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.reticulumEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.nostrEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.nostrSealingEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.nostrColdContactEnabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.nostrUsernameDiscoveryEnabled())
      );
  }

  @Override
  public void write(TransportConfig value, java.nio.ByteBuffer buf) {
      FfiConverterBoolean.INSTANCE.write(value.bleEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.wifiDirectEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.internetEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.reticulumEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.nostrEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.nostrSealingEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.nostrColdContactEnabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.nostrUsernameDiscoveryEnabled(), buf);
  }
}
