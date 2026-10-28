using System.Runtime.InteropServices;

namespace OfflineProtocol.Sdk.Native;

/// <summary>
/// Forwards engine events to app code off the engine thread (CS1 / C K2).
/// </summary>
internal sealed class EventCallbackBridge
{
    private readonly Action<string> _handler;
    private readonly GCHandle _selfHandle;
    private readonly EventFn _onEvent;
    private readonly DropFn _onDrop;

    public EventCallbackBridge(Action<string> handler)
    {
        _handler = handler;
        _selfHandle = GCHandle.Alloc(this);
        _onEvent = OnEvent;
        _onDrop = OnDrop;
    }

    public IntPtr Context => GCHandle.ToIntPtr(_selfHandle);

    public IntPtr OnEventPointer => Marshal.GetFunctionPointerForDelegate(_onEvent);

    public IntPtr DropPointer => Marshal.GetFunctionPointerForDelegate(_onDrop);

    /// <summary>When native never took ownership (install refused).</summary>
    internal void ReleaseLocal()
    {
        if (_selfHandle.IsAllocated)
        {
            _selfHandle.Free();
        }
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void EventFn(IntPtr ctx, IntPtr eventJson);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void DropFn(IntPtr ctx);

    private static void OnEvent(IntPtr ctx, IntPtr eventJson)
    {
        var json = Marshal.PtrToStringUTF8(eventJson) ?? "";
        if (GCHandle.FromIntPtr(ctx).Target is EventCallbackBridge bridge)
        {
            ThreadPool.QueueUserWorkItem(_ => bridge._handler(json));
        }
    }

    private static void OnDrop(IntPtr ctx)
    {
        if (GCHandle.FromIntPtr(ctx).Target is EventCallbackBridge bridge)
        {
            if (bridge._selfHandle.IsAllocated)
            {
                bridge._selfHandle.Free();
            }
        }
    }
}
