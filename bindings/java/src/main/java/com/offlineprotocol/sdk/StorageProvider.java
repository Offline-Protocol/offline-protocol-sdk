// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

import java.util.List;

/**
 * Durable storage the engine keeps its state in: the four operations every
 * binding's provider implements ({@code docs/bridges/c.md} K6). Values are
 * opaque bytes, stored and returned verbatim.
 *
 * <p>Methods may be called from any thread, concurrently, so an implementation
 * must be thread-safe; the engine promises Rust that on the host's behalf and cannot
 * check it. Writes must be durable and atomic per entry. The conformance suite,
 * {@link OfflineProtocol#runStorageConformance(StorageProvider)}, is the bar for
 * a supported backend: an adapter that returns normally from every method can
 * still lose overwrites or tear a concurrent one.
 *
 * <p>Two rules are where a provider loses data. Throw {@link StorageException.Kind#CORRUPTED_DATA}
 * only for a record no later launch can read; a locked keystore or an I/O failure
 * is {@link StorageException.Kind#LOAD_FAILED}. And for the protocol-state provider,
 * {@link #load} must not return more than 8 MiB: a larger entry is corrupt.
 */
public interface StorageProvider {
    /** Writes {@code data} under the key, replacing any previous value. */
    void store(String keyType, String keyId, byte[] data) throws StorageException;

    /** Reads the key, or returns null when it was never written. */
    byte[] load(String keyType, String keyId) throws StorageException;

    /** Removes the key. Removing a key that is not there is success. */
    void remove(String keyType, String keyId) throws StorageException;

    /** The key ids stored under {@code keyType}. */
    List<String> listKeys(String keyType) throws StorageException;
}
