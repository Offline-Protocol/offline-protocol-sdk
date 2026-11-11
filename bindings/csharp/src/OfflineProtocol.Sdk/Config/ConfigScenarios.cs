// Opinionated ProtocolConfig presets (G4). Mirrors bindings/react-native/src/configScenarios.ts.
// No UDL or wire changes.

using System;
using System.Collections.Generic;

namespace OfflineProtocol.Sdk.Config;

/// <summary>Named integration scenarios covered by docs and tests.</summary>
public enum ConfigScenario
{
    localBle,
    proximityMesh,
    internetRelay,
    nostrMesh,

    /// <summary>Cleartext-only local testing. Never ship to production users.</summary>
    developmentCleartext,
}

/// <summary>Scenario presets for <see cref="ProtocolConfig"/> construction.</summary>
public static class ConfigScenarios
{
    public static IReadOnlyList<ConfigScenario> ScenarioIds { get; } =
        Enum.GetValues<ConfigScenario>();

    /// <summary>Builds a full <see cref="ProtocolConfig"/> for <see cref="ProtocolEngine.CreateAsync"/>.</summary>
    public static ProtocolConfig ForScenario(ConfigScenario scenario, string appId, string profile) =>
        ForScenario(scenario, appId, profile, map: null);

    /// <param name="map">Applied last; use record <c>with</c> to tune fields after the preset.</param>
    public static ProtocolConfig ForScenario(
        ConfigScenario scenario,
        string appId,
        string profile,
        Func<ProtocolConfig, ProtocolConfig>? map)
    {
        ArgumentException.ThrowIfNullOrEmpty(appId);
        ArgumentException.ThrowIfNullOrEmpty(profile);

        var config = ApplyScenario(scenario, MeshBaseline(appId, profile));
        return map?.Invoke(config) ?? config;
    }

    /// <summary>JSON for the C API / local API create path (<see cref="ProtocolConfigJson"/>).</summary>
    public static string ForScenarioJson(ConfigScenario scenario, string appId, string profile) =>
        ProtocolConfigJson.Serialize(ForScenario(scenario, appId, profile));

    private static ProtocolConfig MeshBaseline(string appId, string profile) =>
        new()
        {
            AppId = appId,
            Profile = profile,
            BleEnabled = true,
            WifiDirectEnabled = false,
            InternetEnabled = false,
            ReticulumEnabled = false,
            NostrEnabled = false,
            PreferOnline = false,
            InitialTtl = 8,
            EncryptionEnabled = true,
            AutoKeyExchange = true,
            StorePending = true,
            MaxPendingPerPeer = 64,
            MaxPendingGlobal = 4096,
            PendingTtlMs = 86_400_000,
            OverflowPolicy = OverflowPolicy.DropOldest,
        };

    private static ProtocolConfig ApplyScenario(ConfigScenario scenario, ProtocolConfig config) =>
        scenario switch
        {
            ConfigScenario.localBle => config,
            ConfigScenario.proximityMesh => config with { WifiDirectEnabled = true },
            ConfigScenario.internetRelay => config with
            {
                InternetEnabled = true,
                PreferOnline = true,
            },
            ConfigScenario.nostrMesh => config with { NostrEnabled = true },
            ConfigScenario.developmentCleartext => config with
            {
                EncryptionEnabled = false,
                AutoKeyExchange = false,
                StorePending = false,
                RequireEncryption = false,
            },
            _ => throw new ArgumentOutOfRangeException(nameof(scenario), scenario, null),
        };
}
