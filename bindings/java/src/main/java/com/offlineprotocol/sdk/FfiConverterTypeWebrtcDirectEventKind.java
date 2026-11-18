package com.offlineprotocol.sdk;


public enum FfiConverterTypeWebrtcDirectEventKind implements FfiConverterRustBuffer<WebrtcDirectEventKind> {
    INSTANCE;

    @Override
    public WebrtcDirectEventKind read(java.nio.ByteBuffer buf) {
        try {
            return WebrtcDirectEventKind.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(WebrtcDirectEventKind value) {
        return 4L;
    }

    @Override
    public void write(WebrtcDirectEventKind value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}



