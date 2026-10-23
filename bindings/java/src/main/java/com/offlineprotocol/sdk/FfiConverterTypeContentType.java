package com.offlineprotocol.sdk;


public enum FfiConverterTypeContentType implements FfiConverterRustBuffer<ContentType> {
    INSTANCE;

    @Override
    public ContentType read(java.nio.ByteBuffer buf) {
        try {
            return ContentType.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(ContentType value) {
        return 4L;
    }

    @Override
    public void write(ContentType value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


