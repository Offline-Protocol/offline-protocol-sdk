package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeWifiDirectIdentifiers implements FfiConverterRustBuffer<WifiDirectIdentifiers> {
  INSTANCE;

  @Override
  public WifiDirectIdentifiers read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeWifiDirectIdentifiers.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(WifiDirectIdentifiers value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeWifiDirectIdentifiers.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(WifiDirectIdentifiers value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeWifiDirectIdentifiers.INSTANCE.write(value, buf);
    }
  }
}
