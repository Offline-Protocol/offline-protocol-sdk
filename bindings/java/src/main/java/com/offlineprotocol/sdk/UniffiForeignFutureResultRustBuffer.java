package com.offlineprotocol.sdk;


public final class UniffiForeignFutureResultRustBuffer {
    public static final java.lang.foreign.StructLayout LAYOUT = java.lang.foreign.MemoryLayout.structLayout(
        RustBuffer.LAYOUT.withName("returnValue"),
        UniffiRustCallStatus.LAYOUT.withName("callStatus")
    );

    private static final long OFFSET_returnValue = LAYOUT.byteOffset(java.lang.foreign.MemoryLayout.PathElement.groupElement("returnValue"));

    private static final long OFFSET_callStatus = LAYOUT.byteOffset(java.lang.foreign.MemoryLayout.PathElement.groupElement("callStatus"));

    private UniffiForeignFutureResultRustBuffer() {}
    public static java.lang.foreign.MemorySegment getreturnValue(java.lang.foreign.MemorySegment seg) {
        return seg.asSlice(OFFSET_returnValue, RustBuffer.LAYOUT.byteSize());
    }

    public static void setreturnValue(java.lang.foreign.MemorySegment seg, java.lang.foreign.MemorySegment value) {
        java.lang.foreign.MemorySegment.copy(value, 0, seg, OFFSET_returnValue, RustBuffer.LAYOUT.byteSize());
    }
    public static java.lang.foreign.MemorySegment getcallStatus(java.lang.foreign.MemorySegment seg) {
        return seg.asSlice(OFFSET_callStatus, UniffiRustCallStatus.LAYOUT.byteSize());
    }

    public static void setcallStatus(java.lang.foreign.MemorySegment seg, java.lang.foreign.MemorySegment value) {
        java.lang.foreign.MemorySegment.copy(value, 0, seg, OFFSET_callStatus, UniffiRustCallStatus.LAYOUT.byteSize());
    }

    /**
     * Allocate a new instance in the given arena.
     */
    public static java.lang.foreign.MemorySegment allocate(java.lang.foreign.SegmentAllocator allocator) {
        java.lang.foreign.MemorySegment seg = allocator.allocate(LAYOUT);
        seg.fill((byte) 0);
        return seg;
    }
}
