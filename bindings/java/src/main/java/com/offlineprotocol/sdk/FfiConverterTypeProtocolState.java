package com.offlineprotocol.sdk;


public enum FfiConverterTypeProtocolState implements FfiConverterRustBuffer<ProtocolState> {
    INSTANCE;

    @Override
    public ProtocolState read(java.nio.ByteBuffer buf) {
        try {
            return ProtocolState.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(ProtocolState value) {
        return 4L;
    }

    @Override
    public void write(ProtocolState value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


