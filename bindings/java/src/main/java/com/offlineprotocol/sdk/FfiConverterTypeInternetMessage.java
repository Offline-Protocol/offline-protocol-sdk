package com.offlineprotocol.sdk;


public enum FfiConverterTypeInternetMessage implements FfiConverterRustBuffer<InternetMessage> {
  INSTANCE;

  @Override
  public InternetMessage read(java.nio.ByteBuffer buf) {
    return new InternetMessage(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(InternetMessage value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.messageId()) +
            FfiConverterString.INSTANCE.allocationSize(value.recipientId()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.data()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.replyToMsg()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.controlOp()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.controlPayload())
      );
  }

  @Override
  public void write(InternetMessage value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.messageId(), buf);
      FfiConverterString.INSTANCE.write(value.recipientId(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.data(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.replyToMsg(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.controlOp(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.controlPayload(), buf);
  }
}
