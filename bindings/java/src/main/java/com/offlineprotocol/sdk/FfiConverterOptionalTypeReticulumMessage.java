package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeReticulumMessage implements FfiConverterRustBuffer<ReticulumMessage> {
  INSTANCE;

  @Override
  public ReticulumMessage read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeReticulumMessage.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(ReticulumMessage value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeReticulumMessage.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(ReticulumMessage value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeReticulumMessage.INSTANCE.write(value, buf);
    }
  }
}
