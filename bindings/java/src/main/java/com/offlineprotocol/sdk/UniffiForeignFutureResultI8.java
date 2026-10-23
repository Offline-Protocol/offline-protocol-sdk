package com.offlineprotocol.sdk;


public final class UniffiForeignFutureResultI8 {
    public static final java.lang.foreign.StructLayout LAYOUT = java.lang.foreign.MemoryLayout.structLayout(
        java.lang.foreign.ValueLayout.JAVA_BYTE.withName("returnValue"),
        java.lang.foreign.MemoryLayout.paddingLayout(7),
        UniffiRustCallStatus.LAYOUT.withName("callStatus")
    );

    private static final long OFFSET_returnValue = LAYOUT.byteOffset(java.lang.foreign.MemoryLayout.PathElement.groupElement("returnValue"));

    private static final long OFFSET_callStatus = LAYOUT.byteOffset(java.lang.foreign.MemoryLayout.PathElement.groupElement("callStatus"));

    private UniffiForeignFutureResultI8() {}
    public static byte getreturnValue(java.lang.foreign.MemorySegment seg) {
        return (byte) seg.get(java.lang.foreign.ValueLayout.JAVA_BYTE, OFFSET_returnValue);
    }

    public static void setreturnValue(java.lang.foreign.MemorySegment seg, byte value) {
        seg.set(java.lang.foreign.ValueLayout.JAVA_BYTE, OFFSET_returnValue, value);
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
