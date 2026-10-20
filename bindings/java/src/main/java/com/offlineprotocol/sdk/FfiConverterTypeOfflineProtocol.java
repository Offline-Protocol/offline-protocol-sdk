package com.offlineprotocol.sdk;


public enum FfiConverterTypeOfflineProtocol implements FfiConverter<OfflineProtocol, java.lang.Long> {
    INSTANCE;

    @Override
    public java.lang.Long lower(OfflineProtocol value) {
        return value.uniffiCloneHandle();
    }

    @Override
    public OfflineProtocol lift(java.lang.Long value) {
        return new OfflineProtocol(UniffiWithHandle.INSTANCE, value);
    }

    @Override
    public OfflineProtocol read(java.nio.ByteBuffer buf) {
        // The Rust code always writes handles as 8 bytes, and will
        // fail to compile if they don't fit.
        return lift(buf.getLong());
    }

    @Override
    public long allocationSize(OfflineProtocol value) {
        return 8L;
    }

    @Override
    public void write(OfflineProtocol value, java.nio.ByteBuffer buf) {
        // The Rust code always expects handles written as 8 bytes,
        // and will fail to compile if they don't fit.
        buf.putLong(lower(value));
    }
}
