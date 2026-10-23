package com.offlineprotocol.sdk;


public enum FfiConverterTypeEstablishmentState implements FfiConverterRustBuffer<EstablishmentState> {
    INSTANCE;

    @Override
    public EstablishmentState read(java.nio.ByteBuffer buf) {
        try {
            return EstablishmentState.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(EstablishmentState value) {
        return 4L;
    }

    @Override
    public void write(EstablishmentState value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


