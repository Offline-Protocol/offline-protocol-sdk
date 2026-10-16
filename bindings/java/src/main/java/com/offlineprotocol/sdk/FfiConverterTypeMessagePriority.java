package com.offlineprotocol.sdk;


public enum FfiConverterTypeMessagePriority implements FfiConverterRustBuffer<MessagePriority> {
    INSTANCE;

    @Override
    public MessagePriority read(java.nio.ByteBuffer buf) {
        try {
            return MessagePriority.values()[buf.getInt() - 1];
        } catch (java.lang.IndexOutOfBoundsException e) {
            throw new java.lang.RuntimeException("invalid enum value, something is very wrong!!", e);
        }
    }

    @Override
    public long allocationSize(MessagePriority value) {
        return 4L;
    }

    @Override
    public void write(MessagePriority value, java.nio.ByteBuffer buf) {
        buf.putInt(value.ordinal() + 1);
    }
}


