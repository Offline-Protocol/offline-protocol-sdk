package com.offlineprotocol.sdk;


public enum FfiConverterTypeMlsGroupInfo implements FfiConverterRustBuffer<MlsGroupInfo> {
  INSTANCE;

  @Override
  public MlsGroupInfo read(java.nio.ByteBuffer buf) {
    return new MlsGroupInfo(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterSequenceString.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MlsGroupInfo value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.groupId()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.name()) +
            FfiConverterSequenceString.INSTANCE.allocationSize(value.members()) +
            FfiConverterLong.INSTANCE.allocationSize(value.epoch()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.isSession()) +
            FfiConverterLong.INSTANCE.allocationSize(value.createdAtMs()) +
            FfiConverterLong.INSTANCE.allocationSize(value.lastActivityMs())
      );
  }

  @Override
  public void write(MlsGroupInfo value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.groupId(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.name(), buf);
      FfiConverterSequenceString.INSTANCE.write(value.members(), buf);
      FfiConverterLong.INSTANCE.write(value.epoch(), buf);
      FfiConverterBoolean.INSTANCE.write(value.isSession(), buf);
      FfiConverterLong.INSTANCE.write(value.createdAtMs(), buf);
      FfiConverterLong.INSTANCE.write(value.lastActivityMs(), buf);
  }
}
