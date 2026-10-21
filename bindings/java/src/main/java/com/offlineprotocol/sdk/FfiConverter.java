package com.offlineprotocol.sdk;


// The FfiConverter interface handles converter types to and from the FFI
//
// All implementing objects should be public to support external types.  When a
// type is external we need to import it's FfiConverter.
public interface FfiConverter<JavaType, FfiType> {
    // Convert an FFI type to a Java type
    JavaType lift(FfiType value);

    // Convert an Java type to an FFI type
    FfiType lower(JavaType value);

    // Read a Java type from a `ByteBuffer`
    JavaType read(java.nio.ByteBuffer buf);

    // Calculate bytes to allocate when creating a `RustBuffer`
    //
    // This must return at least as many bytes as the write() function will
    // write. It can return more bytes than needed, for example when writing
    // Strings we can't know the exact bytes needed until we the UTF-8
    // encoding, so we pessimistically allocate the largest size possible (3
    // bytes per codepoint).  Allocating extra bytes is not really a big deal
    // because the `RustBuffer` is short-lived.
    long allocationSize(JavaType value);

    // Write a Java type to a `ByteBuffer`
    void write(JavaType value, java.nio.ByteBuffer buf);

    // Lower a value into a `RustBuffer`
    //
    // This method lowers a value into a `RustBuffer` rather than the normal
    // FfiType.  It's used by the callback interface code.  Callback interface
    // returns are always serialized into a `RustBuffer` regardless of their
    // normal FFI type.
    default java.lang.foreign.MemorySegment lowerIntoRustBuffer(JavaType value) {
        java.lang.foreign.MemorySegment rbuf = RustBuffer.alloc(allocationSize(value));
        try {
            java.nio.ByteBuffer bbuf = RustBuffer.asWriteByteBuffer(rbuf);
            write(value, bbuf);
            RustBuffer.setLen(rbuf, (long) bbuf.position());
            return rbuf;
        } catch (java.lang.Throwable e) {
            RustBuffer.free(rbuf);
            throw e;
        }
    }

    // Lift a value from a `RustBuffer`.
    //
    // This here mostly because of the symmetry with `lowerIntoRustBuffer()`.
    // It's currently only used by the `FfiConverterRustBuffer` class below.
    default JavaType liftFromRustBuffer(java.lang.foreign.MemorySegment rbuf) {
        java.nio.ByteBuffer byteBuf = RustBuffer.asByteBuffer(rbuf);
        try {
           JavaType item = read(byteBuf);
           if (byteBuf.hasRemaining()) {
               throw new java.lang.RuntimeException("junk remaining in buffer after lifting, something is very wrong!!");
           }
           return item;
        } finally {
            RustBuffer.free(rbuf);
        }
    }
}

