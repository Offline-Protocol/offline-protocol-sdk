package com.offlineprotocol.sdk;


public enum FfiConverterBoolean implements FfiConverter<java.lang.Boolean, java.lang.Byte> {
  INSTANCE;

  @Override
  public java.lang.Boolean lift(java.lang.Byte value) {
    return (int) value != 0;
  }

  @Override
  public java.lang.Boolean read(java.nio.ByteBuffer buf) {
    return lift(buf.get());
  }

  @Override
  public java.lang.Byte lower(java.lang.Boolean value) {
    return value ? (byte) 1 : (byte) 0;
  }

  @Override
  public long allocationSize(java.lang.Boolean value) {
    return 1L;
  }

  @Override
  public void write(java.lang.Boolean value, java.nio.ByteBuffer buf) {
    buf.put(lower(value));
  }
}