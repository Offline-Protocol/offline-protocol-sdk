package com.offlineprotocol.sdk;


public enum FfiConverterTypeTransportType implements FfiConverterRustBuffer<TransportType> {
    INSTANCE;

    @Override
    public TransportType read(java.nio.ByteBuffer buf) {
        try {
            return TransportType.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(TransportType value) {
        return 4L;
    }

    @Override
    public void write(TransportType value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}



