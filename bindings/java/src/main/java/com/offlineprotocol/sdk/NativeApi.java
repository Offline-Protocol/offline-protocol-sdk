// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

import com.sun.jna.Callback;
import com.sun.jna.Library;
import com.sun.jna.Native;
import com.sun.jna.Pointer;
import com.sun.jna.Structure;
import com.sun.jna.ptr.PointerByReference;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * The C API of {@code bindings/c/include/offline_protocol.h}, declared for JNA.
 * Nothing here is public: {@link OfflineProtocol} is the surface.
 *
 * <p>Strings the library hands out arrive as raw pointers and are read as UTF-8
 * here, because JNA's own {@code String} return would copy the text and leave
 * the original to leak: {@code offline_protocol_string_free} is the only thing
 * that may release it.
 */
interface NativeApi extends Library {
    /** Overrides the library lookup with a path, for a host that does not use the search path. */
    String PATH_PROPERTY = "offline_protocol.library";

    NativeApi INSTANCE = load();

    static NativeApi load() {
        // size_t is declared as long below, which holds only where a pointer is 8 bytes.
        // The desktop releases are all 64-bit; refuse the rest rather than corrupt memory.
        if (Native.POINTER_SIZE != 8) {
            throw new UnsatisfiedLinkError("the Java binding needs a 64-bit JVM");
        }
        Map<String, Object> options = new HashMap<>();
        // Every string in the C API is UTF-8, whatever the JVM's default charset is.
        options.put(Library.OPTION_STRING_ENCODING, "UTF-8");
        String path = System.getProperty(PATH_PROPERTY);
        return Native.load(path != null ? path : "offline_protocol_uniffi", NativeApi.class, options);
    }

    /** {@code OfflineProtocolError}. Written only when a call fails. */
    @Structure.FieldOrder({"status", "code", "variant", "message"})
    class ErrorStruct extends Structure {
        public int status;
        public int code;
        public Pointer variant;
        public Pointer message;
    }

    /** {@code OfflineProtocolStorage}. */
    @Structure.FieldOrder({"ctx", "store", "load", "remove", "list_keys", "release", "drop"})
    class StorageStruct extends Structure {
        public Pointer ctx;
        public StoreFn store;
        public LoadFn load;
        public RemoveFn remove;
        public ListKeysFn list_keys;
        public ReleaseFn release;
        public DropFn drop;
    }

    interface EventFn extends Callback {
        void invoke(Pointer ctx, Pointer eventJson);
    }

    interface DropFn extends Callback {
        void invoke(Pointer ctx);
    }

    interface NotifyFn extends Callback {
        void invoke(Pointer ctx);
    }

    interface StoreFn extends Callback {
        int invoke(Pointer ctx, Pointer keyType, Pointer keyId, Pointer data, long len);
    }

    interface LoadFn extends Callback {
        int invoke(Pointer ctx, Pointer keyType, Pointer keyId, Pointer outData, Pointer outLen);
    }

    interface RemoveFn extends Callback {
        int invoke(Pointer ctx, Pointer keyType, Pointer keyId);
    }

    interface ListKeysFn extends Callback {
        int invoke(Pointer ctx, Pointer keyType, Pointer outJson, Pointer outLen);
    }

    interface ReleaseFn extends Callback {
        void invoke(Pointer ctx, Pointer data, long len);
    }

    String offline_protocol_version();

    int offline_protocol_call_static(String method, String paramsJson, PointerByReference outResult, ErrorStruct err);

    int offline_protocol_create(String configJson, PointerByReference outHandle, ErrorStruct err);

    void offline_protocol_free(Pointer handle);

    int offline_protocol_call(Pointer handle, String method, String paramsJson, PointerByReference outResult, ErrorStruct err);

    int offline_protocol_set_event_callback(Pointer handle, EventFn onEvent, DropFn drop, Pointer ctx, ErrorStruct err);

    Pointer offline_protocol_poll_event(Pointer handle);

    int offline_protocol_set_transport_callback(Pointer handle, int transport, NotifyFn notify, DropFn drop, Pointer ctx, ErrorStruct err);

    int offline_protocol_initialize_mls(Pointer handle, StorageStruct secure, StorageStruct state, ErrorStruct err);

    int offline_protocol_run_storage_conformance(StorageStruct storage, PointerByReference outResult, ErrorStruct err);

    void offline_protocol_string_free(Pointer s);

    void offline_protocol_error_clear(ErrorStruct err);

    /** Reads and frees a library string; null is the empty answer. */
    static String takeString(Pointer p) {
        if (p == null) {
            return null;
        }
        try {
            return p.getString(0, "UTF-8");
        } finally {
            INSTANCE.offline_protocol_string_free(p);
        }
    }

    /** Throws if {@code status} is not Ok, and always clears {@code err}. */
    static void check(int status, ErrorStruct err) {
        try {
            if (status == 0) {
                return;
            }
            err.read();
            String variant = err.variant == null ? null : err.variant.getString(0, "UTF-8");
            String message = err.message == null ? null : err.message.getString(0, "UTF-8");
            throw new OfflineProtocolException(Status.of(status), err.code, variant, message);
        } finally {
            INSTANCE.offline_protocol_error_clear(err);
        }
    }
}
