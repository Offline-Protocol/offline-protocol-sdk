package com.offlineprotocol.config;

import com.offlineprotocol.sdk.OverflowPolicy;
import com.offlineprotocol.sdk.ProtocolConfig;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Consumer;

/**
 * Opinionated {@link ProtocolConfig} presets for JVM hosts.
 *
 * <p>Mirrors {@code bindings/react-native/src/configScenarios.ts} (G4 PR1); keep
 * scenario ids and transport posture in sync. Lives outside
 * {@code com.offlineprotocol.sdk} so binding regeneration does not delete it.
 * No UDL or wire changes.
 */
public final class ConfigScenarios {

    /** Named integration scenarios covered by docs and tests. */
    public enum ConfigScenario {
        localBle,
        proximityMesh,
        internetRelay,
        nostrMesh,
        /** Cleartext-only local testing. Never ship to production users. */
        developmentCleartext,
    }

    public static final List<ConfigScenario> SCENARIO_IDS = List.of(ConfigScenario.values());

    private ConfigScenarios() {}

    /**
     * Builds a full {@link ProtocolConfig} for direct UniFFI use.
     */
    public static ProtocolConfig forScenario(
            ConfigScenario scenario, String appId, String profile) {
        return forScenario(scenario, appId, profile, null);
    }

    public static ProtocolConfig forScenario(
            ConfigScenario scenario,
            String appId,
            String profile,
            Consumer<ProtocolConfig> customize) {
        ProtocolConfig config = meshBaseline(appId, profile);
        applyScenario(scenario, config);
        if (customize != null) {
            customize.accept(config);
        }
        return config;
    }

    /**
     * JSON aligned with mobile bridge presets (local API gateways, tooling). JVM
     * hosts normally use {@link #forScenario} instead.
     */
    public static String forScenarioJson(ConfigScenario scenario, String appId, String profile) {
        return forScenarioJson(scenario, appId, profile, Map.of());
    }

    public static String forScenarioJson(
            ConfigScenario scenario,
            String appId,
            String profile,
            Map<String, Object> extra) {
        Map<String, Object> payload = new LinkedHashMap<>();
        payload.put("appId", appId);
        payload.put("profile", profile);
        mergeJson(payload, scenarioPresetJson(scenario));
        mergeJson(payload, extra);
        return toJson(payload);
    }

    /**
     * Mesh baseline transport flags plus definition defaults (see
     * ConsumerCheckTest.config() for the full field list).
     */
    private static ProtocolConfig meshBaseline(String appId, String profile) {
        return new ProtocolConfig(
                appId,
                profile,
                true,
                false,
                false,
                false,
                false,
                false,
                (byte) 8,
                true,
                true,
                true,
                true,
                64L,
                4096L,
                86_400_000L,
                OverflowPolicy.DROP_OLDEST,
                false,
                256,
                true,
                true,
                false,
                false,
                true,
                true,
                true,
                false,
                true,
                true,
                true,
                null,
                null,
                true,
                true,
                true);
    }

    private static void applyScenario(ConfigScenario scenario, ProtocolConfig config) {
        switch (scenario) {
            case localBle -> { /* baseline only */ }
            case proximityMesh -> config.setWifiDirectEnabled(true);
            case internetRelay -> {
                config.setInternetEnabled(true);
                config.setPreferOnline(true);
            }
            case nostrMesh -> config.setNostrEnabled(true);
            case developmentCleartext -> {
                config.setEncryptionEnabled(false);
                config.setAutoKeyExchange(false);
                config.setStorePending(false);
                config.setRequireEncryption(false);
            }
        }
    }

    private static Map<String, Object> scenarioPresetJson(ConfigScenario scenario) {
        Map<String, Object> payload = flatTransportFlags(scenario);
        payload.put("transports", nestedTransports(scenario));
        switch (scenario) {
            case localBle, proximityMesh, nostrMesh -> { /* flags only */ }
            case internetRelay -> {
                payload.put("preferOnline", true);
                payload.put("dors", Map.of("preferOnline", true));
            }
            case developmentCleartext -> payload.put("encryption", Map.of("enabled", false));
        }
        return payload;
    }

