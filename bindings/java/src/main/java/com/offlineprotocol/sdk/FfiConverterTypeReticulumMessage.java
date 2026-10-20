package com.offlineprotocol.sdk;


public enum FfiConverterTypeReticulumMessage implements FfiConverterRustBuffer<ReticulumMessage> {
  INSTANCE;

  @Override
  public ReticulumMessage read(java.nio.ByteBuffer buf) {
    return new ReticulumMessage(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(ReticulumMessage value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.messageId()) +
            FfiConverterString.INSTANCE.allocationSize(value.recipientId()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.data()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.replyToMsg())
      );
  }

  @Override
  public void write(ReticulumMessage value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.messageId(), buf);
      FfiConverterString.INSTANCE.write(value.recipientId(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.data(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.replyToMsg(), buf);
  }
}
