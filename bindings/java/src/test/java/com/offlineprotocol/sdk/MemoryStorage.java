package com.offlineprotocol.sdk;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * A thread-safe in-memory store, as a host writes one, behind both storage
 * interfaces. They cannot be one class: the key material interface takes and
 * returns {@code List<Byte>} and the protocol-state one {@code byte[]}, and
 * their {@code load} methods differ only in return type.
 */
final class MemoryStorage {
    private final Map<String, byte[]> entries = new ConcurrentHashMap<>();

    private static String key(String type, String id) {
        return type + '\u0000' + id;
    }

    private List<String> keys(String keyType) {
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

    /** The protocol-state provider: bytes as {@code byte[]}. */
    final ProtocolStateStorageProvider state = new ProtocolStateStorageProvider() {
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
        public void delete(String keyType, String keyId) {
            entries.remove(key(keyType, keyId));
        }

        @Override
        public List<String> listKeys(String keyType) {
            return keys(keyType);
        }
    };

    /** The key material provider: bytes as {@code List<Byte>}. */
    final MlsStorageProvider mls = new MlsStorageProvider() {
        @Override
        public void store(String keyType, String keyId, List<Byte> data) {
            byte[] bytes = new byte[data.size()];
            for (int i = 0; i < bytes.length; i++) {
                bytes[i] = data.get(i);
            }
            entries.put(key(keyType, keyId), bytes);
        }

        @Override
        public List<Byte> load(String keyType, String keyId) {
            byte[] v = entries.get(key(keyType, keyId));
            if (v == null) {
                return null;
            }
            List<Byte> out = new ArrayList<>(v.length);
            for (byte b : v) {
                out.add(b);
            }
            return out;
        }

        @Override
        public void delete(String keyType, String keyId) {
            entries.remove(key(keyType, keyId));
        }

        @Override
        public List<String> listKeys(String keyType) {
            return keys(keyType);
        }
    };
}
