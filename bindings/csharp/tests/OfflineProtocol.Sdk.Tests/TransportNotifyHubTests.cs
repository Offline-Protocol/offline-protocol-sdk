using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.Tests;

public sealed class TransportNotifyHubTests
{
    private static readonly TimeSpan DeliveryTimeout = TimeSpan.FromSeconds(5);

    [Fact]
    public void There_is_one_carrier_per_transport()
    {
        Assert.Equal(Enum.GetValues<OfflineProtocolTransport>().Length, TransportNotifyHub.CarrierCount);
    }

    [Fact]
    public void A_throwing_handler_is_counted_and_delivery_continues()
    {
        var calls = 0;
        using var second = new ManualResetEventSlim(false);
        var hub = new TransportNotifyHub();
        try
        {
            hub.ActivateCarrier(0, () =>
            {
                if (Interlocked.Increment(ref calls) == 1)
                {
                    throw new InvalidOperationException("a bug in the app's handler");
                }

                second.Set();
            });

            hub.PostNotify(0);
            var deadline = Environment.TickCount64 + 5_000;
            while (hub.HandlerFailureCount(0) == 0 && Environment.TickCount64 < deadline)
            {
                Thread.Sleep(10);
            }

            hub.PostNotify(0);
            Assert.True(second.Wait(DeliveryTimeout));
            Assert.Equal(1L, hub.HandlerFailureCount(0));
            Assert.Equal(0L, hub.HandlerFailureCount(3));
        }
        finally
        {
            hub.Deactivate();
        }
    }

    // A stamp queued under a replaced registration must not wake its replacement. The delivery
    // thread is held inside another carrier's handler so the stale stamp is still queued when the
    // carrier is replaced; Nostr is drained after BLE in a pass, so its second run marks the pass
    // that read the stale stamp as finished.
    [Fact]
    public void A_stamp_queued_before_a_replacement_does_not_wake_the_new_handler()
    {
        var replaced = 0;
        var nostrRuns = 0;
        using var holding = new ManualResetEventSlim(false);
        using var release = new ManualResetEventSlim(false);
        using var passDone = new ManualResetEventSlim(false);
        var hub = new TransportNotifyHub();
        try
        {
            hub.ActivateCarrier(3, () =>
            {
                if (Interlocked.Increment(ref nostrRuns) == 1)
                {
                    holding.Set();
                    release.Wait();
                }
                else
                {
                    passDone.Set();
                }
            });
            hub.ActivateCarrier(0, () => { });

            hub.PostNotify(3);
            Assert.True(holding.Wait(DeliveryTimeout));
            hub.PostNotify(0);
            hub.DeactivateCarrier(0);
            hub.ActivateCarrier(0, () => Interlocked.Increment(ref replaced));
            release.Set();
            hub.PostNotify(3);

            Assert.True(passDone.Wait(DeliveryTimeout));
            Assert.Equal(0, replaced);
        }
        finally
        {
            release.Set();
            hub.Deactivate();
        }
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
