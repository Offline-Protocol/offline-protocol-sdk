// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

/**
 * A call the library refused. Unchecked: the refusals that matter to a
 * caller are {@link Status#ENGINE_ERROR}, which carries the definition's
 * {@code ProtocolError} variant and position, and the rest are programming
 * errors.
 */
public final class OfflineProtocolException extends RuntimeException {
    private static final long serialVersionUID = 1L;

    private final Status status;
    private final int code;
    private final String variant;

    OfflineProtocolException(Status status, int code, String variant, String message) {
        super(message == null ? status.name() : message);
        this.status = status;
        this.code = code;
        this.variant = variant;
    }

    /** Why the call failed. */
    public Status status() {
        return status;
    }

    /**
     * For {@link Status#ENGINE_ERROR}, the variant's zero-based position in the
     * definition's {@code ProtocolError} (the local API's {@code -32000 - code});
     * otherwise -1.
     */
    public int code() {
        return code;
    }

    /** For {@link Status#ENGINE_ERROR}, the variant's name, such as {@code "MlsNotInitialized"}; otherwise null. */
    public String variant() {
        return variant;
    }
}
