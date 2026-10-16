package com.offlineprotocol.sdk;


public enum FfiConverterTypeMessageStats implements FfiConverterRustBuffer<MessageStats> {
  INSTANCE;

  @Override
  public MessageStats read(java.nio.ByteBuffer buf) {
    return new MessageStats(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterByte.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MessageStats value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.messageId()) +
            FfiConverterLong.INSTANCE.allocationSize(value.sentAtMs()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.deliveredAtMs()) +
            FfiConverterByte.INSTANCE.allocationSize(value.hopCount()) +
            FfiConverterString.INSTANCE.allocationSize(value.status())
      );
  }

  @Override
  public void write(MessageStats value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.messageId(), buf);
      FfiConverterLong.INSTANCE.write(value.sentAtMs(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.deliveredAtMs(), buf);
      FfiConverterByte.INSTANCE.write(value.hopCount(), buf);
      FfiConverterString.INSTANCE.write(value.status(), buf);
  }
}
