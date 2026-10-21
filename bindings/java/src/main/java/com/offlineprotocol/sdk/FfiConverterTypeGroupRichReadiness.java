package com.offlineprotocol.sdk;


public enum FfiConverterTypeGroupRichReadiness implements FfiConverterRustBuffer<GroupRichReadiness> {
  INSTANCE;

  @Override
  public GroupRichReadiness read(java.nio.ByteBuffer buf) {
    return new GroupRichReadiness(
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterSequenceString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(GroupRichReadiness value) {
      return (
            FfiConverterBoolean.INSTANCE.allocationSize(value.ready()) +
            FfiConverterSequenceString.INSTANCE.allocationSize(value.unknownMembers())
      );
  }

  @Override
  public void write(GroupRichReadiness value, java.nio.ByteBuffer buf) {
      FfiConverterBoolean.INSTANCE.write(value.ready(), buf);
      FfiConverterSequenceString.INSTANCE.write(value.unknownMembers(), buf);
  }
}
