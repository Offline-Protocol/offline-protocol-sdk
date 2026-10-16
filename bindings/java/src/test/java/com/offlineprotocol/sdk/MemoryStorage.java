// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/** A thread-safe in-memory provider, as a host writes one. Key ids are the library's own, so no escaping is needed. */
final class MemoryStorage implements StorageProvider {
    private final Map<String, byte[]> entries = new ConcurrentHashMap<>();

    private static String key(String type, String id) {
        return type + '\u0000' + id;
    }

    @Override
    public void store(String keyType, String keyId, byte[] data) {
        entries.put(key(keyType, keyId), data.clone());
    }

    @Override
    public byte[] load(String keyType, String keyId) {
        byte[] v = entries.get(key(keyType, keyId));
        return v == null ? null : v.clone();
    }

    @Override
    public void remove(String keyType, String keyId) {
        entries.remove(key(keyType, keyId));
    }

    @Override
    public List<String> listKeys(String keyType) {
        List<String> ids = new ArrayList<>();
        String prefix = keyType + '\u0000';
        for (String k : entries.keySet()) {
            if (k.startsWith(prefix)) {
                ids.add(k.substring(prefix.length()));
            }
        }
        return ids;
    }

    int size() {
        return entries.size();
    }
}
