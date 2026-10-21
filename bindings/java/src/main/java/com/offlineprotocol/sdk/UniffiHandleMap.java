package com.offlineprotocol.sdk;


// This is used pass an opaque 64-bit handle representing a foreign object to the Rust code.
// Handles are always odd numbers (LSB = 1) to distinguish them from Rust handles (which are even).
class UniffiHandleMap<T extends java.lang.Object> {
    private final java.util.concurrent.ConcurrentHashMap<java.lang.Long, T> map = new java.util.concurrent.ConcurrentHashMap<>();
    // Start at 1 and increment by 2 to always produce odd handles
    private static final long UNIFFI_HANDLEMAP_INITIAL = 1L;
    private static final long UNIFFI_HANDLEMAP_DELTA = 2L;
    private final java.util.concurrent.atomic.AtomicLong counter = new java.util.concurrent.atomic.AtomicLong(UNIFFI_HANDLEMAP_INITIAL);

    public int size() {
        return map.size();
    }

    // Insert a new object into the handle map and get a handle for it
    // Handles are always odd (LSB = 1) to distinguish from Rust handles
    public long insert(T obj) {
        long handle = counter.getAndAdd(UNIFFI_HANDLEMAP_DELTA);
        map.put(handle, obj);
        return handle;
    }

    // Get an object from the handle map
    public T get(long handle) {
        T obj = map.get(handle);
        if (obj == null) {
            throw new InternalException("UniffiHandleMap.get: Invalid handle");
        }
        return obj;
    }

    // Remove an entry from the handlemap and get the Java object back
    public T remove(long handle) {
        T obj = map.remove(handle);
        if (obj == null) {
            throw new InternalException("UniffiHandleMap: Invalid handle");
        }
        return obj;
    }

    // Clone a handle - create a new handle pointing to the same object
    public long clone(long handle) {
        T obj = get(handle);
        return insert(obj);
    }
}

// Contains loading, initialization code,
// and the FFI Function declarations using Java FFM (Foreign Function & Memory API).
