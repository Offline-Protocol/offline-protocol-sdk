package com.offlineprotocol.sdk;


public enum FfiConverterTypeSendMessageOptions implements FfiConverterRustBuffer<SendMessageOptions> {
  INSTANCE;

  @Override
  public SendMessageOptions read(java.nio.ByteBuffer buf) {
    return new SendMessageOptions(
      FfiConverterOptionalTypeMessagePriority.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalTypeContentType.INSTANCE.read(buf),
      FfiConverterOptionalTypeReplyContext.INSTANCE.read(buf),
      FfiConverterOptionalTypeMediaMetadata.INSTANCE.read(buf),
      FfiConverterOptionalTypeForwardInfo.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(SendMessageOptions value) {
      return (
            FfiConverterOptionalTypeMessagePriority.INSTANCE.allocationSize(value.priority()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.replyToMsg()) +
            FfiConverterOptionalTypeContentType.INSTANCE.allocationSize(value.contentType()) +
            FfiConverterOptionalTypeReplyContext.INSTANCE.allocationSize(value.replyContext()) +
            FfiConverterOptionalTypeMediaMetadata.INSTANCE.allocationSize(value.mediaMetadata()) +
            FfiConverterOptionalTypeForwardInfo.INSTANCE.allocationSize(value.forwardInfo()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.appId())
      );
  }

  @Override
  public void write(SendMessageOptions value, java.nio.ByteBuffer buf) {
      FfiConverterOptionalTypeMessagePriority.INSTANCE.write(value.priority(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.replyToMsg(), buf);
      FfiConverterOptionalTypeContentType.INSTANCE.write(value.contentType(), buf);
      FfiConverterOptionalTypeReplyContext.INSTANCE.write(value.replyContext(), buf);
      FfiConverterOptionalTypeMediaMetadata.INSTANCE.write(value.mediaMetadata(), buf);
      FfiConverterOptionalTypeForwardInfo.INSTANCE.write(value.forwardInfo(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.appId(), buf);
  }
}
