using System.Runtime.InteropServices;

namespace OfflineProtocol.Sdk.Native;

internal static partial class OfflineProtocolNative
{
    internal const string LibraryName = "offline_protocol_uniffi";

    internal static string PlatformLibraryFileName =>
        OperatingSystem.IsWindows() ? "offline_protocol_uniffi.dll"
        : OperatingSystem.IsMacOS() ? "liboffline_protocol_uniffi.dylib"
        : "liboffline_protocol_uniffi.so";

    [LibraryImport(LibraryName, EntryPoint = "offline_protocol_version", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial IntPtr offline_protocol_version_raw();

    [LibraryImport(LibraryName, EntryPoint = "offline_protocol_create", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial OfflineProtocolStatus offline_protocol_create(
        string configJson,
        out IntPtr outHandle,
        ref OfflineProtocolError err);

    [LibraryImport(LibraryName, EntryPoint = "offline_protocol_free")]
    internal static partial void offline_protocol_free(IntPtr handle);

    [LibraryImport(LibraryName, EntryPoint = "offline_protocol_call", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial OfflineProtocolStatus offline_protocol_call(
        IntPtr handle,
        string method,
        string? paramsJson,
        out IntPtr outResult,
        ref OfflineProtocolError err);

    [LibraryImport(LibraryName, EntryPoint = "offline_protocol_string_free")]
    internal static partial void offline_protocol_string_free(IntPtr s);

    [LibraryImport(LibraryName, EntryPoint = "offline_protocol_error_clear")]
    internal static partial void offline_protocol_error_clear(ref OfflineProtocolError err);

    internal static string Version()
    {
        NativeLibraryLoader.EnsureRegistered();
        var ptr = offline_protocol_version_raw();
        return Marshal.PtrToStringUTF8(ptr) ?? "";
    }

    internal static string TakeUtf8String(IntPtr ptr)
    {
        if (ptr == IntPtr.Zero)
        {
            return "";
        }

        try
        {
            return Marshal.PtrToStringUTF8(ptr) ?? "";
        }
        finally
        {
            offline_protocol_string_free(ptr);
        }
    }
}
