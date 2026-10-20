package com.offlineprotocol.sdk;


// FFM-based library binding. Each FFI function gets a MethodHandle and a wrapper method.
final class UniffiLib {
    private static final java.lang.foreign.Linker LINKER = java.lang.foreign.Linker.nativeLinker();
    private static final java.lang.foreign.SymbolLookup SYMBOLS;

    
    // The Cleaner for the whole library
    static UniffiCleaner CLEANER;

    static {
        SYMBOLS = NamespaceLibrary.loadLibrary();
    }

    private static java.lang.invoke.MethodHandle findDowncallHandle(String name, java.lang.foreign.FunctionDescriptor descriptor) {
        return SYMBOLS.find(name)
            .map(s -> LINKER.downcallHandle(s, descriptor))
            .orElseThrow(() -> new RuntimeException("Missing FFI symbol: " + name));
    }

    // uniffi_offline_protocol_uniffi_fn_clone_datastore
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_clone_datastore = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_clone_datastore", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_clone_datastore(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_clone_datastore.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_free_datastore
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_free_datastore = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_free_datastore", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_free_datastore(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_free_datastore.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_constructor_datastore_new
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_constructor_datastore_new = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_constructor_datastore_new", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_constructor_datastore_new(long protocol, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_constructor_datastore_new.invokeExact(protocol, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_constructor_datastore_with_storage
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_constructor_datastore_with_storage = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_constructor_datastore_with_storage", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_constructor_datastore_with_storage(long protocol, long storage, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_constructor_datastore_with_storage.invokeExact(protocol, storage, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_attachment_hash
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_attachment_hash = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_attachment_hash", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_datastore_attachment_hash(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_datastore_attachment_hash.invokeExact(_allocator, ptr, data, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_counter_increment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_counter_increment = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_counter_increment", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_DOUBLE, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_counter_increment(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, double amount, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_counter_increment.invokeExact(ptr, spaceId, docId, collection, amount, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_counter_value
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_counter_value = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_counter_value", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_DOUBLE, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static double uniffi_offline_protocol_uniffi_fn_method_datastore_counter_value(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (double) MH_uniffi_offline_protocol_uniffi_fn_method_datastore_counter_value.invokeExact(ptr, spaceId, docId, collection, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_create_doc
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_create_doc = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_create_doc", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_create_doc(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_create_doc.invokeExact(ptr, spaceId, docId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_decline_attachment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_decline_attachment = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_decline_attachment", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_decline_attachment(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment hash, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_decline_attachment.invokeExact(ptr, spaceId, peerId, hash, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_delete_doc
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_delete_doc = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_delete_doc", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_delete_doc(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_delete_doc.invokeExact(ptr, spaceId, docId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_doc_json
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_doc_json = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_doc_json", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_datastore_doc_json(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_datastore_doc_json.invokeExact(_allocator, ptr, spaceId, docId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_doc_size
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_doc_size = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_doc_size", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_method_datastore_doc_size(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_method_datastore_doc_size.invokeExact(ptr, spaceId, docId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_export_raw
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_export_raw = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_export_raw", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_datastore_export_raw(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_datastore_export_raw.invokeExact(_allocator, ptr, spaceId, docId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment hash, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment.invokeExact(ptr, spaceId, hash, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment_from
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment_from = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment_from", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment_from(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment hash, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment_from.invokeExact(ptr, spaceId, peerId, hash, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_flush
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_flush = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_flush", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_flush(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_flush.invokeExact(ptr, spaceId, docId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_flush_all
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_flush_all = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_flush_all", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_flush_all(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_flush_all.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_list_delete
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_list_delete = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_list_delete", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_list_delete(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, int index, int count, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_list_delete.invokeExact(ptr, spaceId, docId, collection, index, count, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_list_docs
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_list_docs = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_list_docs", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_datastore_list_docs(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_datastore_list_docs.invokeExact(_allocator, ptr, spaceId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_list_len
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_list_len = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_list_len", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static int uniffi_offline_protocol_uniffi_fn_method_datastore_list_len(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (int) MH_uniffi_offline_protocol_uniffi_fn_method_datastore_list_len.invokeExact(ptr, spaceId, docId, collection, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_list_push
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_list_push = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_list_push", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_list_push(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, java.lang.foreign.MemorySegment valueJson, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_list_push.invokeExact(ptr, spaceId, docId, collection, valueJson, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_list_spaces
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_list_spaces = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_list_spaces", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_datastore_list_spaces(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_datastore_list_spaces.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_map_delete
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_map_delete = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_map_delete", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_map_delete(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, java.lang.foreign.MemorySegment key, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_map_delete.invokeExact(ptr, spaceId, docId, collection, key, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_map_get_json
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_map_get_json = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_map_get_json", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_datastore_map_get_json(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, java.lang.foreign.MemorySegment key, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_datastore_map_get_json.invokeExact(_allocator, ptr, spaceId, docId, collection, key, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_map_set
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_map_set = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_map_set", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_map_set(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, java.lang.foreign.MemorySegment key, java.lang.foreign.MemorySegment valueJson, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_map_set.invokeExact(ptr, spaceId, docId, collection, key, valueJson, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_provide_attachment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_provide_attachment = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_provide_attachment", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_provide_attachment(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment hash, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_provide_attachment.invokeExact(ptr, spaceId, peerId, hash, data, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_remove_doc
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_remove_doc = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_remove_doc", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_remove_doc(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_remove_doc.invokeExact(ptr, spaceId, docId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_remove_space
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_remove_space = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_remove_space", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_remove_space(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_remove_space.invokeExact(ptr, spaceId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_set_interest
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_set_interest = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_set_interest", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_set_interest(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment patterns, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_set_interest.invokeExact(ptr, spaceId, patterns, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_text_delete
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_text_delete = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_text_delete", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_text_delete(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, int position, int count, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_text_delete.invokeExact(ptr, spaceId, docId, collection, position, count, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_text_insert
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_text_insert = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_text_insert", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_INT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_text_insert(long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, int position, java.lang.foreign.MemorySegment text, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_text_insert.invokeExact(ptr, spaceId, docId, collection, position, text, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_text_value
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_text_value = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_text_value", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_datastore_text_value(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment spaceId, java.lang.foreign.MemorySegment docId, java.lang.foreign.MemorySegment collection, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_datastore_text_value.invokeExact(_allocator, ptr, spaceId, docId, collection, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_datastore_wipe_all
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_datastore_wipe_all = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_datastore_wipe_all", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_datastore_wipe_all(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_datastore_wipe_all.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_clone_meshservices
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_clone_meshservices = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_clone_meshservices", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_clone_meshservices(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_clone_meshservices.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_free_meshservices
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_free_meshservices = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_free_meshservices", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_free_meshservices(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_free_meshservices.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_constructor_meshservices_new
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_constructor_meshservices_new = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_constructor_meshservices_new", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_constructor_meshservices_new(long protocol, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_constructor_meshservices_new.invokeExact(protocol, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_meshservices_discover_services
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_meshservices_discover_services = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_meshservices_discover_services", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_meshservices_discover_services(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment serviceId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_meshservices_discover_services.invokeExact(_allocator, ptr, serviceId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_meshservices_register_service
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_meshservices_register_service = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_meshservices_register_service", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_meshservices_register_service(long ptr, java.lang.foreign.MemorySegment serviceId, java.lang.foreign.MemorySegment version, java.lang.foreign.MemorySegment capabilities, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_meshservices_register_service.invokeExact(ptr, serviceId, version, capabilities, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_meshservices_respond_to_service_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_meshservices_respond_to_service_request = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_meshservices_respond_to_service_request", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_meshservices_respond_to_service_request(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment requestId, java.lang.foreign.MemorySegment requester, java.lang.foreign.MemorySegment serviceId, java.lang.foreign.MemorySegment status, java.lang.foreign.MemorySegment body, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_meshservices_respond_to_service_request.invokeExact(_allocator, ptr, requestId, requester, serviceId, status, body, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_meshservices_send_service_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_meshservices_send_service_request = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_meshservices_send_service_request", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_meshservices_send_service_request(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment provider, java.lang.foreign.MemorySegment serviceId, java.lang.foreign.MemorySegment method, java.lang.foreign.MemorySegment body, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_meshservices_send_service_request.invokeExact(_allocator, ptr, provider, serviceId, method, body, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_meshservices_unregister_service
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_meshservices_unregister_service = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_meshservices_unregister_service", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_meshservices_unregister_service(long ptr, java.lang.foreign.MemorySegment serviceId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_meshservices_unregister_service.invokeExact(ptr, serviceId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_clone_offlineprotocol
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_clone_offlineprotocol = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_clone_offlineprotocol", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_clone_offlineprotocol(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_clone_offlineprotocol.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_free_offlineprotocol
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_free_offlineprotocol = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_free_offlineprotocol", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_free_offlineprotocol(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_free_offlineprotocol.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_constructor_offlineprotocol_new
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_constructor_offlineprotocol_new = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_constructor_offlineprotocol_new", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_constructor_offlineprotocol_new(java.lang.foreign.MemorySegment config, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_constructor_offlineprotocol_new.invokeExact(config, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_accept_connection_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_accept_connection_request = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_accept_connection_request", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_accept_connection_request(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment accepterName, java.lang.foreign.MemorySegment keyPackage, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_accept_connection_request.invokeExact(_allocator, ptr, recipient, accepterName, keyPackage, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_clear_peer_mtu
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_clear_peer_mtu = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_clear_peer_mtu", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_clear_peer_mtu(long ptr, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_clear_peer_mtu.invokeExact(ptr, peerId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_fallback_count
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_fallback_count = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_fallback_count", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_fallback_count(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_fallback_count.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_received = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_received", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_received(long ptr, java.lang.foreign.MemorySegment senderId, java.lang.foreign.MemorySegment fragment, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_received.invokeExact(ptr, senderId, fragment, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_next_fragment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_next_fragment = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_next_fragment", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_next_fragment(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_next_fragment.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_peer_count
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_peer_count = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_peer_count", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static int uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_peer_count(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (int) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_peer_count.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_discovered
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_discovered = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_discovered", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_SHORT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_discovered(long ptr, java.lang.foreign.MemorySegment peerId, short rssi, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_discovered.invokeExact(ptr, peerId, rssi, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_lost
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_lost = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_lost", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_lost(long ptr, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_lost.invokeExact(ptr, peerId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_recipient_not_among_peers_count
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_recipient_not_among_peers_count = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_recipient_not_among_peers_count", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_recipient_not_among_peers_count(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_recipient_not_among_peers_count.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_return_fragment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_return_fragment = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_return_fragment", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_return_fragment(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_return_fragment.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_set_peer_mtu
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_set_peer_mtu = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_set_peer_mtu", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_set_peer_mtu(long ptr, java.lang.foreign.MemorySegment peerId, int maxPayload, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_set_peer_mtu.invokeExact(ptr, peerId, maxPayload, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_status_changed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_status_changed = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_status_changed", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_status_changed(long ptr, byte isAvailable, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_status_changed.invokeExact(ptr, isAvailable, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_undersized_mtu_reports
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_undersized_mtu_reports = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_undersized_mtu_reports", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_undersized_mtu_reports(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_undersized_mtu_reports.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_block_user
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_block_user = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_block_user", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_block_user(long ptr, java.lang.foreign.MemorySegment userId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_block_user.invokeExact(ptr, userId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_connection_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_connection_request = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_connection_request", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_connection_request(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_connection_request.invokeExact(_allocator, ptr, recipient, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_file_transfer
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_file_transfer = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_file_transfer", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_file_transfer(long ptr, java.lang.foreign.MemorySegment fileId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_file_transfer.invokeExact(ptr, fileId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_close_file_stores
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_close_file_stores = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_close_file_stores", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_close_file_stores(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_close_file_stores.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_group = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_group", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_group(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment groupName, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_group.invokeExact(_allocator, ptr, groupName, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_invite
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_invite = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_invite", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_invite(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment petname, byte sign, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_invite.invokeExact(_allocator, ptr, petname, sign, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_derive_user_id_from_public_key
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_derive_user_id_from_public_key = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_derive_user_id_from_public_key", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_derive_user_id_from_public_key(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment publicKey, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_derive_user_id_from_public_key.invokeExact(_allocator, ptr, publicKey, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_disable_telemetry
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_disable_telemetry = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_disable_telemetry", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_disable_telemetry(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_disable_telemetry.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_emit_test_event
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_emit_test_event = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_emit_test_event", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_emit_test_event(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_emit_test_event.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_enable_telemetry
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_enable_telemetry = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_enable_telemetry", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_enable_telemetry(long ptr, java.lang.foreign.MemorySegment config, java.lang.foreign.MemorySegment appState, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_enable_telemetry.invokeExact(ptr, config, appState, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_end_telemetry_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_end_telemetry_session = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_end_telemetry_session", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_end_telemetry_session(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_end_telemetry_session.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_erase_custody
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_erase_custody = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_erase_custody", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_erase_custody(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_erase_custody.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_establish_secure_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_establish_secure_session = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_establish_secure_session", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_establish_secure_session(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_establish_secure_session.invokeExact(_allocator, ptr, peerId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_finalize_file
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_finalize_file = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_finalize_file", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_finalize_file(long ptr, java.lang.foreign.MemorySegment fileId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_finalize_file.invokeExact(ptr, fileId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry_blocking
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry_blocking = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry_blocking", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry_blocking(long ptr, long deadlineMs, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry_blocking.invokeExact(ptr, deadlineMs, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_force_transport
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_force_transport = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_force_transport", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_force_transport(long ptr, java.lang.foreign.MemorySegment transportType, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_force_transport.invokeExact(ptr, transportType, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment originalMessageJson, java.lang.foreign.MemorySegment newRecipient, java.lang.foreign.MemorySegment priority, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message.invokeExact(_allocator, ptr, originalMessageJson, newRecipient, priority, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message_to_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message_to_group = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message_to_group", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message_to_group(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment originalMessageJson, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment priority, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message_to_group.invokeExact(_allocator, ptr, originalMessageJson, groupId, priority, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_gateway_address_declaration
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_gateway_address_declaration = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_gateway_address_declaration", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_gateway_address_declaration(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment challenge, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_gateway_address_declaration.invokeExact(_allocator, ptr, challenge, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_active_transports
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_active_transports = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_active_transports", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_active_transports(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_active_transports.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_battery_level
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_battery_level = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_battery_level", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_battery_level(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_battery_level.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_blocked_users
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_blocked_users = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_blocked_users", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_blocked_users(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_blocked_users.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_custody_stats
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_custody_stats = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_custody_stats", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_custody_stats(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_custody_stats.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dedup_stats
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dedup_stats = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dedup_stats", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dedup_stats(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dedup_stats.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_delivery_success_rate
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_delivery_success_rate = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_delivery_success_rate", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_FLOAT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static float uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_delivery_success_rate(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (float) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_delivery_success_rate.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dors_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dors_config = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dors_config", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dors_config(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dors_config.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_establishment_state
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_establishment_state = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_establishment_state", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_establishment_state(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_establishment_state.invokeExact(_allocator, ptr, peerId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_file_progress
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_file_progress = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_file_progress", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_file_progress(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment fileId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_file_progress.invokeExact(_allocator, ptr, fileId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_info
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_info = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_info", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_info(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_info.invokeExact(_allocator, ptr, groupId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_roles
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_roles = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_roles", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_roles(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_roles.invokeExact(_allocator, ptr, groupId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_identity_public_key
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_identity_public_key = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_identity_public_key", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_identity_public_key(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_identity_public_key.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_is_charging
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_is_charging = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_is_charging", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_is_charging(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_is_charging.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_hops
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_hops = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_hops", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_hops(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_hops.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_latency
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_latency = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_latency", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_latency(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_latency.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_member_role
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_member_role = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_member_role", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_member_role(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment userId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_member_role.invokeExact(_allocator, ptr, groupId, userId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_stats
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_stats = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_stats", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_stats(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_stats.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_tunables
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_tunables = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_tunables", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_tunables(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_tunables.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_message_stats
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_message_stats = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_message_stats", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_message_stats(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_message_stats.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_pending_ack_count
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_pending_ack_count = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_pending_ack_count", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_pending_ack_count(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_pending_ack_count.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_config = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_config", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_config(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_config.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_priority
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_priority = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_priority", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_priority(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_priority.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_retry_queue_size
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_retry_queue_size = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_retry_queue_size", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_retry_queue_size(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_retry_queue_size.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_state
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_state = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_state", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_state(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_state.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_topology
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_topology = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_topology", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_topology(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_topology.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_transport_metrics
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_transport_metrics = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_transport_metrics", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_transport_metrics(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment transportType, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_transport_metrics.invokeExact(_allocator, ptr, transportType, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_relay_sync_state
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_relay_sync_state = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_relay_sync_state", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_relay_sync_state(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_relay_sync_state.invokeExact(_allocator, ptr, groupId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_rich_readiness
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_rich_readiness = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_rich_readiness", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_rich_readiness(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_rich_readiness.invokeExact(_allocator, ptr, groupId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_has_pending_key_package
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_has_pending_key_package = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_has_pending_key_package", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_has_pending_key_package(long ptr, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_has_pending_key_package.invokeExact(ptr, peerId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_identity_assertion
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_identity_assertion = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_identity_assertion", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_identity_assertion(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment signedData, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_identity_assertion.invokeExact(_allocator, ptr, signedData, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls(long ptr, long secureStorage, long protocolStateStorage, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls.invokeExact(ptr, secureStorage, protocolStateStorage, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls_with_file_stores
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls_with_file_stores = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls_with_file_stores", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls_with_file_stores(long ptr, java.lang.foreign.MemorySegment mlsRoot, java.lang.foreign.MemorySegment stateRoot, java.lang.foreign.MemorySegment storeKey, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls_with_file_stores.invokeExact(ptr, mlsRoot, stateRoot, storeKey, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declaration_refused
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declaration_refused = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declaration_refused", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declaration_refused(long ptr, java.lang.foreign.MemorySegment reason, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declaration_refused.invokeExact(ptr, reason, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declared
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declared = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declared", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declared(long ptr, java.lang.foreign.MemorySegment address, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declared.invokeExact(ptr, address, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_confirm_sent
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_confirm_sent = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_confirm_sent", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_confirm_sent(long ptr, java.lang.foreign.MemorySegment messageId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_confirm_sent.invokeExact(ptr, messageId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_get_next_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_get_next_message = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_get_next_message", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_get_next_message(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_get_next_message.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_group_report_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_group_report_received = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_group_report_received", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_group_report_received(long ptr, java.lang.foreign.MemorySegment reportJson, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_group_report_received.invokeExact(ptr, reportJson, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_message_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_message_received = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_message_received", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_message_received(long ptr, java.lang.foreign.MemorySegment senderId, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_message_received.invokeExact(ptr, senderId, data, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_peer_presence
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_peer_presence = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_peer_presence", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_BYTE, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_peer_presence(long ptr, java.lang.foreign.MemorySegment peerId, byte online, java.lang.foreign.MemorySegment lastSeenMs, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_peer_presence.invokeExact(ptr, peerId, online, lastSeenMs, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_presence_watchlist
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_presence_watchlist = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_presence_watchlist", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_presence_watchlist(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_presence_watchlist.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_relay_capabilities
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_relay_capabilities = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_relay_capabilities", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_relay_capabilities(long ptr, java.lang.foreign.MemorySegment capabilities, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_relay_capabilities.invokeExact(ptr, capabilities, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed(long ptr, java.lang.foreign.MemorySegment messageId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed.invokeExact(ptr, messageId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed_with_reason
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed_with_reason = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed_with_reason", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed_with_reason(long ptr, java.lang.foreign.MemorySegment messageId, java.lang.foreign.MemorySegment reason, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed_with_reason.invokeExact(ptr, messageId, reason, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_status_changed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_status_changed = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_status_changed", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_status_changed(long ptr, byte isConnected, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_status_changed.invokeExact(ptr, isConnected, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_invite_to_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_invite_to_group = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_invite_to_group", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_invite_to_group(long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment inviteeUserId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_invite_to_group.invokeExact(ptr, groupId, inviteeUserId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_mls_initialized
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_mls_initialized = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_mls_initialized", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_mls_initialized(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_mls_initialized.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_relay
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_relay = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_relay", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_relay(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_relay.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_user_blocked
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_user_blocked = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_user_blocked", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_user_blocked(long ptr, java.lang.foreign.MemorySegment userId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_user_blocked.invokeExact(ptr, userId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_leave_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_leave_group = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_leave_group", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_leave_group(long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_leave_group.invokeExact(ptr, groupId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_list_groups
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_list_groups = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_list_groups", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_list_groups(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_list_groups.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_local_address
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_local_address = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_local_address", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_local_address(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_local_address.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_clear_pending_welcome
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_clear_pending_welcome = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_clear_pending_welcome", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_clear_pending_welcome(long ptr, java.lang.foreign.MemorySegment otherUserId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_clear_pending_welcome.invokeExact(ptr, otherUserId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_create_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_create_session = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_create_session", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_create_session(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment otherUserId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_create_session.invokeExact(_allocator, ptr, otherUserId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment encrypted, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt.invokeExact(_allocator, ptr, encrypted, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt_from_user
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt_from_user = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt_from_user", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt_from_user(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment encrypted, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt_from_user.invokeExact(_allocator, ptr, encrypted, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_delete_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_delete_session = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_delete_session", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_delete_session(long ptr, java.lang.foreign.MemorySegment otherUserId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_delete_session.invokeExact(ptr, otherUserId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_encrypt_for_user
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_encrypt_for_user = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_encrypt_for_user", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_encrypt_for_user(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment otherUserId, java.lang.foreign.MemorySegment plaintext, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_encrypt_for_user.invokeExact(_allocator, ptr, otherUserId, plaintext, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_generate_key_package
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_generate_key_package = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_generate_key_package", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_generate_key_package(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_generate_key_package.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_or_create_key_package
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_or_create_key_package = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_or_create_key_package", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_or_create_key_package(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_or_create_key_package.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_key_packages
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_key_packages = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_key_packages", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_key_packages(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_key_packages.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_welcome
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_welcome = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_welcome", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_welcome(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment otherUserId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_welcome.invokeExact(_allocator, ptr, otherUserId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_has_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_has_session = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_has_session", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_has_session(long ptr, java.lang.foreign.MemorySegment otherUserId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_has_session.invokeExact(ptr, otherUserId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_import_key_package
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_import_key_package = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_import_key_package", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_import_key_package(long ptr, java.lang.foreign.MemorySegment userId, java.lang.foreign.MemorySegment keyPackageData, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_import_key_package.invokeExact(ptr, userId, keyPackageData, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_join_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_join_session = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_join_session", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_join_session(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment welcome, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_join_session.invokeExact(_allocator, ptr, welcome, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_list_sessions
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_list_sessions = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_list_sessions", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_list_sessions(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_list_sessions.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_mark_key_package_synced
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_mark_key_package_synced = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_mark_key_package_synced", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_mark_key_package_synced(long ptr, java.lang.foreign.MemorySegment packageId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_mark_key_package_synced.invokeExact(ptr, packageId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_process_welcome
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_process_welcome = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_process_welcome", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_process_welcome(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment welcome, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_process_welcome.invokeExact(_allocator, ptr, welcome, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_confirm_sent
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_confirm_sent = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_confirm_sent", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_confirm_sent(long ptr, java.lang.foreign.MemorySegment messageId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_confirm_sent.invokeExact(ptr, messageId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_message = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_message", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_message(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_message.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_query
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_query = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_query", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_query(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_query.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_public_key
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_public_key = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_public_key", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_public_key(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_public_key.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_subscription_filter
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_subscription_filter = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_subscription_filter", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_subscription_filter(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment subscriptionId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_subscription_filter.invokeExact(_allocator, ptr, subscriptionId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received(long ptr, java.lang.foreign.MemorySegment senderId, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received.invokeExact(ptr, senderId, data, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received_at
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received_at = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received_at", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received_at(long ptr, java.lang.foreign.MemorySegment senderId, java.lang.foreign.MemorySegment data, long createdAt, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received_at.invokeExact(ptr, senderId, data, createdAt, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_completed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_completed = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_completed", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_completed(long ptr, java.lang.foreign.MemorySegment queryId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_completed.invokeExact(ptr, queryId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_event_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_event_received = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_event_received", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_event_received(long ptr, java.lang.foreign.MemorySegment queryId, java.lang.foreign.MemorySegment eventJson, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_event_received.invokeExact(ptr, queryId, eventJson, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed(long ptr, java.lang.foreign.MemorySegment messageId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed.invokeExact(ptr, messageId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed_with_reason
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed_with_reason = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed_with_reason", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed_with_reason(long ptr, java.lang.foreign.MemorySegment messageId, java.lang.foreign.MemorySegment reason, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed_with_reason.invokeExact(ptr, messageId, reason, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_status_changed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_status_changed = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_status_changed", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_status_changed(long ptr, byte isConnected, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_status_changed.invokeExact(ptr, isConnected, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_notify_app_state
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_notify_app_state = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_notify_app_state", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_notify_app_state(long ptr, java.lang.foreign.MemorySegment state, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_notify_app_state.invokeExact(ptr, state, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_pause
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_pause = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_pause", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_pause(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_pause.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_poll_event
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_poll_event = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_poll_event", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_poll_event(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_poll_event.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process_file_chunk
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process_file_chunk = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process_file_chunk", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process_file_chunk(long ptr, java.lang.foreign.MemorySegment fileId, int chunkIndex, int totalChunks, long fileSize, java.lang.foreign.MemorySegment fileName, java.lang.foreign.MemorySegment fileChecksum, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process_file_chunk.invokeExact(ptr, fileId, chunkIndex, totalChunks, fileSize, fileName, fileChecksum, data, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_protocol_lock_diagnostics
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_protocol_lock_diagnostics = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_protocol_lock_diagnostics", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_protocol_lock_diagnostics(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_protocol_lock_diagnostics.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_receive_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_receive_message = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_receive_message", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_receive_message(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_receive_message.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reject_connection_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reject_connection_request = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reject_connection_request", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reject_connection_request(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reject_connection_request.invokeExact(_allocator, ptr, recipient, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rekey_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rekey_session = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rekey_session", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rekey_session(long ptr, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rekey_session.invokeExact(ptr, peerId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_release_transport_lock
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_release_transport_lock = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_release_transport_lock", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_release_transport_lock(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_release_transport_lock.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_from_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_from_group = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_from_group", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_from_group(long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment memberId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_from_group.invokeExact(ptr, groupId, memberId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_transport
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_transport = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_transport", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_transport(long ptr, java.lang.foreign.MemorySegment transportType, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_transport.invokeExact(ptr, transportType, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rename_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rename_group = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rename_group", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rename_group(long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment newName, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rename_group.invokeExact(ptr, groupId, newName, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_request_group_relay_registration
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_request_group_relay_registration = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_request_group_relay_registration", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_request_group_relay_registration(long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_request_group_relay_registration.invokeExact(ptr, groupId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resolve_username
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resolve_username = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resolve_username", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resolve_username(long ptr, java.lang.foreign.MemorySegment username, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resolve_username.invokeExact(ptr, username, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resume
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resume = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resume", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resume(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resume.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declaration_refused
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declaration_refused = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declaration_refused", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declaration_refused(long ptr, java.lang.foreign.MemorySegment reason, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declaration_refused.invokeExact(ptr, reason, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declared
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declared = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declared", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declared(long ptr, java.lang.foreign.MemorySegment address, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declared.invokeExact(ptr, address, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_confirm_sent
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_confirm_sent = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_confirm_sent", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_confirm_sent(long ptr, java.lang.foreign.MemorySegment messageId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_confirm_sent.invokeExact(ptr, messageId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_gateway_capabilities
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_gateway_capabilities = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_gateway_capabilities", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_gateway_capabilities(long ptr, java.lang.foreign.MemorySegment capabilities, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_gateway_capabilities.invokeExact(ptr, capabilities, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_get_next_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_get_next_message = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_get_next_message", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_get_next_message(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_get_next_message.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_message_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_message_received = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_message_received", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_message_received(long ptr, java.lang.foreign.MemorySegment senderId, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_message_received.invokeExact(ptr, senderId, data, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_peer_presence
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_peer_presence = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_peer_presence", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_BYTE, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_peer_presence(long ptr, java.lang.foreign.MemorySegment peerId, byte online, java.lang.foreign.MemorySegment lastSeenMs, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_peer_presence.invokeExact(ptr, peerId, online, lastSeenMs, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_presence_watchlist
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_presence_watchlist = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_presence_watchlist", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_presence_watchlist(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_presence_watchlist.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed(long ptr, java.lang.foreign.MemorySegment messageId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed.invokeExact(ptr, messageId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed_with_reason
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed_with_reason = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed_with_reason", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed_with_reason(long ptr, java.lang.foreign.MemorySegment messageId, java.lang.foreign.MemorySegment reason, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed_with_reason.invokeExact(ptr, messageId, reason, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_status_changed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_status_changed = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_status_changed", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_status_changed(long ptr, byte isConnected, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_status_changed.invokeExact(ptr, isConnected, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_connection_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_connection_request = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_connection_request", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_connection_request(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment senderName, java.lang.foreign.MemorySegment keyPackage, java.lang.foreign.MemorySegment initialMessage, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_connection_request.invokeExact(_allocator, ptr, recipient, senderName, keyPackage, initialMessage, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_file
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_file = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_file", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_file(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment fileData, java.lang.foreign.MemorySegment fileName, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_file.invokeExact(_allocator, ptr, recipient, fileData, fileName, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_group_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_group_message = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_group_message", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_group_message(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment content, java.lang.foreign.MemorySegment priority, java.lang.foreign.MemorySegment replyToMsg, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_group_message.invokeExact(_allocator, ptr, groupId, content, priority, replyToMsg, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment fileData, java.lang.foreign.MemorySegment fileName, java.lang.foreign.MemorySegment contentType, java.lang.foreign.MemorySegment mediaMetadata, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media.invokeExact(_allocator, ptr, recipient, fileData, fileName, contentType, mediaMetadata, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media_rich
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media_rich = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media_rich", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media_rich(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment fileData, java.lang.foreign.MemorySegment fileName, java.lang.foreign.MemorySegment contentType, java.lang.foreign.MemorySegment options, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media_rich.invokeExact(_allocator, ptr, recipient, fileData, fileName, contentType, options, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment content, java.lang.foreign.MemorySegment priority, java.lang.foreign.MemorySegment replyToMsg, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message.invokeExact(_allocator, ptr, recipient, content, priority, replyToMsg, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message_rich
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message_rich = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message_rich", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message_rich(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment content, java.lang.foreign.MemorySegment options, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message_rich.invokeExact(_allocator, ptr, recipient, content, options, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_presence_update
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_presence_update = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_presence_update", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_presence_update(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment status, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_presence_update.invokeExact(_allocator, ptr, recipient, status, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_read_receipt
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_read_receipt = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_read_receipt", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_read_receipt(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment messageIds, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_read_receipt.invokeExact(_allocator, ptr, recipient, messageIds, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_typing_indicator
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_typing_indicator = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_typing_indicator", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_typing_indicator(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment recipient, java.lang.foreign.MemorySegment conversationId, byte isTyping, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_typing_indicator.invokeExact(_allocator, ptr, recipient, conversationId, isTyping, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_level
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_level = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_level", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_level(long ptr, byte level, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_level.invokeExact(ptr, level, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_state
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_state = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_state", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_state(long ptr, byte level, byte isCharging, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_state.invokeExact(ptr, level, isCharging, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_ble_transport_callback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_ble_transport_callback = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_ble_transport_callback", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_ble_transport_callback(long ptr, long callback, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_ble_transport_callback.invokeExact(ptr, callback, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_event_callback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_event_callback = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_event_callback", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_event_callback(long ptr, long callback, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_event_callback.invokeExact(ptr, callback, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_member_role
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_member_role = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_member_role", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_member_role(long ptr, java.lang.foreign.MemorySegment groupId, java.lang.foreign.MemorySegment userId, java.lang.foreign.MemorySegment role, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_member_role.invokeExact(ptr, groupId, userId, role, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_nostr_transport_callback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_nostr_transport_callback = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_nostr_transport_callback", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_nostr_transport_callback(long ptr, long callback, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_nostr_transport_callback.invokeExact(ptr, callback, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_relay_priority
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_relay_priority = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_relay_priority", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_relay_priority(long ptr, java.lang.foreign.MemorySegment priority, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_relay_priority.invokeExact(ptr, priority, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_reticulum_transport_callback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_reticulum_transport_callback = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_reticulum_transport_callback", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_reticulum_transport_callback(long ptr, long callback, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_reticulum_transport_callback.invokeExact(ptr, callback, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_telemetry_enabled
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_telemetry_enabled = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_telemetry_enabled", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_telemetry_enabled(long ptr, byte enabled, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_telemetry_enabled.invokeExact(ptr, enabled, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_wifi_direct_transport_callback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_wifi_direct_transport_callback = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_wifi_direct_transport_callback", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_wifi_direct_transport_callback(long ptr, long callback, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_wifi_direct_transport_callback.invokeExact(ptr, callback, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_should_escalate_to_wifi
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_should_escalate_to_wifi = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_should_escalate_to_wifi", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_should_escalate_to_wifi(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_should_escalate_to_wifi.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data.invokeExact(_allocator, ptr, data, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data_bytes
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data_bytes = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data_bytes", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data_bytes(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data_bytes.invokeExact(_allocator, ptr, data, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_start
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_start = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_start", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_start(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_start.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_stop
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_stop = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_stop", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_stop(long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_stop.invokeExact(ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_install_id
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_install_id = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_install_id", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_install_id(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_install_id.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_stats
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_stats = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_stats", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_stats(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_stats.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_unblock_user
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_unblock_user = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_unblock_user", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_unblock_user(long ptr, java.lang.foreign.MemorySegment userId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_unblock_user.invokeExact(ptr, userId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_ack_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_ack_config = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_ack_config", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_ack_config(long ptr, java.lang.foreign.MemorySegment config, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_ack_config.invokeExact(ptr, config, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dedup_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dedup_config = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dedup_config", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dedup_config(long ptr, java.lang.foreign.MemorySegment config, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dedup_config.invokeExact(ptr, config, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dors_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dors_config = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dors_config", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dors_config(long ptr, java.lang.foreign.MemorySegment config, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dors_config.invokeExact(ptr, config, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_relay_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_relay_config = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_relay_config", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_relay_config(long ptr, java.lang.foreign.MemorySegment config, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_relay_config.invokeExact(ptr, config, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_retry_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_retry_config = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_retry_config", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_retry_config(long ptr, java.lang.foreign.MemorySegment config, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_retry_config.invokeExact(ptr, config, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_transport_metrics
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_transport_metrics = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_transport_metrics", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_transport_metrics(long ptr, java.lang.foreign.MemorySegment transportType, java.lang.foreign.MemorySegment metrics, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_transport_metrics.invokeExact(ptr, transportType, metrics, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature(long ptr, java.lang.foreign.MemorySegment publicKey, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment signature, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature.invokeExact(ptr, publicKey, data, signature, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature_bytes
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature_bytes = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature_bytes", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static byte uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature_bytes(long ptr, java.lang.foreign.MemorySegment publicKey, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment signature, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature_bytes.invokeExact(ptr, publicKey, data, signature, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_get_next_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_get_next_message = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_get_next_message", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_get_next_message(java.lang.foreign.SegmentAllocator _allocator, long ptr, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_get_next_message.invokeExact(_allocator, ptr, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_message_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_message_received = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_message_received", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_message_received(long ptr, java.lang.foreign.MemorySegment senderId, java.lang.foreign.MemorySegment data, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_message_received.invokeExact(ptr, senderId, data, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_connected
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_connected = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_connected", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_connected(long ptr, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_connected.invokeExact(ptr, peerId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_disconnected
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_disconnected = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_disconnected", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_disconnected(long ptr, java.lang.foreign.MemorySegment peerId, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_disconnected.invokeExact(ptr, peerId, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_status_changed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_status_changed = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_status_changed", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_status_changed(long ptr, byte isConnected, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_status_changed.invokeExact(ptr, isConnected, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_init_callback_vtable_bletransportcallback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_bletransportcallback = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_init_callback_vtable_bletransportcallback", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_init_callback_vtable_bletransportcallback(java.lang.foreign.MemorySegment vtable) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_bletransportcallback.invokeExact(vtable);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_init_callback_vtable_eventcallback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_eventcallback = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_init_callback_vtable_eventcallback", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_init_callback_vtable_eventcallback(java.lang.foreign.MemorySegment vtable) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_eventcallback.invokeExact(vtable);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_init_callback_vtable_mlsstorageprovider
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_mlsstorageprovider = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_init_callback_vtable_mlsstorageprovider", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_init_callback_vtable_mlsstorageprovider(java.lang.foreign.MemorySegment vtable) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_mlsstorageprovider.invokeExact(vtable);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_init_callback_vtable_nostrtransportcallback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_nostrtransportcallback = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_init_callback_vtable_nostrtransportcallback", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_init_callback_vtable_nostrtransportcallback(java.lang.foreign.MemorySegment vtable) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_nostrtransportcallback.invokeExact(vtable);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_init_callback_vtable_protocolstatestorageprovider
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_protocolstatestorageprovider = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_init_callback_vtable_protocolstatestorageprovider", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_init_callback_vtable_protocolstatestorageprovider(java.lang.foreign.MemorySegment vtable) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_protocolstatestorageprovider.invokeExact(vtable);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_init_callback_vtable_reticulumtransportcallback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_reticulumtransportcallback = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_init_callback_vtable_reticulumtransportcallback", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_init_callback_vtable_reticulumtransportcallback(java.lang.foreign.MemorySegment vtable) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_reticulumtransportcallback.invokeExact(vtable);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_init_callback_vtable_wifidirecttransportcallback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_wifidirecttransportcallback = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_init_callback_vtable_wifidirecttransportcallback", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.ADDRESS));

    static void uniffi_offline_protocol_uniffi_fn_init_callback_vtable_wifidirecttransportcallback(java.lang.foreign.MemorySegment vtable) {
        try {
            MH_uniffi_offline_protocol_uniffi_fn_init_callback_vtable_wifidirecttransportcallback.invokeExact(vtable);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_func_derive_address
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_func_derive_address = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_func_derive_address", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_func_derive_address(java.lang.foreign.SegmentAllocator _allocator, java.lang.foreign.MemorySegment publicKey, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_func_derive_address.invokeExact(_allocator, publicKey, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_func_parse_invite
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_func_parse_invite = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_func_parse_invite", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_func_parse_invite(java.lang.foreign.SegmentAllocator _allocator, java.lang.foreign.MemorySegment blob, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_func_parse_invite.invokeExact(_allocator, blob, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_func_run_storage_conformance
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_func_run_storage_conformance = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_func_run_storage_conformance", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_func_run_storage_conformance(java.lang.foreign.SegmentAllocator _allocator, long storage, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_func_run_storage_conformance.invokeExact(_allocator, storage, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_fn_func_verify_identity_assertion
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_fn_func_verify_identity_assertion = findDowncallHandle("uniffi_offline_protocol_uniffi_fn_func_verify_identity_assertion", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment uniffi_offline_protocol_uniffi_fn_func_verify_identity_assertion(java.lang.foreign.SegmentAllocator _allocator, java.lang.foreign.MemorySegment assertion, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_uniffi_offline_protocol_uniffi_fn_func_verify_identity_assertion.invokeExact(_allocator, assertion, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rustbuffer_alloc
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rustbuffer_alloc = findDowncallHandle("ffi_offline_protocol_uniffi_rustbuffer_alloc", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment ffi_offline_protocol_uniffi_rustbuffer_alloc(java.lang.foreign.SegmentAllocator _allocator, long size, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_ffi_offline_protocol_uniffi_rustbuffer_alloc.invokeExact(_allocator, size, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rustbuffer_from_bytes
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rustbuffer_from_bytes = findDowncallHandle("ffi_offline_protocol_uniffi_rustbuffer_from_bytes", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, ForeignBytes.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment ffi_offline_protocol_uniffi_rustbuffer_from_bytes(java.lang.foreign.SegmentAllocator _allocator, java.lang.foreign.MemorySegment bytes, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_ffi_offline_protocol_uniffi_rustbuffer_from_bytes.invokeExact(_allocator, bytes, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rustbuffer_free
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rustbuffer_free = findDowncallHandle("ffi_offline_protocol_uniffi_rustbuffer_free", java.lang.foreign.FunctionDescriptor.ofVoid(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.ADDRESS));

    static void ffi_offline_protocol_uniffi_rustbuffer_free(java.lang.foreign.MemorySegment buf, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_ffi_offline_protocol_uniffi_rustbuffer_free.invokeExact(buf, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rustbuffer_reserve
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rustbuffer_reserve = findDowncallHandle("ffi_offline_protocol_uniffi_rustbuffer_reserve", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment ffi_offline_protocol_uniffi_rustbuffer_reserve(java.lang.foreign.SegmentAllocator _allocator, java.lang.foreign.MemorySegment buf, long additional, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_ffi_offline_protocol_uniffi_rustbuffer_reserve.invokeExact(_allocator, buf, additional, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_u8
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_u8 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_u8", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_u8(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_u8.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_u8
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_u8 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_u8", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_u8(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_u8.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_u8
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_u8 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_u8", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_u8(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_u8.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_u8
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_u8 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_u8", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static byte ffi_offline_protocol_uniffi_rust_future_complete_u8(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_ffi_offline_protocol_uniffi_rust_future_complete_u8.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_i8
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_i8 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_i8", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_i8(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_i8.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_i8
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_i8 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_i8", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_i8(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_i8.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_i8
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_i8 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_i8", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_i8(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_i8.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_i8
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_i8 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_i8", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_BYTE, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static byte ffi_offline_protocol_uniffi_rust_future_complete_i8(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (byte) MH_ffi_offline_protocol_uniffi_rust_future_complete_i8.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_u16
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_u16 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_u16", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_u16(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_u16.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_u16
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_u16 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_u16", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_u16(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_u16.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_u16
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_u16 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_u16", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_u16(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_u16.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_u16
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_u16 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_u16", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static short ffi_offline_protocol_uniffi_rust_future_complete_u16(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (short) MH_ffi_offline_protocol_uniffi_rust_future_complete_u16.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_i16
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_i16 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_i16", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_i16(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_i16.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_i16
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_i16 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_i16", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_i16(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_i16.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_i16
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_i16 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_i16", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_i16(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_i16.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_i16
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_i16 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_i16", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static short ffi_offline_protocol_uniffi_rust_future_complete_i16(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (short) MH_ffi_offline_protocol_uniffi_rust_future_complete_i16.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_u32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_u32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_u32", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_u32(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_u32.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_u32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_u32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_u32", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_u32(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_u32.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_u32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_u32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_u32", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_u32(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_u32.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_u32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_u32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_u32", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static int ffi_offline_protocol_uniffi_rust_future_complete_u32(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (int) MH_ffi_offline_protocol_uniffi_rust_future_complete_u32.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_i32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_i32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_i32", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_i32(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_i32.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_i32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_i32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_i32", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_i32(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_i32.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_i32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_i32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_i32", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_i32(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_i32.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_i32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_i32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_i32", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_INT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static int ffi_offline_protocol_uniffi_rust_future_complete_i32(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (int) MH_ffi_offline_protocol_uniffi_rust_future_complete_i32.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_u64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_u64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_u64", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_u64(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_u64.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_u64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_u64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_u64", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_u64(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_u64.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_u64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_u64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_u64", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_u64(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_u64.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_u64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_u64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_u64", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long ffi_offline_protocol_uniffi_rust_future_complete_u64(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_ffi_offline_protocol_uniffi_rust_future_complete_u64.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_i64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_i64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_i64", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_i64(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_i64.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_i64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_i64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_i64", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_i64(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_i64.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_i64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_i64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_i64", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_i64(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_i64.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_i64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_i64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_i64", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static long ffi_offline_protocol_uniffi_rust_future_complete_i64(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (long) MH_ffi_offline_protocol_uniffi_rust_future_complete_i64.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_f32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_f32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_f32", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_f32(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_f32.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_f32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_f32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_f32", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_f32(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_f32.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_f32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_f32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_f32", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_f32(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_f32.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_f32
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_f32 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_f32", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_FLOAT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static float ffi_offline_protocol_uniffi_rust_future_complete_f32(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (float) MH_ffi_offline_protocol_uniffi_rust_future_complete_f32.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_f64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_f64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_f64", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_f64(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_f64.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_f64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_f64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_f64", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_f64(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_f64.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_f64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_f64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_f64", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_f64(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_f64.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_f64
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_f64 = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_f64", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_DOUBLE, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static double ffi_offline_protocol_uniffi_rust_future_complete_f64(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (double) MH_ffi_offline_protocol_uniffi_rust_future_complete_f64.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_rust_buffer
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_rust_buffer = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_rust_buffer", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_rust_buffer(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_rust_buffer.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_rust_buffer
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_rust_buffer = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_rust_buffer", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_rust_buffer(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_rust_buffer.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_rust_buffer
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_rust_buffer = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_rust_buffer", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_rust_buffer(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_rust_buffer.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_rust_buffer
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_rust_buffer = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_rust_buffer", java.lang.foreign.FunctionDescriptor.of(RustBuffer.LAYOUT, java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static java.lang.foreign.MemorySegment ffi_offline_protocol_uniffi_rust_future_complete_rust_buffer(java.lang.foreign.SegmentAllocator _allocator, long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            return (java.lang.foreign.MemorySegment) MH_ffi_offline_protocol_uniffi_rust_future_complete_rust_buffer.invokeExact(_allocator, handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_poll_void
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_poll_void = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_poll_void", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS, java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_poll_void(long handle, java.lang.foreign.MemorySegment callback, long callbackData) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_poll_void.invokeExact(handle, callback, callbackData);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_cancel_void
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_cancel_void = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_cancel_void", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_cancel_void(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_cancel_void.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_free_void
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_free_void = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_free_void", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG));

    static void ffi_offline_protocol_uniffi_rust_future_free_void(long handle) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_free_void.invokeExact(handle);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_rust_future_complete_void
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_rust_future_complete_void = findDowncallHandle("ffi_offline_protocol_uniffi_rust_future_complete_void", java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.ADDRESS));

    static void ffi_offline_protocol_uniffi_rust_future_complete_void(long handle, java.lang.foreign.MemorySegment _uniffiOutErr) {
        try {
            MH_ffi_offline_protocol_uniffi_rust_future_complete_void.invokeExact(handle, _uniffiOutErr);
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_func_derive_address
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_func_derive_address = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_func_derive_address", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_func_derive_address() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_func_derive_address.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_func_parse_invite
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_func_parse_invite = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_func_parse_invite", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_func_parse_invite() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_func_parse_invite.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_func_run_storage_conformance
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_func_run_storage_conformance = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_func_run_storage_conformance", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_func_run_storage_conformance() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_func_run_storage_conformance.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_func_verify_identity_assertion
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_func_verify_identity_assertion = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_func_verify_identity_assertion", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_func_verify_identity_assertion() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_func_verify_identity_assertion.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_attachment_hash
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_attachment_hash = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_attachment_hash", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_attachment_hash() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_attachment_hash.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_increment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_increment = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_increment", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_increment() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_increment.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_value
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_value = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_value", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_value() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_value.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_create_doc
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_create_doc = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_create_doc", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_create_doc() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_create_doc.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_decline_attachment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_decline_attachment = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_decline_attachment", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_decline_attachment() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_decline_attachment.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_delete_doc
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_delete_doc = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_delete_doc", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_delete_doc() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_delete_doc.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_json
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_json = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_json", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_json() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_json.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_size
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_size = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_size", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_size() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_size.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_export_raw
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_export_raw = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_export_raw", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_export_raw() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_export_raw.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment_from
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment_from = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment_from", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment_from() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment_from.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_flush
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_flush = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_flush", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_flush() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_flush.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_flush_all
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_flush_all = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_flush_all", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_flush_all() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_flush_all.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_list_delete
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_list_delete = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_list_delete", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_list_delete() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_list_delete.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_list_docs
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_list_docs = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_list_docs", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_list_docs() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_list_docs.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_list_len
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_list_len = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_list_len", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_list_len() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_list_len.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_list_push
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_list_push = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_list_push", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_list_push() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_list_push.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_list_spaces
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_list_spaces = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_list_spaces", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_list_spaces() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_list_spaces.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_map_delete
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_map_delete = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_map_delete", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_map_delete() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_map_delete.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_map_get_json
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_map_get_json = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_map_get_json", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_map_get_json() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_map_get_json.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_map_set
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_map_set = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_map_set", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_map_set() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_map_set.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_provide_attachment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_provide_attachment = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_provide_attachment", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_provide_attachment() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_provide_attachment.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_doc
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_doc = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_doc", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_doc() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_doc.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_space
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_space = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_space", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_space() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_space.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_set_interest
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_set_interest = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_set_interest", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_set_interest() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_set_interest.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_text_delete
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_text_delete = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_text_delete", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_text_delete() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_text_delete.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_text_insert
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_text_insert = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_text_insert", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_text_insert() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_text_insert.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_text_value
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_text_value = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_text_value", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_text_value() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_text_value.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_datastore_wipe_all
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_wipe_all = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_datastore_wipe_all", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_datastore_wipe_all() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_datastore_wipe_all.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_meshservices_discover_services
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_meshservices_discover_services = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_meshservices_discover_services", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_meshservices_discover_services() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_meshservices_discover_services.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_meshservices_register_service
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_meshservices_register_service = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_meshservices_register_service", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_meshservices_register_service() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_meshservices_register_service.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_meshservices_respond_to_service_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_meshservices_respond_to_service_request = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_meshservices_respond_to_service_request", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_meshservices_respond_to_service_request() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_meshservices_respond_to_service_request.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_meshservices_send_service_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_meshservices_send_service_request = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_meshservices_send_service_request", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_meshservices_send_service_request() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_meshservices_send_service_request.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_meshservices_unregister_service
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_meshservices_unregister_service = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_meshservices_unregister_service", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_meshservices_unregister_service() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_meshservices_unregister_service.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_accept_connection_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_accept_connection_request = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_accept_connection_request", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_accept_connection_request() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_accept_connection_request.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_clear_peer_mtu
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_clear_peer_mtu = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_clear_peer_mtu", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_clear_peer_mtu() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_clear_peer_mtu.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_fallback_count
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_fallback_count = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_fallback_count", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_fallback_count() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_fallback_count.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_received = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_received", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_received() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_received.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_next_fragment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_next_fragment = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_next_fragment", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_next_fragment() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_next_fragment.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_peer_count
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_peer_count = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_peer_count", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_peer_count() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_peer_count.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_discovered
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_discovered = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_discovered", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_discovered() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_discovered.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_lost
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_lost = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_lost", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_lost() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_lost.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_recipient_not_among_peers_count
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_recipient_not_among_peers_count = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_recipient_not_among_peers_count", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_recipient_not_among_peers_count() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_recipient_not_among_peers_count.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_return_fragment
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_return_fragment = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_return_fragment", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_return_fragment() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_return_fragment.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_set_peer_mtu
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_set_peer_mtu = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_set_peer_mtu", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_set_peer_mtu() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_set_peer_mtu.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_status_changed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_status_changed = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_status_changed", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_status_changed() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_status_changed.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_undersized_mtu_reports
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_undersized_mtu_reports = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_undersized_mtu_reports", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_undersized_mtu_reports() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_undersized_mtu_reports.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_block_user
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_block_user = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_block_user", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_block_user() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_block_user.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_connection_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_connection_request = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_connection_request", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_connection_request() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_connection_request.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_file_transfer
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_file_transfer = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_file_transfer", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_file_transfer() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_file_transfer.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_close_file_stores
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_close_file_stores = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_close_file_stores", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_close_file_stores() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_close_file_stores.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_group = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_group", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_group() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_group.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_invite
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_invite = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_invite", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_invite() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_invite.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_derive_user_id_from_public_key
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_derive_user_id_from_public_key = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_derive_user_id_from_public_key", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_derive_user_id_from_public_key() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_derive_user_id_from_public_key.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_disable_telemetry
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_disable_telemetry = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_disable_telemetry", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_disable_telemetry() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_disable_telemetry.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_emit_test_event
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_emit_test_event = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_emit_test_event", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_emit_test_event() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_emit_test_event.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_enable_telemetry
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_enable_telemetry = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_enable_telemetry", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_enable_telemetry() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_enable_telemetry.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_end_telemetry_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_end_telemetry_session = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_end_telemetry_session", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_end_telemetry_session() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_end_telemetry_session.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_erase_custody
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_erase_custody = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_erase_custody", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_erase_custody() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_erase_custody.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_establish_secure_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_establish_secure_session = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_establish_secure_session", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_establish_secure_session() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_establish_secure_session.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_finalize_file
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_finalize_file = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_finalize_file", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_finalize_file() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_finalize_file.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry_blocking
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry_blocking = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry_blocking", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry_blocking() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry_blocking.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_force_transport
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_force_transport = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_force_transport", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_force_transport() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_force_transport.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message_to_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message_to_group = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message_to_group", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message_to_group() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message_to_group.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_gateway_address_declaration
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_gateway_address_declaration = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_gateway_address_declaration", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_gateway_address_declaration() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_gateway_address_declaration.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_active_transports
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_active_transports = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_active_transports", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_active_transports() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_active_transports.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_battery_level
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_battery_level = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_battery_level", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_battery_level() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_battery_level.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_blocked_users
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_blocked_users = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_blocked_users", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_blocked_users() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_blocked_users.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_custody_stats
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_custody_stats = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_custody_stats", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_custody_stats() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_custody_stats.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dedup_stats
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dedup_stats = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dedup_stats", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dedup_stats() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dedup_stats.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_delivery_success_rate
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_delivery_success_rate = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_delivery_success_rate", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_delivery_success_rate() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_delivery_success_rate.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dors_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dors_config = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dors_config", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dors_config() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dors_config.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_establishment_state
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_establishment_state = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_establishment_state", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_establishment_state() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_establishment_state.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_file_progress
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_file_progress = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_file_progress", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_file_progress() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_file_progress.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_info
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_info = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_info", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_info() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_info.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_roles
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_roles = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_roles", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_roles() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_roles.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_identity_public_key
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_identity_public_key = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_identity_public_key", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_identity_public_key() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_identity_public_key.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_is_charging
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_is_charging = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_is_charging", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_is_charging() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_is_charging.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_hops
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_hops = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_hops", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_hops() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_hops.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_latency
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_latency = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_latency", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_latency() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_latency.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_member_role
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_member_role = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_member_role", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_member_role() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_member_role.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_stats
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_stats = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_stats", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_stats() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_stats.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_tunables
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_tunables = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_tunables", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_tunables() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_tunables.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_message_stats
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_message_stats = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_message_stats", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_message_stats() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_message_stats.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_pending_ack_count
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_pending_ack_count = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_pending_ack_count", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_pending_ack_count() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_pending_ack_count.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_config = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_config", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_config() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_config.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_priority
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_priority = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_priority", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_priority() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_priority.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_retry_queue_size
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_retry_queue_size = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_retry_queue_size", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_retry_queue_size() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_retry_queue_size.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_state
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_state = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_state", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_state() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_state.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_topology
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_topology = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_topology", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_topology() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_topology.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_transport_metrics
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_transport_metrics = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_transport_metrics", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_transport_metrics() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_transport_metrics.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_relay_sync_state
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_relay_sync_state = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_relay_sync_state", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_relay_sync_state() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_relay_sync_state.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_rich_readiness
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_rich_readiness = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_rich_readiness", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_rich_readiness() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_rich_readiness.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_has_pending_key_package
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_has_pending_key_package = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_has_pending_key_package", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_has_pending_key_package() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_has_pending_key_package.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_identity_assertion
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_identity_assertion = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_identity_assertion", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_identity_assertion() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_identity_assertion.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls_with_file_stores
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls_with_file_stores = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls_with_file_stores", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls_with_file_stores() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls_with_file_stores.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declaration_refused
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declaration_refused = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declaration_refused", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declaration_refused() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declaration_refused.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declared
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declared = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declared", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declared() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declared.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_confirm_sent
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_confirm_sent = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_confirm_sent", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_confirm_sent() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_confirm_sent.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_get_next_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_get_next_message = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_get_next_message", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_get_next_message() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_get_next_message.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_group_report_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_group_report_received = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_group_report_received", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_group_report_received() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_group_report_received.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_message_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_message_received = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_message_received", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_message_received() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_message_received.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_peer_presence
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_peer_presence = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_peer_presence", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_peer_presence() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_peer_presence.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_presence_watchlist
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_presence_watchlist = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_presence_watchlist", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_presence_watchlist() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_presence_watchlist.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_relay_capabilities
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_relay_capabilities = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_relay_capabilities", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_relay_capabilities() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_relay_capabilities.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed_with_reason
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed_with_reason = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed_with_reason", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed_with_reason() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed_with_reason.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_status_changed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_status_changed = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_status_changed", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_status_changed() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_status_changed.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_invite_to_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_invite_to_group = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_invite_to_group", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_invite_to_group() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_invite_to_group.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_mls_initialized
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_mls_initialized = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_mls_initialized", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_mls_initialized() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_mls_initialized.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_relay
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_relay = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_relay", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_relay() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_relay.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_user_blocked
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_user_blocked = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_user_blocked", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_user_blocked() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_user_blocked.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_leave_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_leave_group = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_leave_group", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_leave_group() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_leave_group.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_list_groups
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_list_groups = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_list_groups", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_list_groups() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_list_groups.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_local_address
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_local_address = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_local_address", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_local_address() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_local_address.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_clear_pending_welcome
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_clear_pending_welcome = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_clear_pending_welcome", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_clear_pending_welcome() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_clear_pending_welcome.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_create_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_create_session = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_create_session", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_create_session() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_create_session.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt_from_user
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt_from_user = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt_from_user", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt_from_user() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt_from_user.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_delete_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_delete_session = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_delete_session", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_delete_session() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_delete_session.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_encrypt_for_user
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_encrypt_for_user = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_encrypt_for_user", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_encrypt_for_user() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_encrypt_for_user.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_generate_key_package
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_generate_key_package = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_generate_key_package", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_generate_key_package() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_generate_key_package.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_or_create_key_package
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_or_create_key_package = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_or_create_key_package", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_or_create_key_package() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_or_create_key_package.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_key_packages
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_key_packages = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_key_packages", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_key_packages() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_key_packages.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_welcome
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_welcome = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_welcome", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_welcome() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_welcome.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_has_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_has_session = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_has_session", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_has_session() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_has_session.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_import_key_package
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_import_key_package = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_import_key_package", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_import_key_package() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_import_key_package.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_join_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_join_session = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_join_session", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_join_session() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_join_session.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_list_sessions
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_list_sessions = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_list_sessions", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_list_sessions() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_list_sessions.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_mark_key_package_synced
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_mark_key_package_synced = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_mark_key_package_synced", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_mark_key_package_synced() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_mark_key_package_synced.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_process_welcome
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_process_welcome = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_process_welcome", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_process_welcome() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_process_welcome.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_confirm_sent
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_confirm_sent = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_confirm_sent", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_confirm_sent() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_confirm_sent.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_message = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_message", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_message() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_message.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_query
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_query = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_query", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_query() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_query.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_public_key
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_public_key = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_public_key", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_public_key() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_public_key.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_subscription_filter
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_subscription_filter = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_subscription_filter", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_subscription_filter() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_subscription_filter.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received_at
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received_at = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received_at", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received_at() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received_at.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_completed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_completed = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_completed", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_completed() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_completed.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_event_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_event_received = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_event_received", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_event_received() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_event_received.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed_with_reason
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed_with_reason = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed_with_reason", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed_with_reason() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed_with_reason.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_status_changed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_status_changed = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_status_changed", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_status_changed() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_status_changed.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_notify_app_state
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_notify_app_state = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_notify_app_state", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_notify_app_state() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_notify_app_state.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_pause
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_pause = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_pause", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_pause() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_pause.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_poll_event
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_poll_event = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_poll_event", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_poll_event() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_poll_event.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process_file_chunk
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process_file_chunk = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process_file_chunk", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process_file_chunk() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process_file_chunk.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_protocol_lock_diagnostics
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_protocol_lock_diagnostics = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_protocol_lock_diagnostics", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_protocol_lock_diagnostics() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_protocol_lock_diagnostics.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_receive_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_receive_message = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_receive_message", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_receive_message() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_receive_message.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reject_connection_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reject_connection_request = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reject_connection_request", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reject_connection_request() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reject_connection_request.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rekey_session
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rekey_session = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rekey_session", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rekey_session() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rekey_session.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_release_transport_lock
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_release_transport_lock = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_release_transport_lock", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_release_transport_lock() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_release_transport_lock.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_from_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_from_group = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_from_group", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_from_group() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_from_group.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_transport
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_transport = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_transport", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_transport() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_transport.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rename_group
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rename_group = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rename_group", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rename_group() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rename_group.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_request_group_relay_registration
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_request_group_relay_registration = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_request_group_relay_registration", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_request_group_relay_registration() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_request_group_relay_registration.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resolve_username
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resolve_username = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resolve_username", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resolve_username() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resolve_username.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resume
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resume = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resume", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resume() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resume.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declaration_refused
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declaration_refused = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declaration_refused", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declaration_refused() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declaration_refused.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declared
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declared = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declared", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declared() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declared.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_confirm_sent
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_confirm_sent = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_confirm_sent", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_confirm_sent() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_confirm_sent.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_gateway_capabilities
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_gateway_capabilities = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_gateway_capabilities", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_gateway_capabilities() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_gateway_capabilities.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_get_next_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_get_next_message = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_get_next_message", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_get_next_message() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_get_next_message.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_message_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_message_received = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_message_received", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_message_received() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_message_received.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_peer_presence
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_peer_presence = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_peer_presence", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_peer_presence() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_peer_presence.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_presence_watchlist
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_presence_watchlist = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_presence_watchlist", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_presence_watchlist() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_presence_watchlist.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed_with_reason
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed_with_reason = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed_with_reason", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed_with_reason() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed_with_reason.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_status_changed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_status_changed = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_status_changed", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_status_changed() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_status_changed.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_connection_request
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_connection_request = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_connection_request", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_connection_request() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_connection_request.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_file
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_file = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_file", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_file() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_file.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_group_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_group_message = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_group_message", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_group_message() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_group_message.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media_rich
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media_rich = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media_rich", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media_rich() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media_rich.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message_rich
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message_rich = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message_rich", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message_rich() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message_rich.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_presence_update
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_presence_update = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_presence_update", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_presence_update() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_presence_update.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_read_receipt
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_read_receipt = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_read_receipt", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_read_receipt() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_read_receipt.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_typing_indicator
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_typing_indicator = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_typing_indicator", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_typing_indicator() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_typing_indicator.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_level
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_level = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_level", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_level() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_level.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_state
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_state = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_state", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_state() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_state.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_ble_transport_callback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_ble_transport_callback = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_ble_transport_callback", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_ble_transport_callback() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_ble_transport_callback.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_event_callback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_event_callback = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_event_callback", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_event_callback() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_event_callback.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_member_role
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_member_role = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_member_role", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_member_role() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_member_role.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_nostr_transport_callback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_nostr_transport_callback = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_nostr_transport_callback", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_nostr_transport_callback() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_nostr_transport_callback.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_relay_priority
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_relay_priority = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_relay_priority", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_relay_priority() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_relay_priority.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_reticulum_transport_callback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_reticulum_transport_callback = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_reticulum_transport_callback", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_reticulum_transport_callback() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_reticulum_transport_callback.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_telemetry_enabled
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_telemetry_enabled = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_telemetry_enabled", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_telemetry_enabled() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_telemetry_enabled.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_wifi_direct_transport_callback
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_wifi_direct_transport_callback = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_wifi_direct_transport_callback", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_wifi_direct_transport_callback() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_wifi_direct_transport_callback.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_should_escalate_to_wifi
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_should_escalate_to_wifi = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_should_escalate_to_wifi", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_should_escalate_to_wifi() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_should_escalate_to_wifi.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data_bytes
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data_bytes = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data_bytes", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data_bytes() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data_bytes.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_start
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_start = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_start", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_start() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_start.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_stop
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_stop = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_stop", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_stop() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_stop.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_install_id
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_install_id = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_install_id", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_install_id() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_install_id.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_stats
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_stats = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_stats", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_stats() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_stats.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_unblock_user
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_unblock_user = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_unblock_user", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_unblock_user() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_unblock_user.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_ack_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_ack_config = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_ack_config", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_ack_config() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_ack_config.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dedup_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dedup_config = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dedup_config", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dedup_config() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dedup_config.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dors_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dors_config = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dors_config", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dors_config() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dors_config.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_relay_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_relay_config = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_relay_config", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_relay_config() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_relay_config.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_retry_config
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_retry_config = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_retry_config", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_retry_config() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_retry_config.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_transport_metrics
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_transport_metrics = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_transport_metrics", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_transport_metrics() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_transport_metrics.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature_bytes
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature_bytes = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature_bytes", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature_bytes() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature_bytes.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_get_next_message
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_get_next_message = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_get_next_message", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_get_next_message() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_get_next_message.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_message_received
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_message_received = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_message_received", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_message_received() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_message_received.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_connected
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_connected = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_connected", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_connected() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_connected.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_disconnected
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_disconnected = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_disconnected", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_disconnected() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_disconnected.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_status_changed
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_status_changed = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_status_changed", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_status_changed() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_status_changed.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_constructor_datastore_new
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_constructor_datastore_new = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_constructor_datastore_new", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_constructor_datastore_new() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_constructor_datastore_new.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_constructor_datastore_with_storage
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_constructor_datastore_with_storage = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_constructor_datastore_with_storage", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_constructor_datastore_with_storage() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_constructor_datastore_with_storage.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_constructor_meshservices_new
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_constructor_meshservices_new = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_constructor_meshservices_new", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_constructor_meshservices_new() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_constructor_meshservices_new.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_constructor_offlineprotocol_new
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_constructor_offlineprotocol_new = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_constructor_offlineprotocol_new", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_constructor_offlineprotocol_new() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_constructor_offlineprotocol_new.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_bletransportcallback_on_fragments_available
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_bletransportcallback_on_fragments_available = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_bletransportcallback_on_fragments_available", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_bletransportcallback_on_fragments_available() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_bletransportcallback_on_fragments_available.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_eventcallback_on_event
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_eventcallback_on_event = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_eventcallback_on_event", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_eventcallback_on_event() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_eventcallback_on_event.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_store
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_store = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_store", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_store() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_store.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_load
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_load = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_load", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_load() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_load.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_delete
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_delete = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_delete", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_delete() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_delete.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_list_keys
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_list_keys = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_list_keys", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_list_keys() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_list_keys.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_nostrtransportcallback_on_messages_available
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_nostrtransportcallback_on_messages_available = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_nostrtransportcallback_on_messages_available", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_nostrtransportcallback_on_messages_available() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_nostrtransportcallback_on_messages_available.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_store
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_store = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_store", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_store() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_store.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_load
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_load = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_load", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_load() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_load.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_delete
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_delete = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_delete", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_delete() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_delete.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_list_keys
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_list_keys = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_list_keys", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_list_keys() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_list_keys.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_reticulumtransportcallback_on_messages_available
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_reticulumtransportcallback_on_messages_available = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_reticulumtransportcallback_on_messages_available", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_reticulumtransportcallback_on_messages_available() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_reticulumtransportcallback_on_messages_available.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // uniffi_offline_protocol_uniffi_checksum_method_wifidirecttransportcallback_on_messages_available
    private static final java.lang.invoke.MethodHandle MH_uniffi_offline_protocol_uniffi_checksum_method_wifidirecttransportcallback_on_messages_available = findDowncallHandle("uniffi_offline_protocol_uniffi_checksum_method_wifidirecttransportcallback_on_messages_available", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_SHORT));

    static short uniffi_offline_protocol_uniffi_checksum_method_wifidirecttransportcallback_on_messages_available() {
        try {
            return (short) MH_uniffi_offline_protocol_uniffi_checksum_method_wifidirecttransportcallback_on_messages_available.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    // ffi_offline_protocol_uniffi_uniffi_contract_version
    private static final java.lang.invoke.MethodHandle MH_ffi_offline_protocol_uniffi_uniffi_contract_version = findDowncallHandle("ffi_offline_protocol_uniffi_uniffi_contract_version", java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_INT));

    static int ffi_offline_protocol_uniffi_uniffi_contract_version() {
        try {
            return (int) MH_ffi_offline_protocol_uniffi_uniffi_contract_version.invokeExact();
        } catch (Throwable _ex) { throw new AssertionError("invokeExact failed", _ex); }
    }

    

    // Integrity checks and initialization must happen after all MethodHandle fields
    // are initialized (static fields are initialized in textual order in Java).
    static {
        NamespaceLibrary.uniffiCheckContractApiVersion();
        NamespaceLibrary.uniffiCheckApiChecksums();
        
        CLEANER = UniffiCleaner.create();
        UniffiCallbackInterfaceBleTransportCallback.INSTANCE.register();
        UniffiCallbackInterfaceEventCallback.INSTANCE.register();
        UniffiCallbackInterfaceMlsStorageProvider.INSTANCE.register();
        UniffiCallbackInterfaceNostrTransportCallback.INSTANCE.register();
        UniffiCallbackInterfaceProtocolStateStorageProvider.INSTANCE.register();
        UniffiCallbackInterfaceReticulumTransportCallback.INSTANCE.register();
        UniffiCallbackInterfaceWifiDirectTransportCallback.INSTANCE.register();
        }
}

