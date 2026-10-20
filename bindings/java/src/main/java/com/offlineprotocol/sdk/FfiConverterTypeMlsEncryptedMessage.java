package com.offlineprotocol.sdk;


public enum FfiConverterTypeMlsEncryptedMessage implements FfiConverterRustBuffer<MlsEncryptedMessage> {
  INSTANCE;

  @Override
  public MlsEncryptedMessage read(java.nio.ByteBuffer buf) {
    return new MlsEncryptedMessage(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MlsEncryptedMessage value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.groupId()) +
            FfiConverterString.INSTANCE.allocationSize(value.messageType()) +
            FfiConverterLong.INSTANCE.allocationSize(value.epoch()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.ciphertext()) +
            FfiConverterString.INSTANCE.allocationSize(value.senderId()) +
            FfiConverterLong.INSTANCE.allocationSize(value.timestampMs())
      );
  }

  @Override
  public void write(MlsEncryptedMessage value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.groupId(), buf);
      FfiConverterString.INSTANCE.write(value.messageType(), buf);
      FfiConverterLong.INSTANCE.write(value.epoch(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.ciphertext(), buf);
      FfiConverterString.INSTANCE.write(value.senderId(), buf);
      FfiConverterLong.INSTANCE.write(value.timestampMs(), buf);
  }
}
