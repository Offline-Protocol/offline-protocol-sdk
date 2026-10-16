// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

import com.sun.jna.Memory;
import com.sun.jna.Pointer;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Adapts a {@link StorageProvider} to the C {@code OfflineProtocolStorage}.
 *
 * <p>The callbacks are static singletons that find the provider by the id in
 * {@code ctx} ({@link Callbacks}). A buffer {@code load} or {@code listKeys} hands
 * back is native memory this side allocated, so it is held here until the library
 * returns it through {@code release}, and not a moment longer.
 */
final class StorageBridge {
    private static final ConcurrentHashMap<Long, Memory> HANDED_OUT = new ConcurrentHashMap<>();

    private StorageBridge() {}

    private static StorageProvider provider(Pointer ctx) {
        return (StorageProvider) Callbacks.lookup(ctx);
    }

    /** The status for a failure that is not the provider's own to classify: the operation's, never corruption. */
    private static int failure(Throwable t, StorageException.Kind fallback) {
        return t instanceof StorageException ? ((StorageException) t).kind().value() : fallback.value();
    }

    private static final NativeApi.StoreFn STORE = (ctx, type, id, data, len) -> {
        try {
            byte[] bytes = len == 0 ? new byte[0] : data.getByteArray(0, Math.toIntExact(len));
            provider(ctx).store(type.getString(0, "UTF-8"), id.getString(0, "UTF-8"), bytes);
            return 0;
        } catch (Throwable t) {
            return failure(t, StorageException.Kind.STORE_FAILED);
        }
    };

    private static final NativeApi.LoadFn LOAD = (ctx, type, id, outData, outLen) -> {
        try {
            byte[] bytes = provider(ctx).load(type.getString(0, "UTF-8"), id.getString(0, "UTF-8"));
            if (bytes == null) {
                outData.setPointer(0, null);
                outLen.setLong(0, 0);
            } else {
                hand(bytes, outData, outLen);
            }
            return 0;
        } catch (Throwable t) {
            return failure(t, StorageException.Kind.LOAD_FAILED);
        }
    };

    private static final NativeApi.RemoveFn REMOVE = (ctx, type, id) -> {
        try {
            provider(ctx).remove(type.getString(0, "UTF-8"), id.getString(0, "UTF-8"));
            return 0;
        } catch (Throwable t) {
            return failure(t, StorageException.Kind.DELETE_FAILED);
        }
    };

    private static final NativeApi.ListKeysFn LIST_KEYS = (ctx, type, outJson, outLen) -> {
        try {
            List<String> ids = provider(ctx).listKeys(type.getString(0, "UTF-8"));
            hand(jsonArray(ids).getBytes(StandardCharsets.UTF_8), outJson, outLen);
            return 0;
        } catch (Throwable t) {
            return failure(t, StorageException.Kind.LOAD_FAILED);
        }
    };

    private static final NativeApi.ReleaseFn RELEASE = (ctx, data, len) -> {
        if (data != null) {
            HANDED_OUT.remove(Pointer.nativeValue(data));
        }
    };

    private static void hand(byte[] bytes, Pointer outData, Pointer outLen) {
        Memory buffer = new Memory(Math.max(bytes.length, 1));
        buffer.write(0, bytes, 0, bytes.length);
        HANDED_OUT.put(Pointer.nativeValue(buffer), buffer);
        outData.setPointer(0, buffer);
        outLen.setLong(0, bytes.length);
    }

    /** A JSON array of strings: the form {@code list_keys} answers in. */
    static String jsonArray(List<String> ids) {
        StringBuilder out = new StringBuilder("[");
        for (int i = 0; i < ids.size(); i++) {
            if (i > 0) {
                out.append(',');
            }
            out.append('"');
            for (char c : ids.get(i).toCharArray()) {
                switch (c) {
                    case '"': out.append("\\\""); break;
                    case '\\': out.append("\\\\"); break;
                    default:
                        if (c < 0x20) {
                            out.append(String.format("\\u%04x", (int) c));
                        } else {
                            out.append(c);
                        }
                }
            }
            out.append('"');
        }
        return out.append(']').toString();
    }

    /** The C structure for {@code provider}. The library copies it; its {@code drop} releases the registration. */
    static NativeApi.StorageStruct struct(StorageProvider provider) {
        NativeApi.StorageStruct s = new NativeApi.StorageStruct();
        s.ctx = Callbacks.register(provider);
        s.store = STORE;
        s.load = LOAD;
        s.remove = REMOVE;
        s.list_keys = LIST_KEYS;
        s.release = RELEASE;
        s.drop = Callbacks.DROP;
        return s;
    }

    static int handedOut() {
        return HANDED_OUT.size();
    }
}
