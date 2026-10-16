package com.offlineprotocol.sdk;


public enum FfiConverterLong implements FfiConverter<java.lang.Long, java.lang.Long> {
    INSTANCE;

    @Override
    public java.lang.Long lift(java.lang.Long value) {
        return value;
    }

    @Override
    public java.lang.Long read(java.nio.ByteBuffer buf) {
        return buf.getLong();
    }

    @Override
    public java.lang.Long lower(java.lang.Long value) {
        return value;
    }

    @Override
    public long allocationSize(java.lang.Long value) {
        return 8L;
    }

    @Override
    public void write(java.lang.Long value, java.nio.ByteBuffer buf) {
        buf.putLong(value);
    }
}
