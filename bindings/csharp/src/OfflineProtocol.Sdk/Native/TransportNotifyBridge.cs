using System.Runtime.InteropServices;

namespace OfflineProtocol.Sdk.Native;

/// <summary>
/// Native registration for one transport carrier; delivery runs on <see cref="TransportNotifyHub"/>.
/// </summary>
internal sealed class TransportNotifyBridge
{
    private readonly TransportNotifyHub _hub;
    private readonly int _transportIndex;
    private readonly NotifyFn _onNotify;
    private readonly DropFn _onDrop;
    private readonly IntPtr _notifyPointer;
    private readonly IntPtr _dropPointer;

    // Not readonly: GCHandle is a struct, and Free() on a readonly field frees a defensive
    // copy, leaving IsAllocated true here. The flag below is what makes the free happen once.
    private GCHandle _selfHandle;
    private int _handleFreed;
    private int _accepting;
    private int _teardownFinished;

    public TransportNotifyBridge(
        Action handler,
        OfflineProtocolTransport transport,
        TransportNotifyHub hub)
    {
        _hub = hub;
        _transportIndex = (int)transport;
        _selfHandle = GCHandle.Alloc(this);
        _onNotify = OnNotify;
        _onDrop = OnDrop;
        _notifyPointer = Marshal.GetFunctionPointerForDelegate(_onNotify);
        _dropPointer = Marshal.GetFunctionPointerForDelegate(_onDrop);
        _accepting = 1;
        _hub.ActivateCarrier(_transportIndex, handler);
    }

    public IntPtr Context => GCHandle.ToIntPtr(_selfHandle);

    public IntPtr NotifyPointer => _notifyPointer;

    public IntPtr DropPointer => _dropPointer;

    internal long DroppedNotifyCount => _hub.DroppedNotifyCount(_transportIndex);

    /// <summary>Engine-initiated removal; clears the hub slot for this carrier.</summary>
    internal void Deactivate()
    {
        if (!BeginTeardown())
        {
            return;
        }

        _hub.DeactivateCarrier(_transportIndex);
    }

    internal void ReleaseLocal()
    {
        Deactivate();
        FreeHandleOnce();
    }

    internal static string DeliveryThreadName(OfflineProtocolTransport transport) =>
        TransportNotifyHub.DeliveryThreadName;

    /// <summary>Invokes the native notify entry point without cross-assembly delegate marshaling (tests only).</summary>
    internal void SimulateNativeNotifyForTests() => OnNotify(Context);

    /// <summary>Invokes the native drop entry point without cross-assembly delegate marshaling (tests only).</summary>
    internal void SimulateNativeDropForTests() => OnDrop(Context);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void NotifyFn(IntPtr ctx);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void DropFn(IntPtr ctx);

    private static void OnNotify(IntPtr ctx)
    {
        if (GCHandle.FromIntPtr(ctx).Target is not TransportNotifyBridge bridge)
        {
            return;
        }

        if (Volatile.Read(ref bridge._accepting) == 0)
        {
            return;
        }

        bridge._hub.PostNotify(bridge._transportIndex);
    }

    private static void OnDrop(IntPtr ctx)
    {
        if (GCHandle.FromIntPtr(ctx).Target is not TransportNotifyBridge bridge)
        {
            return;
        }

        // Native replaced this registration; do not touch the hub (the new bridge may own the carrier).
        bridge.EndNativeLifetime();
    }

    private bool BeginTeardown()
    {
        if (Interlocked.CompareExchange(ref _teardownFinished, 1, 0) != 0)
        {
            return false;
        }

        Volatile.Write(ref _accepting, 0);
        return true;
    }

    private void EndNativeLifetime()
    {
        BeginTeardown();
        FreeHandleOnce();
    }

    private void FreeHandleOnce()
    {
        if (Interlocked.CompareExchange(ref _handleFreed, 1, 0) == 0)
        {
            _selfHandle.Free();
        }
    }
}
