using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.Storage;

/// <summary>
/// Pins a managed <see cref="IProtocolStorage"/> as an <c>OfflineProtocolStorage</c> vtable for the C API.
/// Keep alive until the engine drops the provider (after <see cref="ProtocolEngine.DisposeAsync"/>).
/// </summary>
/// <remarks>
/// Single use: the native library calls <c>drop</c> once it lets go, which happens when the engine
/// is freed, when <see cref="ProtocolEngine.InitializeMlsAsync"/> is refused, and as soon as
/// <see cref="ProtocolEngine.RunStorageConformanceAsync"/> returns. Handing a dropped provider
/// to the library again throws <see cref="ObjectDisposedException"/>; wrap the same
/// <see cref="IProtocolStorage"/> in a new provider instead.
/// </remarks>
public sealed class NativeStorageProvider
{
    private readonly IProtocolStorage _storage;
    private readonly StoreFn _store;
    private readonly LoadFn _load;
    private readonly RemoveFn _remove;
    private readonly ListKeysFn _listKeys;
    private readonly ReleaseFn _release;
    private readonly DropFn _drop;

    // Not readonly: GCHandle is a struct, and Free() on a readonly field frees a defensive copy.
    private GCHandle _selfHandle;
    private int _nativeDropped;
    private int _handedOut;

    public NativeStorageProvider(IProtocolStorage storage)
    {
        _storage = storage;
        _selfHandle = GCHandle.Alloc(this);
        _store = StoreImpl;
        _load = LoadImpl;
        _remove = RemoveImpl;
        _listKeys = ListKeysImpl;
        _release = ReleaseImpl;
        _drop = DropImpl;
    }

    /// <summary>Whether the native library has let go of this provider.</summary>
    public bool IsDropped => Volatile.Read(ref _nativeDropped) != 0;

    /// <summary>
    /// Throws when <see cref="ToNative"/> would, without handing anything out. Lets a call that
    /// takes two providers refuse before the first is given away and stranded.
    /// </summary>
    internal void ThrowIfHandedOut()
    {
        if (Volatile.Read(ref _handedOut) != 0 || IsDropped)
        {
            throw new ObjectDisposedException(
                nameof(NativeStorageProvider),
                "This provider was already handed to the native library, which drops it; wrap the storage in a new NativeStorageProvider.");
        }
    }

