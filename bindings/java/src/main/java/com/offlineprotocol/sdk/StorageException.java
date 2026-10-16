// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

/**
 * Thrown by a {@link StorageProvider} to say how an operation failed. Any
 * other exception a provider throws is reported as the operation's own
 * failure ({@link Kind#STORE_FAILED} for a store, and so on), never as
 * corruption.
 */
public final class StorageException extends Exception {
    private static final long serialVersionUID = 1L;

    /** {@code OfflineProtocolStorageStatus} in the C header. Append-only. */
    public enum Kind {
        /** The write did not happen. */
        STORE_FAILED(1),
        /** The read failed and may succeed later. A locked keystore or an I/O failure is this. */
        LOAD_FAILED(2),
        /** The delete failed. */
        DELETE_FAILED(3),
        /** The key is not there, where the operation required it to be. */
        KEY_NOT_FOUND(4),
        /**
         * The record is permanently unreadable. Report this only for a record no
         * later launch can read: the engine answers it on the protocol-state record
         * key by minting a new key, which strands every record sealed under the old one.
         */
        CORRUPTED_DATA(5);

        private final int value;

        Kind(int value) {
            this.value = value;
        }

        int value() {
            return value;
        }
    }

    private final Kind kind;

    public StorageException(Kind kind, String message) {
        super(message);
        this.kind = kind;
    }

    public StorageException(Kind kind, String message, Throwable cause) {
        super(message, cause);
        this.kind = kind;
    }

    public Kind kind() {
        return kind;
    }
}
