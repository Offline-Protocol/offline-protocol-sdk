using System.Text.Json;
using OfflineProtocol.Sdk.Telemetry;

namespace OfflineProtocol.Sdk.Tests;

public sealed class TelemetryConfigJsonTests
{
    [Fact]
    public void EnableTelemetry_omits_os_and_includes_os_major_and_optional_fields()
    {
        var json = TelemetryConfigJson.EnableTelemetry(
            new TelemetryConfig
            {
                ApiKey = "k",
                AppId = "app",
                OsMajor = 11,
                ScrubIds = true,
                MlsVerbosity = MlsVerbosity.Lifecycle,
            },
            AppState.Background);

        using var doc = JsonDocument.Parse(json);
        var root = doc.RootElement;
        var config = root.GetProperty("config");
        Assert.False(config.TryGetProperty("os", out _));
        Assert.Equal(11, config.GetProperty("os_major").GetInt32());
        Assert.Equal("k", config.GetProperty("api_key").GetString());
        Assert.True(config.GetProperty("scrub_ids").GetBoolean());
        Assert.Equal("Lifecycle", config.GetProperty("mls_verbosity").GetString());
        Assert.Equal("Background", root.GetProperty("app_state").GetString());
    }
}
