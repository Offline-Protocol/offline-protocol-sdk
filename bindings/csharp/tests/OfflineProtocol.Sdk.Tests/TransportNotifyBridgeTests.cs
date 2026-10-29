using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.Tests;

public sealed class TransportNotifyBridgeTests
{
    [Fact]
    public void Deactivate_stops_delivery()
    {
        var invocations = 0;
        using var delivered = new ManualResetEventSlim(false);
        var hub = new TransportNotifyHub();
        try
        {
            var bridge = new TransportNotifyBridge(
                () =>
                {
                    Interlocked.Increment(ref invocations);
                    delivered.Set();
                },
                OfflineProtocolTransport.Ble,
                hub);
            bridge.Deactivate();
            bridge.SimulateNativeNotifyForTests();
            Assert.False(delivered.Wait(TimeSpan.FromMilliseconds(200)));
            Assert.Equal(0, invocations);
            bridge.ReleaseLocal();
        }
        finally
        {
            hub.Deactivate();
        }
    }

    [Fact]
    public void Notify_delivers_off_callback_thread()
    {
        var callbackThread = (Thread?)null;
        using var delivered = new ManualResetEventSlim(false);
        var hub = new TransportNotifyHub();
        try
        {
            var bridge = new TransportNotifyBridge(
                () =>
                {
                    callbackThread = Thread.CurrentThread;
                    delivered.Set();
                },
                OfflineProtocolTransport.Ble,
                hub);
            bridge.SimulateNativeNotifyForTests();
            Assert.True(delivered.Wait(TimeSpan.FromSeconds(5)));
            Assert.NotNull(callbackThread);
            Assert.NotEqual(Thread.CurrentThread, callbackThread);
            Assert.Equal(TransportNotifyHub.DeliveryThreadName, callbackThread.Name);
            bridge.ReleaseLocal();
        }
        finally
        {
            hub.Deactivate();
        }
    }

    [Fact]
    public void Notify_coalesces_burst_into_one_handler_run()
    {
        var invocations = 0;
        using var delivered = new ManualResetEventSlim(false);
        var hub = new TransportNotifyHub();
        try
        {
            var bridge = new TransportNotifyBridge(
                () =>
                {
                    Interlocked.Increment(ref invocations);
                    delivered.Set();
                },
                OfflineProtocolTransport.Ble,
                hub);
            bridge.SimulateNativeNotifyForTests();
            bridge.SimulateNativeNotifyForTests();
            bridge.SimulateNativeNotifyForTests();
            Assert.True(delivered.Wait(TimeSpan.FromSeconds(5)));
            Assert.Equal(1, invocations);
            bridge.ReleaseLocal();
        }
        finally
        {
            hub.Deactivate();
        }
    }

    [Fact]
    public void Shared_hub_delivers_multiple_carriers_on_one_thread()
    {
        var ble = 0;
        var nostr = 0;
        Thread? deliveryThread = null;
        var hub = new TransportNotifyHub();
        try
        {
            var bleBridge = new TransportNotifyBridge(
                () =>
                {
                    Interlocked.Increment(ref ble);
                    deliveryThread ??= Thread.CurrentThread;
                },
                OfflineProtocolTransport.Ble,
                hub);
            var nostrBridge = new TransportNotifyBridge(
                () =>
                {
                    Interlocked.Increment(ref nostr);
                    deliveryThread ??= Thread.CurrentThread;
                },
                OfflineProtocolTransport.Nostr,
                hub);
            bleBridge.SimulateNativeNotifyForTests();
            nostrBridge.SimulateNativeNotifyForTests();
            var deadline = Environment.TickCount64 + 5_000;
            while ((ble == 0 || nostr == 0) && Environment.TickCount64 < deadline)
            {
                Thread.Sleep(10);
            }

            Assert.Equal(1, ble);
            Assert.Equal(1, nostr);
            Assert.NotNull(deliveryThread);
            bleBridge.ReleaseLocal();
            nostrBridge.ReleaseLocal();
        }
        finally
        {
            hub.Deactivate();
        }
    }

    [Fact]
    public void Delivery_thread_name_is_shared_across_carriers()
    {
        Assert.Equal(TransportNotifyHub.DeliveryThreadName, TransportNotifyBridge.DeliveryThreadName(OfflineProtocolTransport.Nostr));
    }

    [Fact]
    public void Carrier_handler_swap_delivers_second_handler_when_previous_deactivated_first()
    {
        var first = 0;
        var second = 0;
        using var delivered = new ManualResetEventSlim(false);
        var hub = new TransportNotifyHub();
        try
        {
            var firstBridge = new TransportNotifyBridge(
                () => Interlocked.Increment(ref first),
                OfflineProtocolTransport.Ble,
                hub);
            firstBridge.Deactivate();
            firstBridge.ReleaseLocal();

            var secondBridge = new TransportNotifyBridge(
                () =>
                {
                    Interlocked.Increment(ref second);
                    delivered.Set();
                },
                OfflineProtocolTransport.Ble,
                hub);
            secondBridge.SimulateNativeNotifyForTests();
            Assert.True(delivered.Wait(TimeSpan.FromSeconds(5)));
            Assert.Equal(0, first);
            Assert.Equal(1, second);
            secondBridge.ReleaseLocal();
        }
        finally
        {
            hub.Deactivate();
        }
    }

    [Fact]
    public void Native_drop_does_not_clear_replacement_handler_on_hub()
    {
        var first = 0;
        var second = 0;
        using var delivered = new ManualResetEventSlim(false);
        var hub = new TransportNotifyHub();
        try
        {
            var firstBridge = new TransportNotifyBridge(
                () => Interlocked.Increment(ref first),
                OfflineProtocolTransport.Ble,
                hub);
            var secondBridge = new TransportNotifyBridge(
                () =>
                {
                    Interlocked.Increment(ref second);
                    delivered.Set();
                },
                OfflineProtocolTransport.Ble,
                hub);
            firstBridge.SimulateNativeDropForTests();
            secondBridge.SimulateNativeNotifyForTests();
            Assert.True(delivered.Wait(TimeSpan.FromSeconds(5)));
            Assert.Equal(0, first);
            Assert.Equal(1, second);
            secondBridge.ReleaseLocal();
        }
        finally
        {
            hub.Deactivate();
        }
    }
}
