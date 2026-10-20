package com.offlineprotocol.sdk;


public enum FfiConverterTypeAppState implements FfiConverterRustBuffer<AppState> {
    INSTANCE;

    @Override
    public AppState read(java.nio.ByteBuffer buf) {
        try {
            return AppState.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(AppState value) {
        return 4L;
    }

    @Override
    public void write(AppState value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