    private static Map<String, Object> flatTransportFlags(ConfigScenario scenario) {
        Map<String, Object> flags = new LinkedHashMap<>();
        flags.put("bleEnabled", true);
        flags.put("wifiDirectEnabled", false);
        flags.put("internetEnabled", false);
        flags.put("nostrEnabled", false);
        flags.put("reticulumEnabled", false);
        switch (scenario) {
            case localBle, developmentCleartext -> { /* baseline */ }
            case proximityMesh -> flags.put("wifiDirectEnabled", true);
            case internetRelay -> flags.put("internetEnabled", true);
            case nostrMesh -> flags.put("nostrEnabled", true);
        }
        return flags;
    }

    private static Map<String, Object> nestedTransports(ConfigScenario scenario) {
        return switch (scenario) {
            case localBle -> Map.of(
                    "ble", Map.of("enabled", true),
                    "wifiDirect", Map.of("enabled", false),
                    "internet", Map.of("enabled", false),
                    "nostr", Map.of("enabled", false),
                    "reticulum", Map.of("enabled", false));
            case proximityMesh -> Map.of(
                    "ble", Map.of("enabled", true),
                    "wifiDirect", Map.of("enabled", true),
                    "internet", Map.of("enabled", false),
                    "nostr", Map.of("enabled", false));
            case internetRelay -> Map.of(
                    "ble", Map.of("enabled", true),
                    "internet", Map.of("enabled", true));
            case nostrMesh -> Map.of(
                    "ble", Map.of("enabled", true),
                    "nostr", Map.of("enabled", true));
            case developmentCleartext -> Map.of("ble", Map.of("enabled", true));
        };
    }

    @SuppressWarnings("unchecked")
    private static void mergeJson(Map<String, Object> into, Map<String, Object> layer) {
        for (Map.Entry<String, Object> entry : layer.entrySet()) {
            String key = entry.getKey();
            Object value = entry.getValue();
            Object existing = into.get(key);
            if (value instanceof Map<?, ?> nested && existing instanceof Map<?, ?> existingMap) {
                Map<String, Object> merged = new LinkedHashMap<>((Map<String, Object>) existingMap);
                mergeJson(merged, (Map<String, Object>) nested);
                into.put(key, merged);
            } else {
                into.put(key, value);
            }
        }
    }

    private static String toJson(Object value) {
        if (value == null) {
            return "null";
        }
        if (value instanceof String s) {
            return escape(s);
        }
        if (value instanceof Boolean b) {
            return b ? "true" : "false";
        }
        if (value instanceof Number n) {
            return n.toString();
        }
        if (value instanceof Map<?, ?> map) {
            StringBuilder sb = new StringBuilder();
            sb.append('{');
            boolean first = true;
            for (Map.Entry<?, ?> entry : map.entrySet()) {
                if (!first) {
                    sb.append(',');
                }
                first = false;
                sb.append(escape(String.valueOf(entry.getKey())));
                sb.append(':');
                sb.append(toJson(entry.getValue()));
            }
            sb.append('}');
            return sb.toString();
        }
        throw new IllegalArgumentException("unsupported JSON value: " + value.getClass());
    }

    private static String escape(String s) {
        StringBuilder b = new StringBuilder(s.length() + 8);
        b.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"' -> b.append("\\\"");
                case '\\' -> b.append("\\\\");
                case '\b' -> b.append("\\b");
                case '\f' -> b.append("\\f");
                case '\n' -> b.append("\\n");
                case '\r' -> b.append("\\r");
                case '\t' -> b.append("\\t");
                default -> {
                    if (c < 0x20) {
                        b.append(String.format("\\u%04x", (int) c));
                    } else {
                        b.append(c);
                    }
                }
            }
        }
        b.append('"');
        return b.toString();
    }
}
