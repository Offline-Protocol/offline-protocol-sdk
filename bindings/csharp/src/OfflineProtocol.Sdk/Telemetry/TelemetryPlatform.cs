namespace OfflineProtocol.Sdk.Telemetry;

/// <summary>
/// Host platform metadata for telemetry (C12). <c>os</c> is filled by the native library on
/// <c>enable_telemetry</c>; the binding supplies <see cref="OsMajor"/> only.
/// </summary>
public static class TelemetryPlatform
{
    /// <summary>Operating-system major version for telemetry aggregates; 0 on a platform the binding does not ship for.</summary>
    public static ushort OsMajor =>
        OperatingSystem.IsWindows() || OperatingSystem.IsMacOS() || OperatingSystem.IsLinux()
            ? (ushort)Math.Clamp(Environment.OSVersion.Version.Major, 0, ushort.MaxValue)
            : (ushort)0;
}
