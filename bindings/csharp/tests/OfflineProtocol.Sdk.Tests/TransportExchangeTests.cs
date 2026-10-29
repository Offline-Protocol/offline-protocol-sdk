using System.Text.Json;
using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.Tests;

/// <summary>
/// Transport callbacks against the real native library: two engines in one process with the test
/// carrying Wi-Fi Direct frames between them, as <c>bindings/c/consumer-check/exchange.c</c> does
/// in C. The hub and bridge tests call the managed entry points directly; only this proves that
/// the function pointers the C API holds reach a C# handler, on the delivery thread, and that the
/// queue they signal drains through the generated local API methods.
/// </summary>
public sealed class TransportExchangeTests
{
    // A session forms in a handful of rounds; the margin is for a loaded runner.
    private const int MaxRounds = 600;
    private const string StoreKeyBase64 = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    private static string Config(string profile) =>
        $$"""
        {
          "app_id": "com.offlineprotocol.csharp.tests",
          "profile": "{{profile}}",
          "ble_enabled": false,
          "wifi_direct_enabled": true,
          "internet_enabled": false,
          "reticulum_enabled": false,
          "nostr_enabled": false,
          "prefer_online": false,
          "initial_ttl": 8,
          "encryption_enabled": true,
          "auto_key_exchange": true,
          "require_encryption": true,
          "store_pending": true,
          "max_pending_per_peer": 64,
          "max_pending_global": 4096,
          "pending_ttl_ms": 60000,
          "overflow_policy": "DropOldest"
        }
        """;

    private sealed class Device : IAsyncDisposable
    {
        private readonly string _mlsRoot = TempDir("mls");
        private readonly string _stateRoot = TempDir("state");

        public required ProtocolEngine Engine { get; init; }
        public string Address { get; private set; } = "";
        public int Notified;
        public string? NotifyThread;
        public int Received;

        public static async Task<Device> BringUpAsync(string profile, bool handlerThrows)
        {
            var device = new Device { Engine = await ProtocolEngine.CreateAsync(Config(profile)) };

            // Installed before the identity exists: the engine keeps it and re-applies it to the
            // transport rebuilt against the derived address.
            await device.Engine.SetWifiDirectTransportCallbackAsync(() =>
            {
                Interlocked.Increment(ref device.Notified);
                device.NotifyThread = Thread.CurrentThread.Name;
                if (handlerThrows)
                {
                    throw new InvalidOperationException("a bug in the app's transport handler");
                }
            });
            await device.Engine.InitializeMlsWithFileStoresAsync(device._mlsRoot, device._stateRoot, StoreKeyBase64);
            device.Address = JsonSerializer.Deserialize<string>(await device.Engine.LocalAddressAsync())!;
            await device.Engine.StartAsync();
            await device.Engine.WifiDirectStatusChangedAsync("""{"is_connected":true}""");
            return device;
        }

        /// <summary>The host's side of a stream that proved <paramref name="peer"/>'s address.</summary>
        public Task AnnounceAsync(Device peer) =>
            Engine.WifiDirectPeerConnectedAsync(JsonSerializer.Serialize(new { peer_id = peer.Address }));

        public async ValueTask DisposeAsync()
        {
            await Engine.StopAsync();
            await Engine.CloseFileStoresAsync();
            await Engine.DisposeAsync();
            TryDelete(_mlsRoot);
            TryDelete(_stateRoot);
        }

        private static string TempDir(string what)
        {
            var path = Path.Combine(Path.GetTempPath(), $"op-csharp-x-{what}-{Guid.NewGuid():N}");
            Directory.CreateDirectory(path);
            return path;
        }

        private static void TryDelete(string path)
        {
            try
            {
                Directory.Delete(path, recursive: true);
            }
            catch
            {
                // Best-effort cleanup of temp stores.
            }
        }
    }

