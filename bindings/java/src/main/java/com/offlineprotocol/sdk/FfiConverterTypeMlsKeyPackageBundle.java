package com.offlineprotocol.sdk;


public enum FfiConverterTypeMlsKeyPackageBundle implements FfiConverterRustBuffer<MlsKeyPackageBundle> {
  INSTANCE;

  @Override
  public MlsKeyPackageBundle read(java.nio.ByteBuffer buf) {
    return new MlsKeyPackageBundle(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MlsKeyPackageBundle value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.packageId()) +
            FfiConverterString.INSTANCE.allocationSize(value.userId()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.keyPackageData()) +
            FfiConverterLong.INSTANCE.allocationSize(value.createdAtMs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.expiresAtMs()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.synced())
      );
  }

  @Override
  public void write(MlsKeyPackageBundle value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.packageId(), buf);
      FfiConverterString.INSTANCE.write(value.userId(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.keyPackageData(), buf);
      FfiConverterLong.INSTANCE.write(value.createdAtMs(), buf);
      FfiConverterLong.INSTANCE.write(value.expiresAtMs(), buf);
      FfiConverterBoolean.INSTANCE.write(value.synced(), buf);
  }
}
