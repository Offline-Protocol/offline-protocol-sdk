package com.offlineprotocol.sdk;


public enum FfiConverterTypeGatewayAddressDeclaration implements FfiConverterRustBuffer<GatewayAddressDeclaration> {
  INSTANCE;

  @Override
  public GatewayAddressDeclaration read(java.nio.ByteBuffer buf) {
    return new GatewayAddressDeclaration(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf),
      FfiConverterSequenceByte.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(GatewayAddressDeclaration value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.address()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.publicKey()) +
            FfiConverterSequenceByte.INSTANCE.allocationSize(value.signature())
      );
  }

  @Override
  public void write(GatewayAddressDeclaration value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.address(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.publicKey(), buf);
      FfiConverterSequenceByte.INSTANCE.write(value.signature(), buf);
  }
}
