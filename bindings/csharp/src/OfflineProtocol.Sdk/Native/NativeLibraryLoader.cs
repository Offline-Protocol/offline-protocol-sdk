using System.Reflection;
using System.Runtime.InteropServices;

namespace OfflineProtocol.Sdk.Native;

/// <summary>
/// Resolves <c>offline_protocol_uniffi</c> from <c>OFFLINE_PROTOCOL_LIBRARY_DIR</c> when it is set,
/// and otherwise leaves it to the runtime's own probing (CS3).
/// </summary>
/// <remarks>
/// The fallback must be the runtime's, not <see cref="NativeLibrary.Load(string)"/> with the bare
/// name: that overload is a plain <c>dlopen</c> or <c>LoadLibrary</c> with no <c>lib</c> prefix and
/// no <c>.dylib</c> or <c>.so</c> suffix, so it finds the library on Windows and fails on macOS and
/// Linux even with the library beside the application.
/// </remarks>
internal static class NativeLibraryLoader
{
    internal const string LibraryDirVariable = "OFFLINE_PROTOCOL_LIBRARY_DIR";

    private static readonly object Gate = new();
    private static bool _registered;

    internal static void EnsureRegistered()
    {
        lock (Gate)
        {
            if (!_registered)
            {
                // Marked before anything that can throw: the runtime refuses a second resolver
                // for the same assembly, so a retry after a failure must not install one again.
                _registered = true;
                NativeLibrary.SetDllImportResolver(typeof(OfflineProtocolNative).Assembly, Resolve);
            }
        }

        // A directory that is set and does not hold the library is a configuration error the
        // host should see by name here, not as an unresolved entry point on the first call.
        _ = ExplicitPath();
    }

    private static IntPtr Resolve(string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (libraryName != OfflineProtocolNative.LibraryName)
        {
            return IntPtr.Zero;
        }

        var path = ExplicitPath();
        // Zero hands the name back to the runtime, which probes the application directory and
        // the system paths with the platform's prefix and suffix.
        return path is null ? IntPtr.Zero : NativeLibrary.Load(path);
    }

    /// <summary>The library under <c>OFFLINE_PROTOCOL_LIBRARY_DIR</c>, or null when it is unset.</summary>
    private static string? ExplicitPath()
    {
        var dir = Environment.GetEnvironmentVariable(LibraryDirVariable);
        if (string.IsNullOrWhiteSpace(dir))
        {
            return null;
        }

        var name = OfflineProtocolNative.PlatformLibraryFileName;
        var candidate = Path.Combine(dir, name);
        if (File.Exists(candidate))
        {
            return candidate;
        }

        throw new FileNotFoundException(
            $"Native library not found at {candidate}. Set {LibraryDirVariable} to the directory " +
            $"containing {name} (build with: cargo build --release -p offline-protocol-uniffi), " +
            "or unset it to load the library from the application directory.",
            candidate);
    }
}
