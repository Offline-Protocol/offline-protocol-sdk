using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.Tests;

/// <summary>
/// The engine facade against the real native library: statuses, the queue, and shutdown.
/// Needs <c>OFFLINE_PROTOCOL_LIBRARY_DIR</c> or the library beside the test assembly.
/// </summary>
public sealed class ProtocolEngineTests
{
    // Every ProtocolConfig field without a default in the interface definition.
    internal const string Config =
        """
        {
          "app_id": "com.offlineprotocol.csharp.tests",
          "profile": "csharp-tests",
          "ble_enabled": false,
          "wifi_direct_enabled": false,
          "internet_enabled": true,
          "reticulum_enabled": false,
          "nostr_enabled": false,
          "prefer_online": false,
          "initial_ttl": 8,
          "encryption_enabled": true,
          "auto_key_exchange": true,
          "store_pending": true,
          "max_pending_per_peer": 64,
          "max_pending_global": 4096,
          "pending_ttl_ms": 60000,
          "overflow_policy": "DropOldest"
        }
        """;

    [Fact]
    public void The_library_reports_its_version()
    {
        Assert.Matches(@"^\d+\.\d+\.\d+", ProtocolEngine.LibraryVersion);
    }

    [Fact]
    public async Task A_refused_create_surfaces_the_status_and_message()
    {
        var ex = await Assert.ThrowsAsync<ProtocolEngineException>(() => ProtocolEngine.CreateAsync("{}"));
        Assert.Equal(OfflineProtocolStatus.InvalidParams, ex.Status);
        Assert.Equal(-1, ex.Code);
        Assert.Null(ex.Variant);
        Assert.NotEmpty(ex.Message);
    }

    [Fact]
    public async Task Each_refusal_carries_its_own_status()
    {
        await using var engine = await ProtocolEngine.CreateAsync(Config);

        var unknown = await Assert.ThrowsAsync<ProtocolEngineException>(() => engine.CallAsync("no_such_method"));
        Assert.Equal(OfflineProtocolStatus.MethodNotFound, unknown.Status);

        var badJson = await Assert.ThrowsAsync<ProtocolEngineException>(() => engine.CallAsync("get_state", "{"));
        Assert.Equal(OfflineProtocolStatus.InvalidArgument, badJson.Status);

        var extraKey = await Assert.ThrowsAsync<ProtocolEngineException>(() => engine.CallAsync("get_state", """{"x":1}"""));
        Assert.Equal(OfflineProtocolStatus.InvalidParams, extraKey.Status);

        // The engine is still usable after refusals: the error was cleared, not left half-read.
        Assert.NotEmpty(await engine.CallWithResultAsync("get_state"));
    }

    [Fact]
    public async Task Dispose_lets_every_accepted_call_finish_before_the_free()
    {
        var engine = await ProtocolEngine.CreateAsync(Config, queueCapacity: 4);

        // More calls than the queue holds, none awaited: some wait to enter the channel.
        var calls = Enumerable.Range(0, 64).Select(_ => engine.CallWithResultAsync("get_state")).ToList();
        await engine.DisposeAsync();

        foreach (var call in calls)
        {
            Assert.True(call.IsCompleted);
            // Accepted before dispose, so each ran against a live handle rather than a freed one.
            Assert.NotEmpty(await call);
        }
    }

    [Fact]
    public async Task Calls_after_dispose_are_refused_and_dispose_is_idempotent()
    {
        var engine = await ProtocolEngine.CreateAsync(Config);
        var first = engine.DisposeAsync();
        var second = engine.DisposeAsync();
        await first;
        await second;
        await engine.DisposeAsync();

        await Assert.ThrowsAsync<ObjectDisposedException>(() => engine.CallAsync("get_state"));
    }

    [Fact]
    public async Task A_cancelled_wait_does_not_skip_the_queued_call()
    {
        await using var engine = await ProtocolEngine.CreateAsync(Config, queueCapacity: 1);
        using var cts = new CancellationTokenSource();

        var calls = Enumerable.Range(0, 16)
            .Select(_ => engine.CallWithResultAsync("get_state", null, cts.Token))
            .ToList();
        cts.Cancel();

        foreach (var call in calls)
        {
            try
            {
                await call;
            }
            catch (OperationCanceledException)
            {
                // The caller stopped waiting; the engine state is unaffected either way.
            }
        }

        Assert.NotEmpty(await engine.CallWithResultAsync("get_state"));
    }
}
