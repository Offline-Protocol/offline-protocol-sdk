using System.Text.Json;
using OfflineProtocol.Sdk.Config;

namespace OfflineProtocol.Sdk.Tests;

public sealed class ConfigScenariosTests
{
    [Fact]
    public void Local_ble_json_matches_engine_shape()
    {
        var json = ConfigScenarios.ForScenarioJson(ConfigScenario.localBle, "app", "p");
        using var doc = JsonDocument.Parse(json);
        Assert.True(doc.RootElement.GetProperty("ble_enabled").GetBoolean());
        Assert.False(doc.RootElement.GetProperty("internet_enabled").GetBoolean());
        Assert.False(doc.RootElement.GetProperty("wifi_direct_enabled").GetBoolean());
    }

    [Fact]
    public void Internet_relay_sets_prefer_online()
    {
        var config = ConfigScenarios.ForScenario(ConfigScenario.internetRelay, "a", "p");
        Assert.True(config.InternetEnabled);
        Assert.True(config.PreferOnline);
    }

    [Fact]
    public void Development_cleartext_disables_encryption()
    {
        var config = ConfigScenarios.ForScenario(ConfigScenario.developmentCleartext, "a", "p");
        Assert.False(config.EncryptionEnabled);
        Assert.False(config.RequireEncryption);
    }
}
