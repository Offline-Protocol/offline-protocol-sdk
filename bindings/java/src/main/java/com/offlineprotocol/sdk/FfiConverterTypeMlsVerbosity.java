package com.offlineprotocol.sdk;


public enum FfiConverterTypeMlsVerbosity implements FfiConverterRustBuffer<MlsVerbosity> {
    INSTANCE;

    @Override
    public MlsVerbosity read(java.nio.ByteBuffer buf) {
        try {
            return MlsVerbosity.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(MlsVerbosity value) {
        return 4L;
    }

    @Override
    public void write(MlsVerbosity value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


