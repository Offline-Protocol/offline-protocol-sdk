package com.offlineprotocol.sdk;


public enum FfiConverterTypeWifiDirectIdentifiers implements FfiConverterRustBuffer<WifiDirectIdentifiers> {
  INSTANCE;

  @Override
  public WifiDirectIdentifiers read(java.nio.ByteBuffer buf) {
    return new WifiDirectIdentifiers(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(WifiDirectIdentifiers value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.recordApp()) +
            FfiConverterString.INSTANCE.allocationSize(value.networkName()) +
            FfiConverterString.INSTANCE.allocationSize(value.passphrase())
      );
  }

  @Override
  public void write(WifiDirectIdentifiers value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.recordApp(), buf);
      FfiConverterString.INSTANCE.write(value.networkName(), buf);
      FfiConverterString.INSTANCE.write(value.passphrase(), buf);
  }
}
