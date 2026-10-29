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
    private readonly EventFn _onEvent;
    private readonly DropFn _onDrop;
    private readonly Channel<EventEnvelope> _channel;
    private readonly ChannelWriter<EventEnvelope> _writer;
    private readonly ChannelReader<EventEnvelope> _reader;
    private readonly Thread _deliveryThread;

    // Not readonly: GCHandle is a struct, and Free() on a readonly field frees a defensive
    // copy, leaving IsAllocated true here. The flag below is what makes the free happen once.
    private GCHandle _selfHandle;
    private int _handleFreed;
    private int _accepting;
    private int _deliveryGeneration;
    private int _teardownFinished;
    private long _droppedEventCount;
    private long _handlerFailureCount;

    public EventCallbackBridge(Action<string> handler, int queueCapacity = DefaultEventQueueCapacity)
    {
        _handler = handler;
        _selfHandle = GCHandle.Alloc(this);
        _onEvent = OnEvent;
        _onDrop = OnDrop;
        _channel = Channel.CreateBounded<EventEnvelope>(
            new BoundedChannelOptions(queueCapacity)
            {
                FullMode = BoundedChannelFullMode.DropWrite,
                SingleReader = true,
                SingleWriter = false,
            });
        _writer = _channel.Writer;
        _reader = _channel.Reader;
        _accepting = 1;
        _deliveryGeneration = 1;
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

    /// <summary>Events not delivered because the bounded queue rejected a write.</summary>
    internal long DroppedEventCount => Interlocked.Read(ref _droppedEventCount);

    /// <summary>Events whose handler threw. Delivery continues after a throw.</summary>
    internal long HandlerFailureCount => Interlocked.Read(ref _handlerFailureCount);

    /// <summary>Stops accepting and delivering events; waits for the delivery thread to exit.</summary>
    internal void Deactivate()
    {
        if (Interlocked.CompareExchange(ref _teardownFinished, 1, 0) != 0)
        {
            return;
        }

        Volatile.Write(ref _accepting, 0);
        Interlocked.Increment(ref _deliveryGeneration);
        _writer.TryComplete();

        // A handler that disposes the engine from the delivery thread would otherwise join itself.
        if (Thread.CurrentThread != _deliveryThread)
        {
            _deliveryThread.Join();
        }
    }

    /// <summary>
    /// Releases the bridge when the install call returned. Safe after a refused install too:
    /// the C API calls <c>drop</c> on every refusal, and the handle is freed only once.
    /// </summary>
    internal void ReleaseLocal()
    {
        Deactivate();
        FreeHandleOnce();
    }

    private void FreeHandleOnce()
    {
        if (Interlocked.CompareExchange(ref _handleFreed, 1, 0) == 0)
        {
            _selfHandle.Free();
        }
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void EventFn(IntPtr ctx, IntPtr eventJson);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void DropFn(IntPtr ctx);

    private readonly record struct EventEnvelope(int Generation, string Json);

    // Runs on the engine thread inside the native library. An exception escaping a reverse
    // P/Invoke terminates the process, so nothing may leave this method.
    private static void OnEvent(IntPtr ctx, IntPtr eventJson)
    {
        try
        {
            if (GCHandle.FromIntPtr(ctx).Target is not EventCallbackBridge bridge)
            {
                return;
            }

            if (Volatile.Read(ref bridge._accepting) == 0)
            {
                return;
            }

            var generation = Volatile.Read(ref bridge._deliveryGeneration);
            var json = Marshal.PtrToStringUTF8(eventJson) ?? "";
            if (!bridge._writer.TryWrite(new EventEnvelope(generation, json)))
            {
                Interlocked.Increment(ref bridge._droppedEventCount);
            }
        }
        catch
        {
            // Dropping one event is the only safe outcome on the engine thread.
        }
    }

    private void RunDeliveryLoop()
    {
        try
        {
            while (_reader.WaitToReadAsync().AsTask().GetAwaiter().GetResult())
            {
                while (_reader.TryRead(out var envelope))
                {
                    if (envelope.Generation != Volatile.Read(ref _deliveryGeneration))
                    {
                        continue;
                    }

                    try
                    {
                        _handler(envelope.Json);
                    }
                    catch
                    {
                        // An exception escaping a dedicated thread terminates the process. App
                        // code must not be able to do that through the event handler.
                        Interlocked.Increment(ref _handlerFailureCount);
                    }
                }
            }
        }
        catch (ChannelClosedException)
        {
        }
    }

    private static void OnDrop(IntPtr ctx)
    {
        try
        {
            if (GCHandle.FromIntPtr(ctx).Target is EventCallbackBridge bridge)
            {
                bridge.Deactivate();
                bridge.FreeHandleOnce();
            }
        }
        catch
        {
            // Same reverse P/Invoke rule as OnEvent.
        }
    }
}
