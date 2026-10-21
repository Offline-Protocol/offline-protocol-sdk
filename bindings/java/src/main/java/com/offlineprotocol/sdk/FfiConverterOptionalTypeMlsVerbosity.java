package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeMlsVerbosity implements FfiConverterRustBuffer<MlsVerbosity> {
  INSTANCE;

  @Override
  public MlsVerbosity read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeMlsVerbosity.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(MlsVerbosity value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeMlsVerbosity.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(MlsVerbosity value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeMlsVerbosity.INSTANCE.write(value, buf);
    }
  }
}
