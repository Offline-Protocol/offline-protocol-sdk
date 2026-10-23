package com.offlineprotocol.sdk;


public enum FfiConverterTypeProtocolLockDiagnostics implements FfiConverterRustBuffer<ProtocolLockDiagnostics> {
  INSTANCE;

  @Override
  public ProtocolLockDiagnostics read(java.nio.ByteBuffer buf) {
    return new ProtocolLockDiagnostics(
      FfiConverterBoolean.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(ProtocolLockDiagnostics value) {
      return (
            FfiConverterBoolean.INSTANCE.allocationSize(value.held()) +
            FfiConverterString.INSTANCE.allocationSize(value.holderLocation()) +
            FfiConverterString.INSTANCE.allocationSize(value.holderThread()) +
            FfiConverterLong.INSTANCE.allocationSize(value.heldForMs())
      );
  }

  @Override
  public void write(ProtocolLockDiagnostics value, java.nio.ByteBuffer buf) {
      FfiConverterBoolean.INSTANCE.write(value.held(), buf);
      FfiConverterString.INSTANCE.write(value.holderLocation(), buf);
      FfiConverterString.INSTANCE.write(value.holderThread(), buf);
      FfiConverterLong.INSTANCE.write(value.heldForMs(), buf);
  }
}
