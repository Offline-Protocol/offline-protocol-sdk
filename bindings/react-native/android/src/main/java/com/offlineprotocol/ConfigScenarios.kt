package com.offlineprotocol

import org.json.JSONObject
import uniffi.offline_protocol.ProtocolConfig

/**
 * Opinionated [ProtocolConfig] presets for native Android / Flutter hosts.
 *
 * Mirrors `bindings/react-native/src/configScenarios.ts` (G4 PR1); keep scenario
 * ids and transport posture in sync. No UDL or wire changes.
 */
enum class ConfigScenario {
    localBle,
    proximityMesh,
    internetRelay,
    nostrMesh,

    /** Cleartext-only local testing. Never ship to production users. */
    developmentCleartext,
}

object ConfigScenarios {
  val scenarioIds: List<ConfigScenario> = ConfigScenario.entries

  /**
   * Builds a full [ProtocolConfig] for direct UniFFI use.
   */
  fun forScenario(
      scenario: ConfigScenario,
      appId: String,
      profile: String,
      customize: ((ProtocolConfig) -> Unit)? = null,
  ): ProtocolConfig {
    val config = meshBaseline(appId, profile)
    applyScenario(scenario, config)
    customize?.invoke(config)
    return config
  }

  /**
   * JSON for [ProtocolEngineHost] / Flutter `create(configJson:)`.
   */
  fun forScenarioJson(
      scenario: ConfigScenario,
      appId: String,
      profile: String,
      extra: JSONObject = JSONObject(),
  ): String {
    val payload =
        JSONObject()
            .put("appId", appId)
            .put("profile", profile)
    mergeJson(payload, scenarioPresetJson(scenario))
    mergeJson(payload, extra)
    return payload.toString()
  }

  private fun meshBaseline(appId: String, profile: String): ProtocolConfig {
    val json =
        JSONObject()
            .put("appId", appId)
            .put("profile", profile)
            .put("bleEnabled", true)
            .put("wifiDirectEnabled", false)
            .put("internetEnabled", false)
            .put("reticulumEnabled", false)
            .put("nostrEnabled", false)
    return ProtocolConfigParser.parse(json.toString()).coreConfig
  }

  private fun applyScenario(scenario: ConfigScenario, config: ProtocolConfig) {
    when (scenario) {
      ConfigScenario.localBle -> Unit
      ConfigScenario.proximityMesh -> config.wifiDirectEnabled = true
      ConfigScenario.internetRelay -> {
        config.internetEnabled = true
        config.preferOnline = true
      }
      ConfigScenario.nostrMesh -> config.nostrEnabled = true
      ConfigScenario.developmentCleartext -> {
        config.encryptionEnabled = false
        config.autoKeyExchange = false
        config.storePending = false
        config.requireEncryption = false
      }
    }
  }

  /** Flat UniFFI flags plus nested `transports` / `dors` aligned with TS presets. */
  private fun scenarioPresetJson(scenario: ConfigScenario): JSONObject {
    val payload = flatTransportFlags(scenario)
    payload.put("transports", nestedTransports(scenario))
    when (scenario) {
      ConfigScenario.localBle,
      ConfigScenario.proximityMesh,
      ConfigScenario.nostrMesh -> Unit
      ConfigScenario.internetRelay -> {
        payload.put("preferOnline", true)
        payload.put("dors", JSONObject().put("preferOnline", true))
      }
      ConfigScenario.developmentCleartext -> {
        payload.put("encryption", JSONObject().put("enabled", false))
      }
    }
    return payload
  }

  private fun flatTransportFlags(scenario: ConfigScenario): JSONObject {
    val flags =
        JSONObject()
            .put("bleEnabled", true)
            .put("wifiDirectEnabled", false)
            .put("internetEnabled", false)
            .put("nostrEnabled", false)
            .put("reticulumEnabled", false)
    when (scenario) {
      ConfigScenario.localBle -> Unit
      ConfigScenario.proximityMesh -> flags.put("wifiDirectEnabled", true)
      ConfigScenario.internetRelay -> flags.put("internetEnabled", true)
      ConfigScenario.nostrMesh -> flags.put("nostrEnabled", true)
      ConfigScenario.developmentCleartext -> Unit
    }
    return flags
  }

  private fun nestedTransports(scenario: ConfigScenario): JSONObject =
      when (scenario) {
        ConfigScenario.localBle ->
            JSONObject()
                .put("ble", JSONObject().put("enabled", true))
                .put("wifiDirect", JSONObject().put("enabled", false))
                .put("internet", JSONObject().put("enabled", false))
                .put("nostr", JSONObject().put("enabled", false))
                .put("reticulum", JSONObject().put("enabled", false))
        ConfigScenario.proximityMesh ->
            JSONObject()
                .put("ble", JSONObject().put("enabled", true))
                .put("wifiDirect", JSONObject().put("enabled", true))
                .put("internet", JSONObject().put("enabled", false))
                .put("nostr", JSONObject().put("enabled", false))
        ConfigScenario.internetRelay ->
            JSONObject()
                .put("ble", JSONObject().put("enabled", true))
                .put("internet", JSONObject().put("enabled", true))
        ConfigScenario.nostrMesh ->
            JSONObject()
                .put("ble", JSONObject().put("enabled", true))
                .put("nostr", JSONObject().put("enabled", true))
        ConfigScenario.developmentCleartext ->
            JSONObject().put("ble", JSONObject().put("enabled", true))
      }

  private fun mergeJson(into: JSONObject, layer: JSONObject) {
    layer.keys().forEach { key ->
      val value = layer.get(key)
      val existing = into.opt(key)
      if (value is JSONObject && existing is JSONObject) {
        mergeJson(existing, value)
      } else {
        into.put(key, value)
      }
    }
  }
}
