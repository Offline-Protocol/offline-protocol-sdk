package com.offlineprotocol.sdk;


public enum FfiConverterTypeRelayPriority implements FfiConverterRustBuffer<RelayPriority> {
    INSTANCE;

    @Override
    public RelayPriority read(java.nio.ByteBuffer buf) {
        try {
            return RelayPriority.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(RelayPriority value) {
        return 4L;
    }

    @Override
    public void write(RelayPriority value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


