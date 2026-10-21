package com.offlineprotocol.sdk;


public enum FfiConverterTypeTelemetryOs implements FfiConverterRustBuffer<TelemetryOs> {
    INSTANCE;

    @Override
    public TelemetryOs read(java.nio.ByteBuffer buf) {
        try {
            return TelemetryOs.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(TelemetryOs value) {
        return 4L;
    }

    @Override
    public void write(TelemetryOs value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


