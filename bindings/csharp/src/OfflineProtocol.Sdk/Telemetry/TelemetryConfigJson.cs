using System.Text.Json;

namespace OfflineProtocol.Sdk.Telemetry;

internal static class TelemetryConfigJson
{
    /// <summary>Serializes <c>enable_telemetry</c> parameters. Never includes <c>os</c>.</summary>
    public static string EnableTelemetry(TelemetryConfig config, AppState appState)
    {
        var configObject = new Dictionary<string, object?>
        {
            ["api_key"] = config.ApiKey,
            ["app_id"] = config.AppId,
            ["os_major"] = config.OsMajor ?? TelemetryPlatform.OsMajor,
        };
        AddOptional(configObject, "app_version", config.AppVersion);
        AddOptional(configObject, "debug", config.Debug);
        AddOptional(configObject, "flush_interval_ms", config.FlushIntervalMs);
        AddOptional(configObject, "max_batch_bytes", config.MaxBatchBytes);
        AddOptional(configObject, "max_buffered_records", config.MaxBufferedRecords);
        AddOptional(configObject, "include_device_id", config.IncludeDeviceId);
        AddOptional(configObject, "scrub_ids", config.ScrubIds);
        AddOptional(configObject, "mls_verbosity", config.MlsVerbosity?.ToString());
        AddOptional(configObject, "metrics_cadence_ms", config.MetricsCadenceMs);
        AddOptional(configObject, "routing_diagnostic", config.RoutingDiagnostic);
        AddOptional(configObject, "mls_sampling_bypass", config.MlsSamplingBypass);

        return JsonSerializer.Serialize(
            new Dictionary<string, object?>
            {
                ["config"] = configObject,
                ["app_state"] = appState.ToString(),
            });
    }

    private static void AddOptional<T>(Dictionary<string, object?> target, string key, T? value)
    {
        if (value is not null)
        {
            target[key] = value;
        }
    }
}
