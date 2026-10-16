package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeBleFragment implements FfiConverterRustBuffer<BleFragment> {
  INSTANCE;

  @Override
  public BleFragment read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeBleFragment.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(BleFragment value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeBleFragment.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(BleFragment value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeBleFragment.INSTANCE.write(value, buf);
    }
  }
}
