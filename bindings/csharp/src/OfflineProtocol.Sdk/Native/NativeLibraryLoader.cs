using System.Reflection;
using System.Runtime.InteropServices;

namespace OfflineProtocol.Sdk.Native;

/// <summary>
/// Loads <c>offline_protocol_uniffi</c> from <c>OFFLINE_PROTOCOL_LIBRARY_DIR</c> or the process path.
/// </summary>
internal static class NativeLibraryLoader
{
    private static readonly object Gate = new();
    private static bool _registered;

    internal static void EnsureRegistered()
    {
        lock (Gate)
        {
            if (_registered)
            {
                return;
            }

            NativeLibrary.SetDllImportResolver(
                typeof(OfflineProtocolNative).Assembly,
                ResolveOfflineProtocol);

            _ = NativeLibrary.Load(ResolvePath());
            _registered = true;
        }
    }

    private static IntPtr ResolveOfflineProtocol(string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (libraryName != OfflineProtocolNative.LibraryName)
        {
            return IntPtr.Zero;
        }

        return NativeLibrary.Load(ResolvePath());
    }

    private static string ResolvePath()
    {
        var fromEnv = Environment.GetEnvironmentVariable("OFFLINE_PROTOCOL_LIBRARY_DIR");
        if (!string.IsNullOrWhiteSpace(fromEnv))
        {
            var name = OfflineProtocolNative.PlatformLibraryFileName;
            var candidate = Path.Combine(fromEnv, name);
            if (File.Exists(candidate))
            {
                return candidate;
            }

            throw new FileNotFoundException(
                $"Native library not found at {candidate}. Set OFFLINE_PROTOCOL_LIBRARY_DIR to the directory " +
                $"containing {name} (build with: cargo build --release -p offline-protocol-uniffi).");
        }

        return OfflineProtocolNative.LibraryName;
    }
}
