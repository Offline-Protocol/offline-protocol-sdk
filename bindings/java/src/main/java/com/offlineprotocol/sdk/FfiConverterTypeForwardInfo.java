package com.offlineprotocol.sdk;


public enum FfiConverterTypeForwardInfo implements FfiConverterRustBuffer<ForwardInfo> {
  INSTANCE;

  @Override
  public ForwardInfo read(java.nio.ByteBuffer buf) {
    return new ForwardInfo(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(ForwardInfo value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.originalSender()) +
            FfiConverterString.INSTANCE.allocationSize(value.originalMessageId()) +
            FfiConverterLong.INSTANCE.allocationSize(value.originalTimestamp()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.forwardCount())
      );
  }

  @Override
  public void write(ForwardInfo value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.originalSender(), buf);
      FfiConverterString.INSTANCE.write(value.originalMessageId(), buf);
      FfiConverterLong.INSTANCE.write(value.originalTimestamp(), buf);
      FfiConverterInteger.INSTANCE.write(value.forwardCount(), buf);
  }
}
