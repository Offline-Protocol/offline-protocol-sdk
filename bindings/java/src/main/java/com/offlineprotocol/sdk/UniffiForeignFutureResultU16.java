package com.offlineprotocol.sdk;


public final class UniffiForeignFutureResultU16 {
    public static final java.lang.foreign.StructLayout LAYOUT = java.lang.foreign.MemoryLayout.structLayout(
        java.lang.foreign.ValueLayout.JAVA_SHORT.withName("returnValue"),
        java.lang.foreign.MemoryLayout.paddingLayout(6),
        UniffiRustCallStatus.LAYOUT.withName("callStatus")
    );

    private static final long OFFSET_returnValue = LAYOUT.byteOffset(java.lang.foreign.MemoryLayout.PathElement.groupElement("returnValue"));

    private static final long OFFSET_callStatus = LAYOUT.byteOffset(java.lang.foreign.MemoryLayout.PathElement.groupElement("callStatus"));

    private UniffiForeignFutureResultU16() {}
    public static short getreturnValue(java.lang.foreign.MemorySegment seg) {
        return (short) seg.get(java.lang.foreign.ValueLayout.JAVA_SHORT_UNALIGNED, OFFSET_returnValue);
    }

    public static void setreturnValue(java.lang.foreign.MemorySegment seg, short value) {
        seg.set(java.lang.foreign.ValueLayout.JAVA_SHORT_UNALIGNED, OFFSET_returnValue, value);
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
