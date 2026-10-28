package com.offlineprotocol.sdk;


public enum FfiConverterTypeSubscriptionRulesFallback implements FfiConverterRustBuffer<SubscriptionRulesFallback> {
    INSTANCE;

    @Override
    public SubscriptionRulesFallback read(java.nio.ByteBuffer buf) {
        try {
            return SubscriptionRulesFallback.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(SubscriptionRulesFallback value) {
        return 4L;
    }

    @Override
    public void write(SubscriptionRulesFallback value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


