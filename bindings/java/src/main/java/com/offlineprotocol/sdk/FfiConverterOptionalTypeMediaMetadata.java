package com.offlineprotocol.sdk;

public enum FfiConverterOptionalTypeMediaMetadata implements FfiConverterRustBuffer<MediaMetadata> {
  INSTANCE;

  @Override
  public MediaMetadata read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterTypeMediaMetadata.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(MediaMetadata value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterTypeMediaMetadata.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(MediaMetadata value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterTypeMediaMetadata.INSTANCE.write(value, buf);
    }
  }
}
