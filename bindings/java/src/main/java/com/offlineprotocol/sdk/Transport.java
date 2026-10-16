// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

/**
 * The transports that notify the host of outbound frames. The Internet
 * transport has no notification in the definition; the host polls it.
 */
public enum Transport {
    BLE(0),
    WIFI_DIRECT(1),
    RETICULUM(2),
    NOSTR(3);

    private final int value;

    Transport(int value) {
        this.value = value;
    }

    /** {@code OfflineProtocolTransport} in the C header. */
    int value() {
        return value;
    }
}