    /// <summary>
    /// The vtable for one native call that takes ownership. Throws once the provider was handed out
    /// before, because the library drops what it was given and the context handle dies with it.
    /// </summary>
    internal OfflineProtocolStorage ToNative()
    {
        if (Interlocked.CompareExchange(ref _handedOut, 1, 0) != 0 || IsDropped)
        {
            throw new ObjectDisposedException(
                nameof(NativeStorageProvider),
                "This provider was already handed to the native library, which drops it; wrap the storage in a new NativeStorageProvider.");
        }

        return new()
        {
            Ctx = GCHandle.ToIntPtr(_selfHandle),
            Store = Marshal.GetFunctionPointerForDelegate(_store),
            Load = Marshal.GetFunctionPointerForDelegate(_load),
            Remove = Marshal.GetFunctionPointerForDelegate(_remove),
            ListKeys = Marshal.GetFunctionPointerForDelegate(_listKeys),
            Release = Marshal.GetFunctionPointerForDelegate(_release),
            Drop = Marshal.GetFunctionPointerForDelegate(_drop),
        };
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int StoreFn(IntPtr ctx, IntPtr keyType, IntPtr keyId, IntPtr data, UIntPtr len);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int LoadFn(
        IntPtr ctx,
        IntPtr keyType,
        IntPtr keyId,
        out IntPtr outData,
        out UIntPtr outLen);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int RemoveFn(IntPtr ctx, IntPtr keyType, IntPtr keyId);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int ListKeysFn(IntPtr ctx, IntPtr keyType, out IntPtr outJson, out UIntPtr outLen);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void ReleaseFn(IntPtr ctx, IntPtr data, UIntPtr len);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void DropFn(IntPtr ctx);

    private static NativeStorageProvider FromCtx(IntPtr ctx) =>
        (NativeStorageProvider)GCHandle.FromIntPtr(ctx).Target!;

    private static string Utf8(IntPtr ptr) => Marshal.PtrToStringUTF8(ptr) ?? "";

    private static int StoreImpl(IntPtr ctx, IntPtr keyType, IntPtr keyId, IntPtr data, UIntPtr len)
    {
        try
        {
            var self = FromCtx(ctx);
            var length = (int)len;
            var bytes = length > 0 ? new byte[length] : Array.Empty<byte>();
            if (length > 0)
            {
                Marshal.Copy(data, bytes, 0, length);
            }

            return (int)self._storage.Store(Utf8(keyType), Utf8(keyId), bytes);
        }
        catch
        {
            return (int)OfflineProtocolStorageStatus.StoreFailed;
        }
    }

    private static int LoadImpl(
        IntPtr ctx,
        IntPtr keyType,
        IntPtr keyId,
        out IntPtr outData,
        out UIntPtr outLen)
    {
        outData = IntPtr.Zero;
        outLen = UIntPtr.Zero;
        try
        {
            var self = FromCtx(ctx);
            var loaded = self._storage.Load(Utf8(keyType), Utf8(keyId));
            if (loaded is null)
            {
                return (int)OfflineProtocolStorageStatus.Ok;
            }

            outData = Marshal.AllocHGlobal(loaded.Length);
            if (loaded.Length > 0)
            {
                Marshal.Copy(loaded, 0, outData, loaded.Length);
            }

            outLen = (UIntPtr)(uint)loaded.Length;
            return (int)OfflineProtocolStorageStatus.Ok;
        }
        catch
        {
            return (int)OfflineProtocolStorageStatus.LoadFailed;
        }
    }

    private static int RemoveImpl(IntPtr ctx, IntPtr keyType, IntPtr keyId)
    {
        try
        {
            return (int)FromCtx(ctx)._storage.Remove(Utf8(keyType), Utf8(keyId));
        }
        catch
        {
            return (int)OfflineProtocolStorageStatus.DeleteFailed;
        }
    }

    private static int ListKeysImpl(IntPtr ctx, IntPtr keyType, out IntPtr outJson, out UIntPtr outLen)
    {
        outJson = IntPtr.Zero;
        outLen = UIntPtr.Zero;
        try
        {
            var self = FromCtx(ctx);
            var type = Utf8(keyType);
            var json = JsonSerializer.Serialize(self._storage.ListKeyIds(type));
            var bytes = Encoding.UTF8.GetBytes(json);
            outJson = Marshal.AllocHGlobal(bytes.Length);
            if (bytes.Length > 0)
            {
                Marshal.Copy(bytes, 0, outJson, bytes.Length);
            }

            outLen = (UIntPtr)(uint)bytes.Length;
            return (int)OfflineProtocolStorageStatus.Ok;
        }
        catch
        {
            return (int)OfflineProtocolStorageStatus.LoadFailed;
        }
    }

    private static void ReleaseImpl(IntPtr ctx, IntPtr data, UIntPtr len)
    {
        _ = ctx;
        _ = len;
        if (data != IntPtr.Zero)
        {
            Marshal.FreeHGlobal(data);
        }
    }

    private static void DropImpl(IntPtr ctx)
    {
        try
        {
            if (GCHandle.FromIntPtr(ctx).Target is NativeStorageProvider self)
            {
                self.OnNativeDrop();
            }
        }
        catch
        {
            // An exception escaping a reverse P/Invoke terminates the process.
        }
    }

    private void OnNativeDrop()
    {
        if (Interlocked.CompareExchange(ref _nativeDropped, 1, 0) != 0)
        {
            return;
        }

        _selfHandle.Free();
    }
}
