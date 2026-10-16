package com.offlineprotocol.sdk;


// FfiConverter that uses `RustBuffer` as the FfiType
public interface FfiConverterRustBuffer<JavaType> extends FfiConverter<JavaType, java.lang.foreign.MemorySegment> {
    @Override
    default JavaType lift(java.lang.foreign.MemorySegment value) {
        return liftFromRustBuffer(value);
    }
    @Override
    default java.lang.foreign.MemorySegment lower(JavaType value) {
        return lowerIntoRustBuffer(value);
    }
}
