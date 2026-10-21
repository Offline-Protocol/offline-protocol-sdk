package com.offlineprotocol.sdk;


public enum FfiConverterTypeEncryptionConfig implements FfiConverterRustBuffer<EncryptionConfig> {
  INSTANCE;

  @Override
  public EncryptionConfig read(java.nio.ByteBuffer buf) {
    return new EncryptionConfig(
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterTypePendingQueueConfig.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(EncryptionConfig value) {
      return (
            FfiConverterBoolean.INSTANCE.allocationSize(value.enabled()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.autoKeyExchange()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.storePending()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.requireEncryption()) +
            FfiConverterTypePendingQueueConfig.INSTANCE.allocationSize(value.pendingQueue())
      );
  }

  @Override
  public void write(EncryptionConfig value, java.nio.ByteBuffer buf) {
      FfiConverterBoolean.INSTANCE.write(value.enabled(), buf);
      FfiConverterBoolean.INSTANCE.write(value.autoKeyExchange(), buf);
      FfiConverterBoolean.INSTANCE.write(value.storePending(), buf);
      FfiConverterBoolean.INSTANCE.write(value.requireEncryption(), buf);
      FfiConverterTypePendingQueueConfig.INSTANCE.write(value.pendingQueue(), buf);
  }
}
