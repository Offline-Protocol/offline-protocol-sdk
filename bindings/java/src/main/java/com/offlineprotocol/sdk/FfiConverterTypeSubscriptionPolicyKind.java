package com.offlineprotocol.sdk;


public enum FfiConverterTypeSubscriptionPolicyKind implements FfiConverterRustBuffer<SubscriptionPolicyKind> {
    INSTANCE;

    @Override
    public SubscriptionPolicyKind read(java.nio.ByteBuffer buf) {
        try {
            return SubscriptionPolicyKind.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(SubscriptionPolicyKind value) {
        return 4L;
    }

    @Override
    public void write(SubscriptionPolicyKind value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


