package com.offlineprotocol.sdk;


public enum FfiConverterTypeFileProgress implements FfiConverterRustBuffer<FileProgress> {
  INSTANCE;

  @Override
  public FileProgress read(java.nio.ByteBuffer buf) {
    return new FileProgress(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterByte.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(FileProgress value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.fileId()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.chunksSent()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.totalChunks()) +
            FfiConverterByte.INSTANCE.allocationSize(value.percentage())
      );
  }

  @Override
  public void write(FileProgress value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.fileId(), buf);
      FfiConverterInteger.INSTANCE.write(value.chunksSent(), buf);
      FfiConverterInteger.INSTANCE.write(value.totalChunks(), buf);
      FfiConverterByte.INSTANCE.write(value.percentage(), buf);
  }
}
