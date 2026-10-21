package com.offlineprotocol.sdk;


// Lowers `&[u8]` / `[ByRef] bytes` arguments, which Rust borrows for the duration of the call
// rather than taking ownership of a RustBuffer.
//
// Only `lower` is reachable: zero-copy bytes flow foreign -> Rust in argument position only, so a
// borrow can never be lifted or serialized. `FfiConverter` is implemented anyway so the compiler
// enforces the full set.
//
// The buffer must not be mutated by another thread while a call is in flight, since Rust is reading
// it directly.
public enum FfiConverterByRefBytes implements FfiConverter<java.nio.ByteBuffer, java.lang.foreign.MemorySegment> {
    INSTANCE;

    // The struct is read by Rust during the call, so each one needs its own slice; see
    // UniffiSlabAllocator.
    private static final UniffiSlabAllocator ALLOCATOR = new UniffiSlabAllocator(ForeignBytes.LAYOUT, 1024);

    @Override
    public java.lang.foreign.MemorySegment lower(java.nio.ByteBuffer value) {
        if (!value.isDirect()) {
            throw new java.lang.IllegalArgumentException(
                "UniFFI zero-copy &[u8] requires a direct ByteBuffer, so Rust can borrow it without "
                + "a copy. Convert with: ByteBuffer.allocateDirect(arr.length).put(arr).flip()");
        }
        java.lang.foreign.MemorySegment fb = ALLOCATOR.allocate(ForeignBytes.LAYOUT);
        int remaining = value.remaining();
        ForeignBytes.setLen(fb, remaining);
        // A zero-length direct buffer has no meaningful address; Rust reads (null, 0) as `&[]`.
        // Otherwise ofBuffer honours position/limit, so Rust sees exactly the remaining slice.
        ForeignBytes.setData(fb, remaining == 0
            ? java.lang.foreign.MemorySegment.NULL
            : java.lang.foreign.MemorySegment.ofBuffer(value));
        return fb;
    }

    @Override
    public java.nio.ByteBuffer lift(java.lang.foreign.MemorySegment value) {
        throw new java.lang.UnsupportedOperationException(
            "ByRef bytes cannot be lifted: zero-copy &[u8] only flows foreign to Rust");
    }

    @Override
    public java.nio.ByteBuffer read(java.nio.ByteBuffer buf) {
        throw new java.lang.UnsupportedOperationException(
            "ByRef bytes cannot be read from a buffer: zero-copy &[u8] is only supported in argument position");
    }

    @Override
    public void write(java.nio.ByteBuffer value, java.nio.ByteBuffer buf) {
        throw new java.lang.UnsupportedOperationException(
            "ByRef bytes cannot be written to a buffer: zero-copy &[u8] is only supported in argument position");
    }

    @Override
    public long allocationSize(java.nio.ByteBuffer value) {
        throw new java.lang.UnsupportedOperationException(
            "ByRef bytes have no RustBuffer allocation size: zero-copy &[u8] is only supported in argument position");
    }
}
