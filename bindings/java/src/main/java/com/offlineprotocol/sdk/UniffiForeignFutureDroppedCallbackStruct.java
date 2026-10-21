package com.offlineprotocol.sdk;


public final class UniffiForeignFutureDroppedCallbackStruct {
    public static final java.lang.foreign.StructLayout LAYOUT = java.lang.foreign.MemoryLayout.structLayout(
        java.lang.foreign.ValueLayout.JAVA_LONG.withName("handle"),
        java.lang.foreign.ValueLayout.ADDRESS.withName("free")
    );

    private static final long OFFSET_handle = LAYOUT.byteOffset(java.lang.foreign.MemoryLayout.PathElement.groupElement("handle"));

    private static final long OFFSET_free = LAYOUT.byteOffset(java.lang.foreign.MemoryLayout.PathElement.groupElement("free"));

    private UniffiForeignFutureDroppedCallbackStruct() {}
    public static long gethandle(java.lang.foreign.MemorySegment seg) {
        return (long) seg.get(java.lang.foreign.ValueLayout.JAVA_LONG_UNALIGNED, OFFSET_handle);
    }

    public static void sethandle(java.lang.foreign.MemorySegment seg, long value) {
        seg.set(java.lang.foreign.ValueLayout.JAVA_LONG_UNALIGNED, OFFSET_handle, value);
    }
    public static java.lang.foreign.MemorySegment getfree(java.lang.foreign.MemorySegment seg) {
        return (java.lang.foreign.MemorySegment) seg.get(java.lang.foreign.ValueLayout.ADDRESS_UNALIGNED, OFFSET_free);
    }

    public static void setfree(java.lang.foreign.MemorySegment seg, java.lang.foreign.MemorySegment value) {
        seg.set(java.lang.foreign.ValueLayout.ADDRESS_UNALIGNED, OFFSET_free, value);
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
