// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Path;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * The Java program that proves the binding, the library and the contract in
 * docs/bridges/c.md agree: the counterpart of bindings/c/consumer-check/main.c.
 * It loads the real library, so it fails if the JNA declarations and the header drift.
 */
class ConsumerCheckTest {
    static final String CONFIG = "{\"app_id\":\"com.example.java\",\"profile\":\"java-consumer\","
            + "\"ble_enabled\":false,\"wifi_direct_enabled\":false,\"internet_enabled\":true,"
            + "\"reticulum_enabled\":false,\"nostr_enabled\":false,\"prefer_online\":false,"
            + "\"initial_ttl\":8,\"encryption_enabled\":true,\"auto_key_exchange\":true,"
            + "\"store_pending\":true,\"max_pending_per_peer\":64,\"max_pending_global\":4096,"
            + "\"pending_ttl_ms\":60000,\"overflow_policy\":\"DropOldest\"}";

    @Test
    void versionAndInstanceLessCallsNeedNoEngine() {
        assertFalse(OfflineProtocol.version().isEmpty());
        String address = OfflineProtocol.callStatic(
                "derive_address", "{\"public_key\":\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\"}");
        assertTrue(address.startsWith("\"off1"), address);
    }

    @Test
    void anEngineAnswersAndEventsReachBothTheListenerAndThePollQueue() {
        try (OfflineProtocol engine = OfflineProtocol.create(CONFIG)) {
            assertEquals("\"Stopped\"", engine.call("get_state", null));

            List<String> heard = new CopyOnWriteArrayList<>();
            Thread caller = Thread.currentThread();
            Thread[] on = new Thread[1];
            engine.setEventListener(json -> {
                on[0] = Thread.currentThread();
                heard.add(json);
            });
            engine.call("emit_test_event", null);
            assertEquals(1, heard.size());
            assertEquals(caller, on[0], "the event runs on the calling thread");
            assertEquals(heard.get(0), engine.pollEvent());
            assertNull(engine.pollEvent());

            // A null listener stops delivery; the queue still fills.
            engine.setEventListener(null);
            engine.call("emit_test_event", null);
            assertEquals(1, heard.size());
            assertNotNull(engine.pollEvent());
        }
    }

    @Test
    void refusalsCarryTheirStatus() {
        try (OfflineProtocol engine = OfflineProtocol.create(CONFIG)) {
            assertEquals(Status.METHOD_NOT_FOUND,
                    assertThrows(OfflineProtocolException.class, () -> engine.call("no_such_method", null)).status());
            assertEquals(Status.INVALID_ARGUMENT,
                    assertThrows(OfflineProtocolException.class, () -> engine.call("get_state", "{")).status());
            assertEquals(Status.INVALID_PARAMS,
                    assertThrows(OfflineProtocolException.class, () -> engine.call("get_state", "{\"x\":1}")).status());
        }
        assertEquals(Status.INVALID_ARGUMENT,
                assertThrows(OfflineProtocolException.class, () -> OfflineProtocol.create("{")).status());
    }

    @Test
    void anEngineRefusalNamesTheDefinitionsVariantAndPosition() {
        try (OfflineProtocol engine = OfflineProtocol.create(CONFIG)) {
            engine.call("start", null);
            OfflineProtocolException e = assertThrows(OfflineProtocolException.class, () -> engine.call("send_message",
                    "{\"recipient\":\"off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s\",\"content\":\"hi\","
                            + "\"priority\":\"Medium\"}"));
            assertEquals(Status.ENGINE_ERROR, e.status());
            assertEquals("EncryptFailed", e.variant());
            assertEquals(6, e.code());
            engine.call("stop", null);
        }
    }

    @Test
    void aClosedEngineRefusesCallsAndCloseIsIdempotent() {
        OfflineProtocol engine = OfflineProtocol.create(CONFIG);
        engine.close();
        engine.close();
        assertThrows(IllegalStateException.class, () -> engine.call("get_state", null));
    }

    @Test
    void aTransportListenerIsDroppedWhenTheEngineIsFreed() {
        int before = Callbacks.liveRegistrations();
        try (OfflineProtocol engine = OfflineProtocol.create(CONFIG)) {
            engine.setTransportListener(Transport.BLE, () -> { });
            assertEquals(before + 1, Callbacks.liveRegistrations());
        }
        assertEquals(before, Callbacks.liveRegistrations(), "drop(ctx) releases the registration");
    }

    @Test
    void aReplacedEventListenerIsDropped() {
        int before = Callbacks.liveRegistrations();
        try (OfflineProtocol engine = OfflineProtocol.create(CONFIG)) {
            engine.setEventListener(json -> { });
            engine.setEventListener(json -> { });
            assertEquals(before + 1, Callbacks.liveRegistrations());
        }
        assertEquals(before, Callbacks.liveRegistrations());
    }

    @Test
    void aListenerThatThrowsDoesNotUnwindIntoTheEngine() {
        try (OfflineProtocol engine = OfflineProtocol.create(CONFIG)) {
            engine.setEventListener(json -> {
                throw new IllegalStateException("host bug");
            });
            engine.call("emit_test_event", null);
            assertNotNull(engine.pollEvent());
        }
    }

    @Test
    void aJavaStorageProviderPassesTheConformanceSuite() {
        String report = OfflineProtocol.runStorageConformance(new MemoryStorage());
        assertTrue(report.contains("\"failures\":[]"), report);
    }

    @Test
    void twoJavaProvidersGiveTheEngineAnAddress() {
        MemoryStorage secure = new MemoryStorage();
        MemoryStorage state = new MemoryStorage();
        int before = Callbacks.liveRegistrations();
        try (OfflineProtocol engine = OfflineProtocol.create(CONFIG)) {
            engine.initializeMls(secure, state);
            assertTrue(engine.call("local_address", null).startsWith("\"off1"));
            assertTrue(secure.size() > 0, "nothing reached the secure provider");
            engine.call("start", null);
            engine.call("stop", null);
        }
        assertEquals(before, Callbacks.liveRegistrations());
        assertEquals(0, StorageBridge.handedOut(), "every buffer was returned through release");
    }

    @Test
    void storageKeyIdsAreEscapedAsJson() {
        assertEquals("[\"a\",\"b\\\"c\",\"d\\\\e\",\"f\\u0001\"]",
                StorageBridge.jsonArray(List.of("a", "b\"c", "d\\e", "f\u0001")));
    }

    @Test
    void theBuiltInFileStoresNeedNoProvider(@TempDir Path dir) {
        Path mls = dir.resolve("mls");
        Path state = dir.resolve("state");
        mls.toFile().mkdirs();
        state.toFile().mkdirs();
        try (OfflineProtocol engine = OfflineProtocol.create(CONFIG)) {
            engine.call("initialize_mls_with_file_stores", "{\"mls_root\":\"" + mls + "\",\"state_root\":\"" + state
                    + "\",\"store_key\":\"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=\"}");
            assertTrue(engine.call("local_address", null).startsWith("\"off1"));
            engine.call("close_file_stores", null);
        }
    }
}
