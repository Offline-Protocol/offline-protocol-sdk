package com.offlineprotocol.sdk;


public enum FfiConverterTypePresenceStatus implements FfiConverterRustBuffer<PresenceStatus> {
    INSTANCE;

    @Override
    public PresenceStatus read(java.nio.ByteBuffer buf) {
        try {
            return PresenceStatus.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(PresenceStatus value) {
        return 4L;
    }

    @Override
    public void write(PresenceStatus value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


