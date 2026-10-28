namespace OfflineProtocol.Sdk.Telemetry;

/// <summary>
/// Host platform metadata for telemetry (C12). <c>os</c> is filled by the native library on
/// <c>enable_telemetry</c>; the binding supplies <see cref="OsMajor"/> only.
/// </summary>
public static class TelemetryPlatform
{
    /// <summary>Operating-system major version for telemetry aggregates.</summary>
    public static ushort OsMajor
    {
        get
        {
            if (OperatingSystem.IsWindows())
            {
                return (ushort)Math.Clamp(Environment.OSVersion.Version.Major, 0, ushort.MaxValue);
            }

            if (OperatingSystem.IsMacOS() || OperatingSystem.IsLinux())
            {
                return (ushort)Math.Clamp(Environment.OSVersion.Version.Major, 0, ushort.MaxValue);
            }

            return 0;
        }
    }
}
