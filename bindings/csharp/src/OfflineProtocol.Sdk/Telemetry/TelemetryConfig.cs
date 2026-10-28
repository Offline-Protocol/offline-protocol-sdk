namespace OfflineProtocol.Sdk.Telemetry;

/// <summary>
/// Telemetry enable parameters the app may set. <c>os</c> is never included here; the C API fills it.
/// </summary>
public sealed class TelemetryConfig
{
    public required string ApiKey { get; init; }

    public required string AppId { get; init; }

    /// <summary>Defaults to <see cref="TelemetryPlatform.OsMajor"/> when unset.</summary>
    public ushort? OsMajor { get; init; }

    public string? AppVersion { get; init; }

    public bool? Debug { get; init; }

    public ulong? FlushIntervalMs { get; init; }

    public ulong? MaxBatchBytes { get; init; }

    public uint? MaxBufferedRecords { get; init; }

    public bool? IncludeDeviceId { get; init; }

    public bool? ScrubIds { get; init; }

    public MlsVerbosity? MlsVerbosity { get; init; }

    public ulong? MetricsCadenceMs { get; init; }

    public bool? RoutingDiagnostic { get; init; }

    public bool? MlsSamplingBypass { get; init; }
}
