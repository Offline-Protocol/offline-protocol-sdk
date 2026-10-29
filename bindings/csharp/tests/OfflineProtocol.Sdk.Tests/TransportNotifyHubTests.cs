using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.Tests;

public sealed class TransportNotifyHubTests
{
    [Fact]
    public void Default_queue_capacity_scales_per_carrier()
    {
        Assert.Equal(256, TransportNotifyHub.DefaultPerCarrierQueueCapacity);
        Assert.Equal(256 * TransportNotifyHub.CarrierCount, TransportNotifyHub.DefaultQueueCapacity);
    }

    [Fact]
    public void Per_carrier_queues_do_not_starve_sibling_carriers()
    {
        var ble = 0;
        var nostr = 0;
        var hub = new TransportNotifyHub(perCarrierQueueCapacity: 4);
        try
        {
            hub.ActivateCarrier(0, () => Interlocked.Increment(ref ble));
            hub.ActivateCarrier(3, () => Interlocked.Increment(ref nostr));
            for (var n = 0; n < 8; n++)
            {
                hub.PostNotify(0);
            }

            for (var n = 0; n < 2; n++)
            {
                hub.PostNotify(3);
            }

            var deadline = Environment.TickCount64 + 5_000;
            while ((ble == 0 || nostr == 0) && Environment.TickCount64 < deadline)
            {
                Thread.Sleep(10);
            }

            Assert.True(ble > 0);
            Assert.True(nostr > 0);
        }
        finally
        {
            hub.Deactivate();
        }
    }
}
