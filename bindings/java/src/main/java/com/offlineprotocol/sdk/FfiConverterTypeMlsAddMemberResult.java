package com.offlineprotocol.sdk;


public enum FfiConverterTypeMlsAddMemberResult implements FfiConverterRustBuffer<MlsAddMemberResult> {
  INSTANCE;

  @Override
  public MlsAddMemberResult read(java.nio.ByteBuffer buf) {
    return new MlsAddMemberResult(
      FfiConverterTypeMlsWelcomeMessage.INSTANCE.read(buf),
      FfiConverterTypeMlsEncryptedMessage.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(MlsAddMemberResult value) {
      return (
            FfiConverterTypeMlsWelcomeMessage.INSTANCE.allocationSize(value.welcome()) +
            FfiConverterTypeMlsEncryptedMessage.INSTANCE.allocationSize(value.commit())
      );
  }

  @Override
  public void write(MlsAddMemberResult value, java.nio.ByteBuffer buf) {
      FfiConverterTypeMlsWelcomeMessage.INSTANCE.write(value.welcome(), buf);
      FfiConverterTypeMlsEncryptedMessage.INSTANCE.write(value.commit(), buf);
  }
}
