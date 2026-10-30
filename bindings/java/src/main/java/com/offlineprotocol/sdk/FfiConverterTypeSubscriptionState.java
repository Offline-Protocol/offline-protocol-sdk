package com.offlineprotocol.sdk;


public enum FfiConverterTypeSubscriptionState implements FfiConverterRustBuffer<SubscriptionState> {
    INSTANCE;

    @Override
    public SubscriptionState read(java.nio.ByteBuffer buf) {
        try {
            return SubscriptionState.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(SubscriptionState value) {
        return 4L;
    }

    @Override
    public void write(SubscriptionState value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


