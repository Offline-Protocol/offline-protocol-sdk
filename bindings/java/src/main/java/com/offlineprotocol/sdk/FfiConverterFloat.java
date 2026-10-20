package com.offlineprotocol.sdk;


public enum FfiConverterFloat implements FfiConverter<java.lang.Float, java.lang.Float>{
  INSTANCE;

    @Override
    public java.lang.Float lift(java.lang.Float value) {
        return value;
    }

    @Override
    public java.lang.Float read(java.nio.ByteBuffer buf) {
        return buf.getFloat();
    }

    @Override
    public java.lang.Float lower(java.lang.Float value) {
        return value;
    }

    @Override
    public long allocationSize(java.lang.Float value) {
        return 4L;
    }

    @Override
    public void write(java.lang.Float value, java.nio.ByteBuffer buf) {
        buf.putFloat(value);
    }
}