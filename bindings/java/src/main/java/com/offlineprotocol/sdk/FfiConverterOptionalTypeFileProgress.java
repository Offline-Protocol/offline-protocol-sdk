package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeFileProgress implements FfiConverterRustBuffer<FileProgress> {
  INSTANCE;

  @Override
  public FileProgress read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeFileProgress.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(FileProgress value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeFileProgress.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(FileProgress value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeFileProgress.INSTANCE.write(value, buf);
    }
  }
}
