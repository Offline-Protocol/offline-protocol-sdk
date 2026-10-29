using System.Text.Json;
using OfflineProtocol.Sdk.Native;
using OfflineProtocol.Sdk.Storage;
using OfflineProtocol.Sdk.Telemetry;

namespace OfflineProtocol.Sdk.Tests;

/// <summary>
/// Events, storage providers and telemetry against the real native library. Each test owns its
/// engine; the library is loaded as for <see cref="ProtocolEngineTests"/>.
/// </summary>
public sealed class EngineIntegrationTests
{
    private static readonly TimeSpan DeliveryTimeout = TimeSpan.FromSeconds(10);

    [Fact]
    public async Task An_event_reaches_the_handler_on_the_delivery_thread_and_the_poll_queue()
    {
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);
        var delivered = new TaskCompletionSource<(string Json, string? Thread)>(
            TaskCreationOptions.RunContinuationsAsynchronously);
        await engine.SetEventCallbackAsync(eventJson => delivered.TrySetResult((eventJson, Thread.CurrentThread.Name)));

        await engine.CallAsync("emit_test_event");
        var (json, thread) = await delivered.Task.WaitAsync(DeliveryTimeout);

        // Never the engine thread: a handler there could re-enter the engine (CS1).
        Assert.Equal("offline-protocol-events", thread);
        Assert.Equal(json, await engine.PollEventAsync());
    }

    [Fact]
    public async Task A_throwing_handler_is_contained_and_delivery_continues()
    {
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);
        var calls = 0;
        var second = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        await engine.SetEventCallbackAsync(_ =>
        {
            if (Interlocked.Increment(ref calls) == 1)
            {
                throw new InvalidOperationException("a bug in the app's handler");
            }

            second.TrySetResult();
        });

        await engine.CallAsync("emit_test_event");
        await engine.CallAsync("emit_test_event");
        await second.Task.WaitAsync(DeliveryTimeout);

        Assert.Equal(1L, engine.EventHandlerFailureCount);
    }

    [Fact]
    public async Task A_cleared_callback_is_never_called_again_and_events_still_reach_the_poll_queue()
    {
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);
        var calls = 0;
        var first = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        await engine.SetEventCallbackAsync(_ =>
        {
            Interlocked.Increment(ref calls);
            first.TrySetResult();
        });
        await engine.CallAsync("emit_test_event");
        await first.Task.WaitAsync(DeliveryTimeout);

        // Clearing joins the delivery thread before it returns, so no call can still be in flight.
        await engine.SetEventCallbackAsync(null);
        var before = Volatile.Read(ref calls);
        await engine.CallAsync("emit_test_event");

        Assert.Equal(before, Volatile.Read(ref calls));
        Assert.NotNull(await engine.PollEventAsync());
    }

    [Fact]
    public async Task The_memory_store_passes_conformance_and_the_run_consumes_its_provider()
    {
        var provider = new NativeStorageProvider(new MemoryProtocolStorage());
        var report = await ProtocolEngine.RunStorageConformanceAsync(provider);

        using var doc = JsonDocument.Parse(report);
        Assert.Empty(doc.RootElement.GetProperty("failures").EnumerateArray());
        Assert.NotEmpty(doc.RootElement.GetProperty("passed").EnumerateArray());

        // The library dropped the provider when the suite finished; a second use is refused in
        // managed code instead of handing native a freed context handle.
        Assert.True(provider.IsDropped);
        var reuse = ProtocolEngine.RunStorageConformanceAsync(provider);
        await Assert.ThrowsAsync<ObjectDisposedException>(() => reuse);
    }

    [Fact]
    public async Task Mls_over_two_managed_providers_gives_a_local_address()
    {
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);
        var secure = new MemoryProtocolStorage();
        await engine.InitializeMlsAsync(
            new NativeStorageProvider(secure),
            new NativeStorageProvider(new MemoryProtocolStorage()));

        Assert.StartsWith("\"off1", await engine.LocalAddressAsync(), StringComparison.Ordinal);
    }

    [Fact]
    public async Task A_used_or_shared_provider_is_refused_before_either_is_handed_out()
    {
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);
        var used = new NativeStorageProvider(new MemoryProtocolStorage());
        await ProtocolEngine.RunStorageConformanceAsync(used);

        var fresh = new NativeStorageProvider(new MemoryProtocolStorage());
        await Assert.ThrowsAsync<ObjectDisposedException>(() => engine.InitializeMlsAsync(fresh, used));
        await Assert.ThrowsAsync<ArgumentException>(() => engine.InitializeMlsAsync(fresh, fresh));

        // Neither refusal gave `fresh` away, so it still attaches.
        await engine.InitializeMlsAsync(fresh, new NativeStorageProvider(new MemoryProtocolStorage()));
        Assert.StartsWith("\"off1", await engine.LocalAddressAsync(), StringComparison.Ordinal);
    }

    [Fact]
    public async Task Enable_telemetry_reaches_the_engine_with_os_filled_by_the_library()
    {
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);

        // An empty key is refused by the engine's own validation. Reaching it proves the C API
        // filled `os` and the JSON decoded; nothing is enabled, so nothing leaves the machine.
        var ex = await Assert.ThrowsAsync<ProtocolEngineException>(() => engine.EnableTelemetryAsync(
            new TelemetryConfig { ApiKey = "", AppId = "com.offlineprotocol.csharp.tests" }));
        Assert.Equal(OfflineProtocolStatus.EngineError, ex.Status);
        Assert.Equal("TelemetryConfigInvalid", ex.Variant);
        Assert.Contains("api_key", ex.Message, StringComparison.Ordinal);
    }

    [Fact]
    public async Task Telemetry_lifecycle_calls_succeed_while_telemetry_is_disabled()
    {
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);

        await engine.NotifyAppStateAsync(AppState.Background);
        await engine.NotifyAppStateAsync(AppState.Active);
        await engine.EndTelemetrySessionAsync();
        await engine.DisableTelemetryAsync();

        // No uploader, so nothing can finish: false, and promptly, never a refusal.
        Assert.False(await engine.FlushTelemetryBlockingAsync(50));
    }

    [Fact]
    public async Task Static_calls_need_no_engine_and_refusals_fault_the_task()
    {
        var address = await ProtocolEngine.CallStaticAsync(
            "derive_address",
            """{"public_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}""");
        Assert.StartsWith("\"off1", address, StringComparison.Ordinal);

        // A refusal comes back as a faulted task, never as a throw at the call site.
        var refused = ProtocolEngine.CallStaticAsync("no_such_function");
        var ex = await Assert.ThrowsAsync<ProtocolEngineException>(() => refused);
        Assert.Equal(OfflineProtocolStatus.MethodNotFound, ex.Status);
    }
}
