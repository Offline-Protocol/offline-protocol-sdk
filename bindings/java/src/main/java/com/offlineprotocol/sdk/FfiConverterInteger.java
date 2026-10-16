package com.offlineprotocol.sdk;


public enum FfiConverterInteger implements FfiConverter<java.lang.Integer, java.lang.Integer>{
  INSTANCE;

    @Override
    public java.lang.Integer lift(java.lang.Integer value) {
        return value;
    }

    @Override
    public java.lang.Integer read(java.nio.ByteBuffer buf) {
        return buf.getInt();
    }

    @Override
    public java.lang.Integer lower(java.lang.Integer value) {
        return value;
    }

    @Override
    public long allocationSize(java.lang.Integer value) {
        return 4L;
    }

    @Override
    public void write(java.lang.Integer value, java.nio.ByteBuffer buf) {
        buf.putInt(value);
    }
}