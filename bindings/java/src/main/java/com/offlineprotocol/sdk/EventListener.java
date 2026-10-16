// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

/**
 * Receives engine events as JSON, the same text {@link OfflineProtocol#pollEvent()} returns.
 *
 * <p>It runs synchronously on the thread that is inside the engine, before the
 * engine call that produced the event returns: your own thread for an event
 * {@code sendMessage} emitted, or one of the engine's. So it must be safe to call
 * from any thread, and <b>it must not call back into the engine</b>, which may be
 * holding a lock the call needs. Copy the text, queue it, return. An exception it
 * throws is swallowed: unwinding into Rust is undefined behaviour.
 */
@FunctionalInterface
public interface EventListener {
    void onEvent(String eventJson);
}
