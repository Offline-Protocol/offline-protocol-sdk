package com.offlineprotocol.sdk;


public enum FfiConverterTypeOverflowPolicy implements FfiConverterRustBuffer<OverflowPolicy> {
    INSTANCE;

    @Override
    public OverflowPolicy read(java.nio.ByteBuffer buf) {
        try {
            return OverflowPolicy.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(OverflowPolicy value) {
        return 4L;
    }

    @Override
    public void write(OverflowPolicy value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


