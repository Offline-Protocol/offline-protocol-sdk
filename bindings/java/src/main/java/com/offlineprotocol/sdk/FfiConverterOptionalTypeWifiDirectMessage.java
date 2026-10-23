package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeWifiDirectMessage implements FfiConverterRustBuffer<WifiDirectMessage> {
  INSTANCE;

  @Override
  public WifiDirectMessage read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeWifiDirectMessage.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(WifiDirectMessage value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeWifiDirectMessage.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(WifiDirectMessage value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeWifiDirectMessage.INSTANCE.write(value, buf);
    }
  }
}
