package com.offlineprotocol.sdk;


public enum FfiConverterTypeBleFragment implements FfiConverterRustBuffer<BleFragment> {
  INSTANCE;

  @Override
  public BleFragment read(java.nio.ByteBuffer buf) {
    return new BleFragment(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(BleFragment value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.recipientId()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.data())
      );
  }

  @Override
  public void write(BleFragment value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.recipientId(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.data(), buf);
  }
}
