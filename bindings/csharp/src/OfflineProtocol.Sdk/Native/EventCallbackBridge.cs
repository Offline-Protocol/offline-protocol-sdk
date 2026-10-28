using System.Runtime.InteropServices;
using System.Threading.Channels;

namespace OfflineProtocol.Sdk.Native;

/// <summary>
/// Forwards engine events to app code off the engine thread (CS1 / C K2).
/// </summary>
internal sealed class EventCallbackBridge
{
    public const int DefaultEventQueueCapacity = 256;

    private readonly Action<string> _handler;
    private readonly GCHandle _selfHandle;
    private readonly EventFn _onEvent;
    private readonly DropFn _onDrop;
    private readonly Channel<string> _channel;
    private readonly ChannelWriter<string> _writer;
    private readonly ChannelReader<string> _reader;
    private readonly Thread _deliveryThread;
    private int _accepting;
    private int _teardownFinished;

    public EventCallbackBridge(Action<string> handler, int queueCapacity = DefaultEventQueueCapacity)
    {
        _handler = handler;
        _selfHandle = GCHandle.Alloc(this);
        _onEvent = OnEvent;
        _onDrop = OnDrop;
        _channel = Channel.CreateBounded<string>(
            new BoundedChannelOptions(queueCapacity)
            {
                FullMode = BoundedChannelFullMode.DropWrite,
                SingleReader = true,
                SingleWriter = false,
            });
        _writer = _channel.Writer;
        _reader = _channel.Reader;
        _accepting = 1;
        _deliveryThread = new Thread(RunDeliveryLoop)
        {
            Name = "offline-protocol-events",
            IsBackground = true,
        };
        _deliveryThread.Start();
    }

    public IntPtr Context => GCHandle.ToIntPtr(_selfHandle);

    public IntPtr OnEventPointer => Marshal.GetFunctionPointerForDelegate(_onEvent);

    public IntPtr DropPointer => Marshal.GetFunctionPointerForDelegate(_onDrop);

    /// <summary>Stops accepting and delivering events; waits for the delivery thread to exit.</summary>
    internal void Deactivate()
    {
        if (Interlocked.CompareExchange(ref _teardownFinished, 1, 0) != 0)
        {
            return;
        }

        Volatile.Write(ref _accepting, 0);
        _writer.TryComplete();
        _deliveryThread.Join();
    }

    /// <summary>When native never took ownership (install refused).</summary>
    internal void ReleaseLocal()
    {
        Deactivate();
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
        if (GCHandle.FromIntPtr(ctx).Target is not EventCallbackBridge bridge)
        {
            return;
        }

        if (Volatile.Read(ref bridge._accepting) == 0)
        {
            return;
        }

        var json = Marshal.PtrToStringUTF8(eventJson) ?? "";
        bridge._writer.TryWrite(json);
    }

    private void RunDeliveryLoop()
    {
        try
        {
            while (_reader.WaitToReadAsync().AsTask().GetAwaiter().GetResult())
            {
                while (_reader.TryRead(out var json))
                {
                    if (Volatile.Read(ref _accepting) == 0)
                    {
                        continue;
                    }

                    _handler(json);
                }
            }
        }
        catch (ChannelClosedException)
        {
        }
    }

    private static void OnDrop(IntPtr ctx)
    {
        if (GCHandle.FromIntPtr(ctx).Target is EventCallbackBridge bridge)
        {
            bridge.Deactivate();
            if (bridge._selfHandle.IsAllocated)
            {
                bridge._selfHandle.Free();
            }
        }
    }
}
