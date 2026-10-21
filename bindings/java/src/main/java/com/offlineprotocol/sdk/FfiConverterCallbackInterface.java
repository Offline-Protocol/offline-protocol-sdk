package com.offlineprotocol.sdk;


public abstract class FfiConverterCallbackInterface<CallbackInterface extends java.lang.Object> implements FfiConverter<CallbackInterface, java.lang.Long> {
    // Magic number for the Rust proxy to call using the same mechanism as every other method,
    // to free the callback once it's dropped by Rust.
    static final int IDX_CALLBACK_FREE = 0;
    // Callback return codes
    static final int UNIFFI_CALLBACK_SUCCESS = 0;
    static final int UNIFFI_CALLBACK_ERROR = 1;
    static final int UNIFFI_CALLBACK_UNEXPECTED_ERROR = 2;

    public final UniffiHandleMap<CallbackInterface> handleMap = new UniffiHandleMap<>();

    void drop(long handle) {
        handleMap.remove(handle);
    }

    @Override
    public CallbackInterface lift(java.lang.Long value) {
        return handleMap.get(value);
    }

    @Override
    public CallbackInterface read(java.nio.ByteBuffer buf) {
      return lift(buf.getLong());
    }

    @Override
    public java.lang.Long lower(CallbackInterface value) {
      return handleMap.insert(value);
    }

    @Override
    public long allocationSize(CallbackInterface value) {
      return 8L;
    }

    @Override
    public void write(CallbackInterface value, java.nio.ByteBuffer buf) {
        buf.putLong(lower(value));
    }
}
