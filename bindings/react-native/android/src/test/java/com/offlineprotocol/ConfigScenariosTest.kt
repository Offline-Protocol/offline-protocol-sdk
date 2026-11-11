package com.offlineprotocol

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ConfigScenariosTest {

    @Test
    fun localBleParsesFromJson() {
        val json = ConfigScenarios.forScenarioJson(ConfigScenario.localBle, "a", "p")
        val config = ProtocolConfigParser.parse(json).coreConfig
        assertTrue(config.bleEnabled)
        assertFalse(config.internetEnabled)
        assertFalse(config.wifiDirectEnabled)
        assertFalse(config.nostrEnabled)
    }

    @Test
    fun internetRelayPreferOnline() {
        val config = ConfigScenarios.forScenario(ConfigScenario.internetRelay, "a", "p")
        assertTrue(config.internetEnabled)
        assertTrue(config.preferOnline)
    }

    @Test
    fun developmentCleartextDisablesEncryption() {
        val config = ConfigScenarios.forScenario(ConfigScenario.developmentCleartext, "a", "p")
        assertFalse(config.encryptionEnabled)
        assertFalse(config.requireEncryption)
    }
}
