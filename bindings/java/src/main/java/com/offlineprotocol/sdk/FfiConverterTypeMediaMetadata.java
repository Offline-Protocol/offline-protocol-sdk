package com.offlineprotocol.sdk;


public enum FfiConverterTypeMediaMetadata implements FfiConverterRustBuffer<MediaMetadata> {
  INSTANCE;

  @Override
  public MediaMetadata read(java.nio.ByteBuffer buf) {
    return new MediaMetadata(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterOptionalInteger.INSTANCE.read(buf),
      FfiConverterOptionalInteger.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MediaMetadata value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.mimeType()) +
            FfiConverterString.INSTANCE.allocationSize(value.fileName()) +
            FfiConverterLong.INSTANCE.allocationSize(value.fileSize()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.durationMs()) +
            FfiConverterOptionalInteger.INSTANCE.allocationSize(value.width()) +
            FfiConverterOptionalInteger.INSTANCE.allocationSize(value.height()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.thumbnailBase64()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.mediaId()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.downloadUrl()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.thumbnailUrl()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.encryptionKey()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.iv()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.ciphertextHash()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.stickerProvider()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.stickerRemoteId()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.stickerKind())
      );
  }

  @Override
  public void write(MediaMetadata value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.mimeType(), buf);
      FfiConverterString.INSTANCE.write(value.fileName(), buf);
      FfiConverterLong.INSTANCE.write(value.fileSize(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.durationMs(), buf);
      FfiConverterOptionalInteger.INSTANCE.write(value.width(), buf);
      FfiConverterOptionalInteger.INSTANCE.write(value.height(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.thumbnailBase64(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.mediaId(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.downloadUrl(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.thumbnailUrl(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.encryptionKey(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.iv(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.ciphertextHash(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.stickerProvider(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.stickerRemoteId(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.stickerKind(), buf);
  }
}
