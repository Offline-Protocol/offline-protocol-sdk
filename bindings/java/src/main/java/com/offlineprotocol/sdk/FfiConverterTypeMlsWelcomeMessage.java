package com.offlineprotocol.sdk;


public enum FfiConverterTypeMlsWelcomeMessage implements FfiConverterRustBuffer<MlsWelcomeMessage> {
  INSTANCE;

  @Override
  public MlsWelcomeMessage read(java.nio.ByteBuffer buf) {
    return new MlsWelcomeMessage(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MlsWelcomeMessage value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.groupId()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.welcomeData()) +
            FfiConverterString.INSTANCE.allocationSize(value.inviterId()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.groupName()) +
            FfiConverterLong.INSTANCE.allocationSize(value.timestampMs())
      );
  }

  @Override
  public void write(MlsWelcomeMessage value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.groupId(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.welcomeData(), buf);
      FfiConverterString.INSTANCE.write(value.inviterId(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.groupName(), buf);
      FfiConverterLong.INSTANCE.write(value.timestampMs(), buf);
  }
}
