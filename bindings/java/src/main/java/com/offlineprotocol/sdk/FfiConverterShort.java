package com.offlineprotocol.sdk;


public enum FfiConverterShort implements FfiConverter<java.lang.Short, java.lang.Short>{
  INSTANCE;

    @Override
    public java.lang.Short lift(java.lang.Short value) {
        return value;
    }

    @Override
    public java.lang.Short read(java.nio.ByteBuffer buf) {
        return buf.getShort();
    }

    @Override
    public java.lang.Short lower(java.lang.Short value) {
        return value;
    }

    @Override
    public long allocationSize(java.lang.Short value) {
        return 2L;
    }

    @Override
    public void write(java.lang.Short value, java.nio.ByteBuffer buf) {
        buf.putShort(value);
    }
}