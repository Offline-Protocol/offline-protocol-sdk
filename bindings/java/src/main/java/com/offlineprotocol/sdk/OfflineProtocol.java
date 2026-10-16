// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

import com.sun.jna.Pointer;
import com.sun.jna.ptr.PointerByReference;
import java.util.Objects;
import java.util.concurrent.locks.Lock;
import java.util.concurrent.locks.ReadWriteLock;
import java.util.concurrent.locks.ReentrantReadWriteLock;

/**
 * An engine, driven through the C API ({@code docs/bridges/c.md}).
 *
 * <p>Methods are named as the local API names them: bare for the engine
 * ({@code "send_message"}), {@code "services."} for mesh services,
 * {@code "data."} for the document store. Parameters are a JSON object keyed by
 * the interface definition's parameter names, or {@code null} for none; the result
 * is JSON text. Bring your own JSON library: nothing here parses it, and nothing
 * here changes when the definition grows a method.
 *
 * <p>An engine is usable from any thread. {@link #close()} waits for calls in
 * flight and frees the engine once; the engine is not usable afterwards.
 *
 * <pre>{@code
 * try (OfflineProtocol engine = OfflineProtocol.create(configJson)) {
 *     engine.call("initialize_mls_with_file_stores", fileStoreParams);
 *     engine.call("start", null);
 *     String address = engine.call("local_address", null); // "\"off1...\""
 * }
 * }</pre>
 */
public final class OfflineProtocol implements AutoCloseable {
    private final Pointer handle;
    // Calls take the read side so they run together; close takes the write
    // side, which is the C contract's "no call in flight when it is freed".
    private final ReadWriteLock lifecycle = new ReentrantReadWriteLock();
    private boolean closed;

    private OfflineProtocol(Pointer handle) {
        this.handle = handle;
    }

    /** The library's version, the release it was built from, such as {@code "0.29.0"}. */
    public static String version() {
        return NativeApi.INSTANCE.offline_protocol_version();
    }

    /**
     * Creates an engine from a {@code ProtocolConfig} given as a JSON object. Fields
     * the interface definition gives a default may be omitted.
     *
     * @throws OfflineProtocolException if the configuration is refused
     */
    public static OfflineProtocol create(String configJson) {
        Objects.requireNonNull(configJson, "configJson");
        NativeApi.ErrorStruct err = new NativeApi.ErrorStruct();
        PointerByReference out = new PointerByReference();
        NativeApi.check(NativeApi.INSTANCE.offline_protocol_create(configJson, out, err), err);
        return new OfflineProtocol(out.getValue());
    }

    /**
     * Calls one of the definition's instance-less functions ({@code derive_address},
     * {@code parse_invite}, {@code verify_identity_assertion}), which need no engine.
     */
    public static String callStatic(String method, String paramsJson) {
        Objects.requireNonNull(method, "method");
        NativeApi.ErrorStruct err = new NativeApi.ErrorStruct();
        PointerByReference out = new PointerByReference();
        NativeApi.check(NativeApi.INSTANCE.offline_protocol_call_static(method, paramsJson, out, err), err);
        return NativeApi.takeString(out.getValue());
    }

    /**
     * Calls a method of the engine, its mesh services or its document store.
     *
     * @param paramsJson a JSON object, or {@code null} for no parameters
     * @return the result as JSON; {@code "null"} for a method that returns nothing
     * @throws OfflineProtocolException if the engine or the binding refuses the call
     * @throws IllegalStateException if the engine is closed
     */
    public String call(String method, String paramsJson) {
        Objects.requireNonNull(method, "method");
        Lock lock = lifecycle.readLock();
        lock.lock();
        try {
            requireOpen();
            NativeApi.ErrorStruct err = new NativeApi.ErrorStruct();
            PointerByReference out = new PointerByReference();
            NativeApi.check(NativeApi.INSTANCE.offline_protocol_call(handle, method, paramsJson, out, err), err);
            return NativeApi.takeString(out.getValue());
        } finally {
            lock.unlock();
        }
    }

    /**
     * Installs the event listener, replacing any earlier one. {@code null} stops
     * delivery. Every event is also queued for {@link #pollEvent()} either way.
     */
    public void setEventListener(EventListener listener) {
        Lock lock = lifecycle.readLock();
        lock.lock();
        try {
            requireOpen();
            NativeApi.ErrorStruct err = new NativeApi.ErrorStruct();
            int status;
            if (listener == null) {
                status = NativeApi.INSTANCE.offline_protocol_set_event_callback(handle, null, null, null, err);
            } else {
                status = NativeApi.INSTANCE.offline_protocol_set_event_callback(
                        handle, Callbacks.EVENT, Callbacks.DROP, Callbacks.register(listener), err);
            }
            NativeApi.check(status, err);
        } finally {
            lock.unlock();
        }
    }

    /** The oldest queued event as JSON, or null when none is queued. The queue keeps the newest 1000. */
    public String pollEvent() {
        Lock lock = lifecycle.readLock();
        lock.lock();
        try {
            requireOpen();
            return NativeApi.takeString(NativeApi.INSTANCE.offline_protocol_poll_event(handle));
        } finally {
            lock.unlock();
        }
    }

    /**
     * Installs a transport's outbound notification, replacing any earlier one: it says
     * the transport has frames for the host to drain with the {@code ble_},
     * {@code wifi_direct_}, {@code reticulum_} or {@code nostr_} methods. The same
     * thread and re-entrancy rules as {@link EventListener} apply. {@code null} removes it.
     */
    public void setTransportListener(Transport transport, Runnable notify) {
        Objects.requireNonNull(transport, "transport");
        Lock lock = lifecycle.readLock();
        lock.lock();
        try {
            requireOpen();
            NativeApi.ErrorStruct err = new NativeApi.ErrorStruct();
            int status;
            if (notify == null) {
                status = NativeApi.INSTANCE.offline_protocol_set_transport_callback(
                        handle, transport.value(), null, null, null, err);
            } else {
                status = NativeApi.INSTANCE.offline_protocol_set_transport_callback(
                        handle, transport.value(), Callbacks.NOTIFY, Callbacks.DROP, Callbacks.register(notify), err);
            }
            NativeApi.check(status, err);
        } finally {
            lock.unlock();
        }
    }

    /** Frees the engine, once. Waits for calls in flight; later calls throw {@link IllegalStateException}. */
    @Override
    public void close() {
        Lock lock = lifecycle.writeLock();
        lock.lock();
        try {
            if (!closed) {
                closed = true;
                NativeApi.INSTANCE.offline_protocol_free(handle);
            }
        } finally {
            lock.unlock();
        }
    }

    private void requireOpen() {
        if (closed) {
            throw new IllegalStateException("the engine is closed");
        }
    }
}
