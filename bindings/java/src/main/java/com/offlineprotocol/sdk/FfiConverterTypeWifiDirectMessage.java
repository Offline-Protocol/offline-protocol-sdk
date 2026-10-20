package com.offlineprotocol.sdk;


public enum FfiConverterTypeWifiDirectMessage implements FfiConverterRustBuffer<WifiDirectMessage> {
  INSTANCE;

  @Override
  public WifiDirectMessage read(java.nio.ByteBuffer buf) {
    return new WifiDirectMessage(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(WifiDirectMessage value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.recipientId()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.data())
      );
  }

  @Override
  public void write(WifiDirectMessage value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.recipientId(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.data(), buf);
  }
}
