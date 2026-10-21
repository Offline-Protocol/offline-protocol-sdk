package com.offlineprotocol.sdk;


public enum FfiConverterTypeMediaSendOptions implements FfiConverterRustBuffer<MediaSendOptions> {
  INSTANCE;

  @Override
  public MediaSendOptions read(java.nio.ByteBuffer buf) {
    return new MediaSendOptions(
      FfiConverterOptionalTypeMediaMetadata.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalTypeReplyContext.INSTANCE.read(buf),
      FfiConverterOptionalTypeForwardInfo.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MediaSendOptions value) {
      return (
            FfiConverterOptionalTypeMediaMetadata.INSTANCE.allocationSize(value.mediaMetadata()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.caption()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.replyToMsg()) +
            FfiConverterOptionalTypeReplyContext.INSTANCE.allocationSize(value.replyContext()) +
            FfiConverterOptionalTypeForwardInfo.INSTANCE.allocationSize(value.forwardInfo()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.fileId()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.appId())
      );
  }

  @Override
  public void write(MediaSendOptions value, java.nio.ByteBuffer buf) {
      FfiConverterOptionalTypeMediaMetadata.INSTANCE.write(value.mediaMetadata(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.caption(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.replyToMsg(), buf);
      FfiConverterOptionalTypeReplyContext.INSTANCE.write(value.replyContext(), buf);
      FfiConverterOptionalTypeForwardInfo.INSTANCE.write(value.forwardInfo(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.fileId(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.appId(), buf);
  }
}
