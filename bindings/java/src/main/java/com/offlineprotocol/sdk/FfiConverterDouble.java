package com.offlineprotocol.sdk;


public enum FfiConverterDouble implements FfiConverter<java.lang.Double, java.lang.Double>{
  INSTANCE;

    @Override
    public java.lang.Double lift(java.lang.Double value) {
        return value;
    }

    @Override
    public java.lang.Double read(java.nio.ByteBuffer buf) {
        return buf.getDouble();
    }

    @Override
    public java.lang.Double lower(java.lang.Double value) {
        return value;
    }

    @Override
    public long allocationSize(java.lang.Double value) {
        return 8L;
    }

    @Override
    public void write(java.lang.Double value, java.nio.ByteBuffer buf) {
        buf.putDouble(value);
    }
}