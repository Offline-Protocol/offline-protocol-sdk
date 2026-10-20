package com.offlineprotocol.sdk;


public final class UniffiRustCallStatus {
    public static final java.lang.foreign.StructLayout LAYOUT = java.lang.foreign.MemoryLayout.structLayout(
        java.lang.foreign.ValueLayout.JAVA_BYTE.withName("code"),
        java.lang.foreign.MemoryLayout.paddingLayout(7),  // 7 bytes padding for alignment before RustBuffer
        RustBuffer.LAYOUT.withName("error_buf")
    );

    private static final long OFFSET_CODE = LAYOUT.byteOffset(java.lang.foreign.MemoryLayout.PathElement.groupElement("code"));
    private static final long OFFSET_ERROR_BUF = LAYOUT.byteOffset(java.lang.foreign.MemoryLayout.PathElement.groupElement("error_buf"));

    public static final byte UNIFFI_CALL_SUCCESS = 0;
    public static final byte UNIFFI_CALL_ERROR = 1;
    public static final byte UNIFFI_CALL_UNEXPECTED_ERROR = 2;

    private UniffiRustCallStatus() {}

    public static byte getCode(java.lang.foreign.MemorySegment seg) {
        return seg.get(java.lang.foreign.ValueLayout.JAVA_BYTE, OFFSET_CODE);
    }

    public static void setCode(java.lang.foreign.MemorySegment seg, byte value) {
        seg.set(java.lang.foreign.ValueLayout.JAVA_BYTE, OFFSET_CODE, value);
    }

    public static java.lang.foreign.MemorySegment getErrorBuf(java.lang.foreign.MemorySegment seg) {
        return seg.asSlice(OFFSET_ERROR_BUF, RustBuffer.LAYOUT.byteSize());
    }

    public static void setErrorBuf(java.lang.foreign.MemorySegment seg, java.lang.foreign.MemorySegment errorBuf) {
        java.lang.foreign.MemorySegment.copy(errorBuf, 0, seg, OFFSET_ERROR_BUF, RustBuffer.LAYOUT.byteSize());
    }

    public static boolean isSuccess(java.lang.foreign.MemorySegment seg) {
        return getCode(seg) == UNIFFI_CALL_SUCCESS;
    }

    public static boolean isError(java.lang.foreign.MemorySegment seg) {
        return getCode(seg) == UNIFFI_CALL_ERROR;
    }

    public static boolean isPanic(java.lang.foreign.MemorySegment seg) {
        return getCode(seg) == UNIFFI_CALL_UNEXPECTED_ERROR;
    }

    /**
     * Allocate a new RustCallStatus in the given arena.
     */
    public static java.lang.foreign.MemorySegment allocate(java.lang.foreign.SegmentAllocator allocator) {
        java.lang.foreign.MemorySegment seg = allocator.allocate(LAYOUT);
        seg.fill((byte) 0);
        return seg;
    }

    // Slab allocator for RustCallStatus segments used by async callback error handling.
    // See UniffiSlabAllocator for the design rationale.
    private static final UniffiSlabAllocator STATUS_ALLOCATOR = new UniffiSlabAllocator(LAYOUT, 1024);

    /**
     * Create a RustCallStatus with the given code and error buffer.
     * Used by async callback interface error handling.
     */
    public static java.lang.foreign.MemorySegment create(byte code, java.lang.foreign.MemorySegment errorBuf) {
        java.lang.foreign.MemorySegment seg = STATUS_ALLOCATOR.allocate(LAYOUT);
        seg.fill((byte) 0);
        setCode(seg, code);
        setErrorBuf(seg, errorBuf);
        return seg;
    }
}

