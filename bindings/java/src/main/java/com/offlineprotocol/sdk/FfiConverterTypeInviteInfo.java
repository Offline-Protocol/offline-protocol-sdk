package com.offlineprotocol.sdk;


public enum FfiConverterTypeInviteInfo implements FfiConverterRustBuffer<InviteInfo> {
  INSTANCE;

  @Override
  public InviteInfo read(java.nio.ByteBuffer buf) {
    return new InviteInfo(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterBoolean.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(InviteInfo value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.address()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.publicKey()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.petname()) +
            FfiConverterBoolean.INSTANCE.allocationSize(value.signed())
      );
  }

  @Override
  public void write(InviteInfo value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.address(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.publicKey(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.petname(), buf);
      FfiConverterBoolean.INSTANCE.write(value.signed(), buf);
  }
}