    [Fact]
    public async Task Native_notifies_reach_the_handler_and_the_queue_they_signal_carries_a_session()
    {
        // Alice's handler throws on every run: the exchange must still complete, and the throws
        // must be counted rather than end the test host.
        await using var alice = await Device.BringUpAsync("alice", handlerThrows: true);
        await using var bob = await Device.BringUpAsync("bob", handlerThrows: false);
        Assert.NotEqual(alice.Address, bob.Address);
        await alice.AnnounceAsync(bob);
        await bob.AnnounceAsync(alice);

        await DeliverAsync(alice, bob, "hello from alice");
        await DeliverAsync(bob, alice, "hello from bob");

        // The signal is posted, not run inline, so let the delivery thread catch up.
        var deadline = Environment.TickCount64 + 10_000;
        while ((Volatile.Read(ref alice.Notified) == 0 || Volatile.Read(ref bob.Notified) == 0
                || alice.Engine.TransportHandlerFailureCount(OfflineProtocolTransport.WifiDirect) == 0)
               && Environment.TickCount64 < deadline)
        {
            await Task.Delay(10);
        }

        Assert.True(alice.Notified > 0, "alice's stream transport never notified");
        Assert.True(bob.Notified > 0, "bob's stream transport never notified");
        Assert.Equal(TransportNotifyHub.DeliveryThreadName, bob.NotifyThread);
        // Every run threw; a run still in flight may not be counted yet, so a bound, not equality.
        var aliceFailures = alice.Engine.TransportHandlerFailureCount(OfflineProtocolTransport.WifiDirect);
        Assert.InRange(aliceFailures, 1L, Volatile.Read(ref alice.Notified));
        Assert.Equal(0L, bob.Engine.TransportHandlerFailureCount(OfflineProtocolTransport.WifiDirect));
        Assert.Equal(0L, alice.Engine.TransportHandlerFailureCount(OfflineProtocolTransport.Ble));
    }

    [Fact]
    public async Task Installed_swapped_and_cleared_callbacks_never_fire_and_dispose_with_one_installed()
    {
        var calls = 0;
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);
        await engine.SetBleTransportCallbackAsync(() => Interlocked.Increment(ref calls));
        await engine.SetBleTransportCallbackAsync(() => Interlocked.Increment(ref calls));
        await engine.SetBleTransportCallbackAsync(null);
        await engine.SetNostrTransportCallbackAsync(() => Interlocked.Increment(ref calls));
        Assert.Equal(0, calls);

        // Disposing with a registration still installed runs the dispose-time clear; the free
        // must complete rather than fault.
        await engine.DisposeAsync();
        Assert.Equal(0, calls);
    }

    /// <summary>Moves every queued frame out of <paramref name="from"/> into <paramref name="to"/>.</summary>
    private static async Task<int> CarryAsync(Device from, Device to)
    {
        var moved = 0;
        while (true)
        {
            var next = await from.Engine.WifiDirectGetNextMessageAsync();
            if (next == "null")
            {
                return moved;
            }

            using var frame = JsonDocument.Parse(next);
            var recipient = frame.RootElement.GetProperty("recipient_id").GetString();
            var data = frame.RootElement.GetProperty("data").GetString();
            Assert.Equal(to.Address, recipient);
            await to.Engine.WifiDirectMessageReceivedAsync(
                JsonSerializer.Serialize(new { sender_id = from.Address, data }));
            moved++;
        }
    }

    private static async Task DrainEventsAsync(Device device, string text)
    {
        while (await device.Engine.PollEventAsync() is { } json)
        {
            if (json.Contains("\"type\":\"message_received\"", StringComparison.Ordinal)
                && json.Contains($"\"content\":\"{text}\"", StringComparison.Ordinal))
            {
                device.Received++;
            }
        }
    }

    /// <summary>Pumps both ways, ticking both engines, until <paramref name="to"/> has <paramref name="text"/>.</summary>
    private static async Task DeliverAsync(Device from, Device to, string text)
    {
        await from.Engine.SendMessageAsync(
            JsonSerializer.Serialize(new { recipient = to.Address, content = text, priority = "Medium" }));
        var round = 0;
        to.Received = 0;
        for (; round < MaxRounds && to.Received == 0; round++)
        {
            var moved = await CarryAsync(from, to) + await CarryAsync(to, from);
            await from.Engine.ProcessAsync();
            await to.Engine.ProcessAsync();
            await DrainEventsAsync(to, text);
            await DrainEventsAsync(from, "\u0001");
            if (moved == 0)
            {
                await Task.Delay(10);
            }
        }

        Assert.True(to.Received == 1, $"\"{text}\" delivered {to.Received} times after {round} rounds");
    }
}
