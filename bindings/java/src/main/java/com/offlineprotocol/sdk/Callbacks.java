// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

import com.sun.jna.Pointer;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * What the library calls back into.
 *
 * <p>A C callback has one {@code ctx} pointer, and a JNA {@code Callback} object
 * must stay strongly reachable for as long as native code may call it. Both are
 * solved the same way: the callback objects are static singletons, and the
 * per-registration state is looked up by an integer carried in {@code ctx}. The
 * library calls {@code drop(ctx)} exactly once when it no longer holds a
 * registration, a refused one included ({@code docs/bridges/c.md} K2), and that
 * is the only place an entry leaves the table. A host that removed it earlier
 * would race a callback already running on another thread.
 */
final class Callbacks {
    private static final ConcurrentHashMap<Long, Object> TABLE = new ConcurrentHashMap<>();
    private static final AtomicLong NEXT = new AtomicLong(1); // 0 would be a null ctx

    private Callbacks() {}

    static final NativeApi.DropFn DROP = ctx -> TABLE.remove(id(ctx));

    static final NativeApi.EventFn EVENT = (ctx, json) -> {
        Object target = TABLE.get(id(ctx));
        if (target instanceof EventListener && json != null) {
            try {
                ((EventListener) target).onEvent(json.getString(0, "UTF-8"));
            } catch (Throwable ignored) {
                // Unwinding through a C frame into Rust is undefined behaviour.
            }
        }
    };

    static final NativeApi.NotifyFn NOTIFY = ctx -> {
        Object target = TABLE.get(id(ctx));
        if (target instanceof Runnable) {
            try {
                ((Runnable) target).run();
            } catch (Throwable ignored) {
                // As above.
            }
        }
    };

    /** Registers {@code target} and returns the {@code ctx} to hand the library alongside {@link #DROP}. */
    static Pointer register(Object target) {
        long id = NEXT.getAndIncrement();
        TABLE.put(id, target);
        return Pointer.createConstant(id);
    }

    static Object lookup(Pointer ctx) {
        return TABLE.get(id(ctx));
    }

    static int liveRegistrations() {
        return TABLE.size();
    }

    private static long id(Pointer ctx) {
        return Pointer.nativeValue(ctx);
    }
}
