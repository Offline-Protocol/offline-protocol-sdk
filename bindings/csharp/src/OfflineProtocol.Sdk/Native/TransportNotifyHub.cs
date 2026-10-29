using System.Threading.Channels;

namespace OfflineProtocol.Sdk.Native;

/// <summary>
/// One delivery thread per engine; each carrier has its own bounded queue (CS7).
/// </summary>
internal sealed class TransportNotifyHub
{
    /// <summary>
    /// One slot per <see cref="OfflineProtocolTransport"/> member, so a member appended to the C
    /// API (and so to the enum, which <c>the_csharp_transport_enum_is_the_c_apis</c> holds) gets a
    /// queue without a second number to update.
    /// </summary>
    public static readonly int CarrierCount = Enum.GetValues<OfflineProtocolTransport>().Length;
    public const int DefaultPerCarrierQueueCapacity = 256;
    public const string DeliveryThreadName = "offline-protocol-transport-notify";

    private readonly Channel<int>[] _generationChannels;
    private readonly ChannelWriter<int>[] _generationWriters;
    private readonly ChannelReader<int>[] _generationReaders;
    private readonly Channel<byte> _wakeChannel;
    private readonly ChannelWriter<byte> _wakeWriter;
    private readonly ChannelReader<byte> _wakeReader;
    private readonly Action?[] _handlers = new Action?[CarrierCount];
    private readonly int[] _accepting = new int[CarrierCount];
    private readonly int[] _deliveryGeneration = new int[CarrierCount];
    private readonly long[] _droppedNotifyCount = new long[CarrierCount];
    private readonly long[] _handlerFailureCount = new long[CarrierCount];
    private readonly Thread _deliveryThread;
    private int _teardownFinished;

    public TransportNotifyHub(int perCarrierQueueCapacity = DefaultPerCarrierQueueCapacity)
    {
        _generationChannels = new Channel<int>[CarrierCount];
        _generationWriters = new ChannelWriter<int>[CarrierCount];
        _generationReaders = new ChannelReader<int>[CarrierCount];
        for (var i = 0; i < CarrierCount; i++)
        {
            var channel = Channel.CreateBounded<int>(
                new BoundedChannelOptions(perCarrierQueueCapacity)
                {
                    FullMode = BoundedChannelFullMode.DropWrite,
                    SingleReader = true,
                    SingleWriter = false,
                });
            _generationChannels[i] = channel;
            _generationWriters[i] = channel.Writer;
            _generationReaders[i] = channel.Reader;
            _deliveryGeneration[i] = 1;
        }

        _wakeChannel = Channel.CreateBounded<byte>(
            new BoundedChannelOptions(1)
            {
                FullMode = BoundedChannelFullMode.DropWrite,
                SingleReader = true,
                SingleWriter = false,
            });
        _wakeWriter = _wakeChannel.Writer;
        _wakeReader = _wakeChannel.Reader;

        _deliveryThread = new Thread(RunDeliveryLoop)
        {
            Name = DeliveryThreadName,
            IsBackground = true,
        };
        _deliveryThread.Start();
    }

    /// <remarks>
    /// The generation is left where <see cref="DeactivateCarrier"/> moved it. Resetting it here
    /// would make a stamp the replaced registration queued match the new one, and wake the new
    /// handler for a notify it never received.
    /// </remarks>
    internal void ActivateCarrier(int transportIndex, Action handler)
    {
        _handlers[transportIndex] = handler;
        Volatile.Write(ref _accepting[transportIndex], 1);
    }

    internal void DeactivateCarrier(int transportIndex)
    {
        Volatile.Write(ref _accepting[transportIndex], 0);
        Interlocked.Increment(ref _deliveryGeneration[transportIndex]);
        _handlers[transportIndex] = null;
    }

    internal void PostNotify(int transportIndex)
    {
        if ((uint)transportIndex >= (uint)CarrierCount)
        {
            return;
        }

        if (Volatile.Read(ref _accepting[transportIndex]) == 0)
        {
            return;
        }

        var generation = Volatile.Read(ref _deliveryGeneration[transportIndex]);
        if (!_generationWriters[transportIndex].TryWrite(generation))
        {
            Interlocked.Increment(ref _droppedNotifyCount[transportIndex]);
            return;
        }

        _wakeWriter.TryWrite(0);
    }

    internal long DroppedNotifyCount(int transportIndex) =>
        (uint)transportIndex < (uint)CarrierCount
            ? Interlocked.Read(ref _droppedNotifyCount[transportIndex])
            : 0;

    /// <summary>Handler runs for this carrier that threw. Delivery continues after a throw.</summary>
    internal long HandlerFailureCount(int transportIndex) =>
        (uint)transportIndex < (uint)CarrierCount
            ? Interlocked.Read(ref _handlerFailureCount[transportIndex])
            : 0;

    internal void Deactivate()
    {
        if (Interlocked.CompareExchange(ref _teardownFinished, 1, 0) != 0)
        {
            return;
        }

        for (var i = 0; i < CarrierCount; i++)
        {
            DeactivateCarrier(i);
            _generationWriters[i].TryComplete();
        }

        _wakeWriter.TryComplete();

        // A handler that disposes the engine from the delivery thread would otherwise join itself.
        if (Thread.CurrentThread != _deliveryThread)
        {
            _deliveryThread.Join();
        }
    }

    private void RunDeliveryLoop()
    {
        try
        {
            while (_wakeReader.WaitToReadAsync().AsTask().GetAwaiter().GetResult())
            {
                while (_wakeReader.TryRead(out _))
                {
                }

                for (var i = 0; i < CarrierCount; i++)
                {
                    DeliverCoalescedForCarrier(i);
                }
            }
        }
        catch (ChannelClosedException)
        {
        }
    }

    /// <summary>
    /// Coalesces queued notifies for one carrier; generation and accepting are re-read at delivery time.
    /// </summary>
    private void DeliverCoalescedForCarrier(int carrierIndex)
    {
        var deliver = false;
        var matchedGeneration = -1;

        while (_generationReaders[carrierIndex].TryRead(out var stampedGeneration))
        {
            if (Volatile.Read(ref _accepting[carrierIndex]) == 0)
            {
                continue;
            }

            if (stampedGeneration != Volatile.Read(ref _deliveryGeneration[carrierIndex]))
            {
                continue;
            }

            deliver = true;
            matchedGeneration = stampedGeneration;
        }

        if (!deliver)
        {
            return;
        }

        if (Volatile.Read(ref _accepting[carrierIndex]) == 0)
        {
            return;
        }

        if (matchedGeneration != Volatile.Read(ref _deliveryGeneration[carrierIndex]))
        {
            return;
        }

        var handler = _handlers[carrierIndex];
        if (handler is null)
        {
            return;
        }

        try
        {
            handler();
        }
        catch
        {
            // An exception escaping a dedicated thread terminates the process. App code must not
            // be able to do that through a transport handler, any more than through the event one.
            Interlocked.Increment(ref _handlerFailureCount[carrierIndex]);
        }
    }
}
