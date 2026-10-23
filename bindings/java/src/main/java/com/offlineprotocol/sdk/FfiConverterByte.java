package com.offlineprotocol.sdk;


public enum FfiConverterByte implements FfiConverter<java.lang.Byte, java.lang.Byte>{
  INSTANCE;

    @Override
    public java.lang.Byte lift(java.lang.Byte value) {
        return value;
    }

    @Override
    public java.lang.Byte read(java.nio.ByteBuffer buf) {
        return buf.get();
    }

    @Override
    public java.lang.Byte lower(java.lang.Byte value) {
        return value;
    }

    @Override
    public long allocationSize(java.lang.Byte value) {
        return 1L;
    }

    @Override
    public void write(java.lang.Byte value, java.nio.ByteBuffer buf) {
        buf.put(value);
    }
}