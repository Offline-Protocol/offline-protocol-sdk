package com.offlineprotocol.sdk;


public enum FfiConverterTypeReplyContext implements FfiConverterRustBuffer<ReplyContext> {
  INSTANCE;

  @Override
  public ReplyContext read(java.nio.ByteBuffer buf) {
    return new ReplyContext(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(ReplyContext value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.sender()) +
            FfiConverterString.INSTANCE.allocationSize(value.text()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.timestamp()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.replyMediaLabel()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.replyContentType())
      );
  }

  @Override
  public void write(ReplyContext value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.sender(), buf);
      FfiConverterString.INSTANCE.write(value.text(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.timestamp(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.replyMediaLabel(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.replyContentType(), buf);
  }
}
