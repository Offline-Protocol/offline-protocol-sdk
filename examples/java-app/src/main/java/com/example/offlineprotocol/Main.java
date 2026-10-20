package com.example.offlineprotocol;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.offlineprotocol.sdk.InternetMessage;
import com.offlineprotocol.sdk.MessagePriority;
import com.offlineprotocol.sdk.OfflineProtocol;
import com.offlineprotocol.sdk.OverflowPolicy;
import com.offlineprotocol.sdk.ProtocolConfig;
import com.offlineprotocol.sdk.ProtocolException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.List;
import java.util.function.BooleanSupplier;

/**
 * Two devices, Alice and Bob, in one JVM. They pair, establish an end-to-end
 * encrypted session and exchange two messages.
 *
 * <p>A real app moves bytes over a radio or a socket. Here the "network" is a
 * loop that takes what one engine wants to send ({@code internetGetNextMessage})
 * and hands it to the engine it is addressed to ({@code internetMessageReceived}),
 * which is exactly what a transport does. Nothing else is simulated: the
 * pairing, the MLS session and the encryption are the engine's own.
 */
public final class Main {
    private static final ObjectMapper JSON = new ObjectMapper();

    /** One device: an engine, the name it pairs under, and the events it has seen. */
    private static final class Device {
        final String name;
        final OfflineProtocol engine;
        final String address;
        final List<JsonNode> seen = new ArrayList<>();

        Device(String name, Path storage) throws Exception {
            this.name = name;
            this.engine = new OfflineProtocol(config(name));

            // The engine's own encrypted file stores. An app with a keystore of
            // its own implements MlsStorageProvider and ProtocolStateStorageProvider
            // and calls initializeMls instead.
            byte[] storeKey = new byte[32];
            new SecureRandom().nextBytes(storeKey);
            engine.initializeMlsWithFileStores(
                    Files.createDirectory(storage.resolve("mls")).toString(),
                    Files.createDirectory(storage.resolve("state")).toString(),
                    storeKey);
            this.address = engine.localAddress();

            engine.start();
            // The internet slot is the carrier here, and it is "up".
            engine.internetStatusChanged(true);
        }

        List<Byte> keyPackage() throws Exception {
            return engine.mlsGetOrCreateKeyPackage().keyPackageData();
        }

        /** Reads the engine's events and answers the pairing ones, as an app does. */
        void handleEvents() throws Exception {
            String json;
            while ((json = engine.pollEvent()) != null) {
                JsonNode event = JSON.readTree(json);
                seen.add(event);
                switch (event.path("type").asText()) {
                    case "connection_request_received" -> {
                        String from = event.path("sender").asText();
                        importKeyPackage(from, event.path("key_package"));
                        engine.acceptConnectionRequest(from, name, keyPackage());
                        tryEstablish(from);
                    }
                    case "connection_accepted" -> {
                        String from = event.path("accepted_by").asText();
                        importKeyPackage(from, event.path("key_package"));
                        tryEstablish(from);
                    }
                    default -> { }
                }
            }
        }

        /**
         * Best effort. With auto_key_exchange on, the engine finishes the session
         * itself once the peer's key package arrives over the wire, so a refusal
         * for want of one here only means "not yet".
         */
        private void tryEstablish(String peer) {
            try {
                engine.establishSecureSession(peer);
            } catch (ProtocolException.SessionNotReady notYet) {
                // The key package comes next.
            } catch (ProtocolException e) {
                throw new IllegalStateException(e);
            }
        }

        private void importKeyPackage(String peer, JsonNode keyPackage) throws Exception {
            if (keyPackage.isArray() && !keyPackage.isEmpty()) {
                List<Byte> bytes = new ArrayList<>(keyPackage.size());
                keyPackage.forEach(b -> bytes.add((byte) b.asInt()));
                engine.mlsImportKeyPackage(peer, bytes);
            }
        }

        boolean saw(String type) {
            return seen.stream().anyMatch(e -> type.equals(e.path("type").asText()));
        }

        String received(String content) {
            return seen.stream()
                    .filter(e -> "message_received".equals(e.path("type").asText()))
                    .filter(e -> content.equals(e.path("content").asText()))
                    .map(e -> e.path("sender").asText())
                    .findFirst()
                    .orElse(null);
        }
    }

    /** The configuration this example needs; every field is spelled out (docs/bridges/java.md, J2). */
    static ProtocolConfig config(String profile) {
        return new ProtocolConfig(
                "com.example.java", profile,
                false, false, true, false, false, true, // ble, wifi direct, internet, reticulum, nostr, prefer online
                (byte) 8, true, true, true, true, // initial ttl, encryption, auto key exchange, store pending, require encryption
                64L, 4096L, 60_000L, OverflowPolicy.DROP_OLDEST,
                false, 256, true, true, false, false, // edge-driven DM, group size, group relay x2, admin commits, transport identity
                true, true, true, false, true, true, true, // binary wire, nostr x3, compact envelope, rich payload, recovery
                null, null, // mesh relay, custody
                true, true, true); // data, control freshness, encrypt service messages
    }

    /** The network: whatever one device wants to send goes to the device it is addressed to. */
    private static void carry(List<Device> devices) throws Exception {
        for (Device from : devices) {
            InternetMessage message;
            while ((message = from.engine.internetGetNextMessage()) != null) {
                for (Device to : devices) {
                    if (to.address.equals(message.recipientId())) {
                        to.engine.internetMessageReceived(from.address, message.data());
                    }
                }
            }
        }
    }

    private static void until(List<Device> devices, String what, BooleanSupplier done) throws Exception {
        long deadline = System.nanoTime() + 20_000_000_000L;
        while (!done.getAsBoolean()) {
            if (System.nanoTime() > deadline) {
                throw new IllegalStateException("timed out waiting for: " + what);
            }
            for (Device device : devices) {
                device.engine.process();
                device.handleEvents();
            }
            carry(devices);
            Thread.sleep(50);
        }
        System.out.println("  ok: " + what);
    }

    public static void main(String[] args) throws Exception {
        Path root = Files.createTempDirectory("offline-protocol-java-example");
        Device alice = new Device("alice", Files.createDirectory(root.resolve("alice")));
        Device bob = new Device("bob", Files.createDirectory(root.resolve("bob")));
        List<Device> devices = List.of(alice, bob);
        System.out.println("alice " + alice.address);
        System.out.println("bob   " + bob.address);

        try {
            // Pairing: Alice asks, Bob accepts, and each ends up holding the
            // other's key package, which is what the session is made from.
            alice.engine.sendConnectionRequest(bob.address, alice.name, alice.keyPackage(), null);
            until(devices, "bob and alice are paired",
                    () -> alice.saw("connection_accepted") && alice.engine.hasPendingKeyPackage(bob.address));

            alice.engine.sendMessage(bob.address, "hello bob", MessagePriority.MEDIUM, null);
            until(devices, "bob received \"hello bob\" (encrypted end to end)",
                    () -> bob.received("hello bob") != null);

            bob.engine.sendMessage(alice.address, "hello alice", MessagePriority.MEDIUM, null);
            until(devices, "alice received \"hello alice\"",
                    () -> alice.received("hello alice") != null);

            System.out.println("done");
        } finally {
            for (Device device : devices) {
                device.engine.stop();
                device.engine.close();
            }
        }
    }
}
