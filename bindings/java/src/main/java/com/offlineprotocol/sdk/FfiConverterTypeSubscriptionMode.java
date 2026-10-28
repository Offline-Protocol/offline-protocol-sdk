package com.offlineprotocol.sdk;


public enum FfiConverterTypeSubscriptionMode implements FfiConverterRustBuffer<SubscriptionMode> {
    INSTANCE;

    @Override
    public SubscriptionMode read(java.nio.ByteBuffer buf) {
        try {
            return SubscriptionMode.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(SubscriptionMode value) {
        return 4L;
    }

    @Override
    public void write(SubscriptionMode value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


