package com.offlineprotocol.sdk;


// Thread-local slab allocator for short-lived native memory segments.
//
// Several FFM code paths need to allocate small struct-sized segments (RustBuffer
// at 24 bytes, RustCallStatus at 32 bytes) that are consumed immediately and never
// referenced again.
//
// Alternatives considered:
//   - Arena.global(): zero overhead but leaks permanently. At high call rates this
//     adds up fast (e.g. 100k calls × 24-32 bytes = 2.4-3.2 MB never reclaimed).
//   - Arena.ofAuto() per call: correct but creates a new Arena + PhantomReference
//     per call, adding ~50-100ns of GC pressure to every call.
//   - Thread-local reusable segment: zero overhead but causes SIGABRT - the FFM
//     runtime retains internal references to allocator-provided segments, so reusing
//     the same segment across calls corrupts FFM's internal state.
//
// This slab approach: allocate a batch of slots from one Arena.ofAuto(), then hand
// out slices. Each call gets a unique slice (avoiding the FFM reuse crash). When the
// slab is exhausted, a new one is allocated and the old one becomes GC-eligible once
// all its slices are consumed (which is immediate - callers read struct fields before
// the next call). Amortized cost: one Arena + one native malloc per `slots` calls.
class UniffiSlabAllocator implements java.lang.foreign.SegmentAllocator {
    private final long slabBytes;
    private final long alignment;
    private final ThreadLocal<java.lang.foreign.MemorySegment> slab;
    private final ThreadLocal<long[]> offset;

    UniffiSlabAllocator(java.lang.foreign.MemoryLayout layout, long slots) {
        this.slabBytes = layout.byteSize() * slots;
        this.alignment = layout.byteAlignment();
        this.slab = ThreadLocal.withInitial(() ->
            java.lang.foreign.Arena.ofAuto().allocate(this.slabBytes, this.alignment)
        );
        this.offset = ThreadLocal.withInitial(() -> new long[]{0});
    }

    @Override
    public java.lang.foreign.MemorySegment allocate(long byteSize, long byteAlignment) {
        long[] off = this.offset.get();
        java.lang.foreign.MemorySegment s = this.slab.get();
        if (off[0] + byteSize > s.byteSize()) {
            s = java.lang.foreign.Arena.ofAuto().allocate(this.slabBytes, this.alignment);
            this.slab.set(s);
            off[0] = 0;
        }
        java.lang.foreign.MemorySegment result = s.asSlice(off[0], byteSize);
        off[0] += byteSize;
        return result;
    }
}

