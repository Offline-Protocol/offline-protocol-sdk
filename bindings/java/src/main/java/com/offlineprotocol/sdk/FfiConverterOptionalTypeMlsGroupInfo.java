package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeMlsGroupInfo implements FfiConverterRustBuffer<MlsGroupInfo> {
  INSTANCE;

  @Override
  public MlsGroupInfo read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeMlsGroupInfo.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(MlsGroupInfo value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeMlsGroupInfo.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(MlsGroupInfo value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeMlsGroupInfo.INSTANCE.write(value, buf);
    }
  }
}
