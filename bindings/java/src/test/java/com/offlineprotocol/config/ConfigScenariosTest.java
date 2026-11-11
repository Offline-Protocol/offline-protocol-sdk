package com.offlineprotocol.config;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

class ConfigScenariosTest {

    @Test
    void localBleJsonCarriesFlatTransportFlags() {
        String json = ConfigScenarios.forScenarioJson(ConfigScenario.localBle, "a", "p");
        assertTrue(json.contains("\"bleEnabled\":true"));
        assertTrue(json.contains("\"internetEnabled\":false"));
        assertTrue(json.contains("\"wifiDirectEnabled\":false"));
    }

    @Test
    void internetRelayPreferOnline() {
        var config = ConfigScenarios.forScenario(ConfigScenario.internetRelay, "a", "p");
        assertTrue(config.getInternetEnabled());
        assertTrue(config.getPreferOnline());
    }

    @Test
    void developmentCleartextDisablesEncryption() {
        var config = ConfigScenarios.forScenario(ConfigScenario.developmentCleartext, "a", "p");
        assertFalse(config.getEncryptionEnabled());
        assertFalse(config.getRequireEncryption());
    }

    @Test
    void forScenarioUsesRequestedIdentity() {
        var config = ConfigScenarios.forScenario(ConfigScenario.localBle, "app-id", "prof");
        assertEquals("app-id", config.getAppId());
        assertEquals("prof", config.getProfile());
    }
}
