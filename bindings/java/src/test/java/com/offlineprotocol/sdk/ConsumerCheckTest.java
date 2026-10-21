package com.offlineprotocol.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * The Java program that proves the generated binding, the library and the
 * interface definition agree: the counterpart of bindings/c/consumer-check/main.c.
 * It loads the real library, whose FFI checksums the generated code verifies
 * before the first call, so a binding generated from a different definition
 * than the library fails every test here.
 */
class ConsumerCheckTest {
    /** A configuration with the definition's defaults; the generated record has no defaults of its own. */
    static ProtocolConfig config() {
        return new ProtocolConfig(
                "com.example.java", "java-consumer",
                false, false, true, false, false, false, // ble, wifi direct, internet, reticulum, nostr, prefer online
                (byte) 8, true, true, true, true, // initial ttl, encryption, auto key exchange, store pending, require encryption
                64L, 4096L, 60_000L, OverflowPolicy.DROP_OLDEST,
                false, 256, true, true, false, false, // edge-driven DM, group size, group relay x2, admin commits, transport identity
                true, true, true, false, true, true, true, // binary wire, nostr x3, compact envelope, rich payload, recovery
                null, null, // mesh relay, custody
                true, true, true); // data, control freshness, encrypt service messages
    }

    static final byte[] STORE_KEY = new byte[32];

    static {
        for (int i = 0; i < STORE_KEY.length; i++) {
            STORE_KEY[i] = (byte) i;
        }
    }

    @Test
    void anInstanceLessFunctionNeedsNoEngine() throws Exception {
        List<Byte> publicKey = new ArrayList<>(Collections.nCopies(32, (byte) 0));
        String address = OfflineProtocolLib.deriveAddress(publicKey);
        assertTrue(address.startsWith("off1"), address);
    }

    @Test
    void anEngineAnswersAndEventsReachBothTheCallbackAndThePollQueue() throws Exception {
        try (OfflineProtocol engine = new OfflineProtocol(config())) {
            assertEquals(ProtocolState.STOPPED, engine.getState());

            List<String> heard = new CopyOnWriteArrayList<>();
            Thread[] on = new Thread[1];
            engine.setEventCallback(json -> {
                on[0] = Thread.currentThread();
                heard.add(json);
            });
            engine.emitTestEvent();
            assertEquals(1, heard.size());
            assertEquals(Thread.currentThread(), on[0], "the event runs on the calling thread");
            assertEquals(heard.get(0), engine.pollEvent());
            assertNull(engine.pollEvent());
        }
    }

    @Test
    void anEngineRefusalIsATypedException() throws Exception {
        try (OfflineProtocol engine = new OfflineProtocol(config())) {
            engine.start();
            // No storage yet, so there is no session to encrypt under.
            assertThrows(ProtocolException.EncryptFailed.class, () -> engine.sendMessage(
                    "off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s", "hi", MessagePriority.MEDIUM, null));
            engine.stop();
        }
    }

    @Test
    void aJavaStorageProviderPassesTheConformanceSuite() {
        String report = OfflineProtocolLib.runStorageConformance(new MemoryStorage().state);
        assertTrue(report.contains("\"failures\":[]"), report);
    }

    @Test
    void twoJavaProvidersGiveTheEngineAnAddress() throws Exception {
        MemoryStorage secure = new MemoryStorage();
        MemoryStorage state = new MemoryStorage();
        try (OfflineProtocol engine = new OfflineProtocol(config())) {
            engine.initializeMls(secure.mls, state.state);
            assertTrue(engine.localAddress().startsWith("off1"));
            assertTrue(secure.size() > 0, "nothing reached the secure provider");
            engine.start();
            engine.stop();
        }
    }

    @Test
    void theBuiltInFileStoresNeedNoProvider(@TempDir Path dir) throws Exception {
        Path mls = Files.createDirectory(dir.resolve("mls"));
        Path state = Files.createDirectory(dir.resolve("state"));
        try (OfflineProtocol engine = new OfflineProtocol(config())) {
            engine.initializeMlsWithFileStores(mls.toString(), state.toString(), STORE_KEY);
            assertTrue(engine.localAddress().startsWith("off1"));
            engine.closeFileStores();
        }
    }

    @Test
    void aProviderThatThrowsIsReportedAsAStorageFailure() {
        ProtocolStateStorageProvider failing = new ProtocolStateStorageProvider() {
            @Override
            public void store(String keyType, String keyId, byte[] data) throws MlsStorageException {
                throw new MlsStorageException.StoreFailed("disk full");
            }

            @Override
            public byte[] load(String keyType, String keyId) {
                return null;
            }

            @Override
            public void delete(String keyType, String keyId) {
            }

            @Override
            public List<String> listKeys(String keyType) {
                return new ArrayList<>();
            }
        };
        String report = OfflineProtocolLib.runStorageConformance(failing);
        assertFalse(report.contains("\"failures\":[]"), "a provider that cannot store must not pass: " + report);
        // The exception crossed back into the engine as its storage error, not as a crash.
        assertTrue(report.contains("Failed to store protocol state"), report);
    }
}
