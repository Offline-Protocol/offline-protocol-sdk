// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

/** Why a call produced no result: {@code OfflineProtocolStatus} in the C header, minus {@code Ok}. */
public enum Status {
    /** The engine refused; {@link OfflineProtocolException#variant()} names the {@code ProtocolError} variant. */
    ENGINE_ERROR(1),
    /** Text that is not UTF-8, or parameters that are not JSON. */
    INVALID_ARGUMENT(2),
    /** No method of that name, or one that has its own entry point. */
    METHOD_NOT_FOUND(3),
    /** The parameters are JSON and do not fit the declaration. */
    INVALID_PARAMS(4),
    /** The library failed: a panic caught at the boundary. */
    INTERNAL(5);

    private final int value;

    Status(int value) {
        this.value = value;
    }

    /** The number the C header gives this status. */
    public int value() {
        return value;
    }

    static Status of(int value) {
        for (Status s : values()) {
            if (s.value == value) {
                return s;
            }
        }
        // A status this binding does not know is a library newer than the
        // binding. It is still a failure, and Internal is the honest name.
        return INTERNAL;
    }
}
