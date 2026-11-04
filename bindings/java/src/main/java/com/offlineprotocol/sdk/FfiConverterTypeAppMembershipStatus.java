package com.offlineprotocol.sdk;


public enum FfiConverterTypeAppMembershipStatus implements FfiConverterRustBuffer<AppMembershipStatus> {
  INSTANCE;

  @Override
  public AppMembershipStatus read(java.nio.ByteBuffer buf) {
    return new AppMembershipStatus(
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterOptionalLong.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(AppMembershipStatus value) {
      return (
            FfiConverterBoolean.INSTANCE.allocationSize(value.configured()) +
            FfiConverterOptionalLong.INSTANCE.allocationSize(value.certificateNotAfter()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.visibleMembers())
      );
  }

  @Override
  public void write(AppMembershipStatus value, java.nio.ByteBuffer buf) {
      FfiConverterBoolean.INSTANCE.write(value.configured(), buf);
      FfiConverterOptionalLong.INSTANCE.write(value.certificateNotAfter(), buf);
      FfiConverterInteger.INSTANCE.write(value.visibleMembers(), buf);
  }
}
