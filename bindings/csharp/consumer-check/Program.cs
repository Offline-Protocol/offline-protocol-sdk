using OfflineProtocol.Sdk;
using OfflineProtocol.Sdk.Config;
using OfflineProtocol.Sdk.LocalApi;
using OfflineProtocol.Sdk.Model;
using OfflineProtocol.Sdk.Storage;
using OfflineProtocol.Sdk.Telemetry;

// Every ProtocolConfig field without a default in the interface definition; the rest take the
// definition's defaults. An example program's values, as in bindings/c/README.md, never the SDK's.
var config = new ProtocolConfig
{
    AppId = "com.offlineprotocol.csharp.consumer",
    Profile = "csharp-consumer",
    BleEnabled = false,
    WifiDirectEnabled = false,
    InternetEnabled = true,
    ReticulumEnabled = false,
    NostrEnabled = false,
    PreferOnline = false,
    InitialTtl = 8,
    EncryptionEnabled = true,
    AutoKeyExchange = true,
    StorePending = true,
    MaxPendingPerPeer = 64,
    MaxPendingGlobal = 4096,
    PendingTtlMs = 60_000,
    OverflowPolicy = OverflowPolicy.DropOldest,
};

// A fixed placeholder key for a throwaway store; a real host keeps its key out of the source.
const string StoreKeyBase64 = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

Console.WriteLine($"library version: {ProtocolEngine.LibraryVersion}");

var derive = await ProtocolEngine.DeriveAddressAsync(
    """{"public_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}""");
if (!derive.StartsWith("\"off1", StringComparison.Ordinal))
{
    Console.Error.WriteLine($"FAIL: derive_address expected off1… prefix, got {derive}");
    return 1;
}

var conformance = await ProtocolEngine.RunStorageConformanceAsync(
    new NativeStorageProvider(new MemoryProtocolStorage()));
var stateStorage = new MemoryProtocolStorage();
var stateProvider = new NativeStorageProvider(stateStorage);
if (!conformance.Contains("\"failures\":[]", StringComparison.Ordinal))
{
    Console.Error.WriteLine($"FAIL: storage conformance {conformance}");
    return 1;
}

var mlsRoot = Path.Combine(Path.GetTempPath(), "op-csharp-mls-" + Guid.NewGuid().ToString("N"));
var stateRoot = Path.Combine(Path.GetTempPath(), "op-csharp-state-" + Guid.NewGuid().ToString("N"));
Directory.CreateDirectory(mlsRoot);
Directory.CreateDirectory(stateRoot);

try
{
    await using var engine = await ProtocolEngine.CreateAsync(config);
    var state = await LocalApiTyped.GetStateAsync(engine);
    if (state != ProtocolState.Stopped)
    {
        Console.Error.WriteLine($"FAIL: expected Stopped before start, got {state}");
        return 1;
    }

    var secureStorage = new MemoryProtocolStorage();
    var secureProvider = new NativeStorageProvider(secureStorage);
    await engine.InitializeMlsAsync(secureProvider, stateProvider);
    var addressJson = await engine.LocalAddressAsync();
    if (!addressJson.StartsWith("\"off1", StringComparison.Ordinal))
    {
        Console.Error.WriteLine($"FAIL: local_address expected off1… prefix, got {addressJson}");
        return 1;
    }

    var seenEvents = 0;
    string? lastEvent = null;
    using var eventGate = new ManualResetEventSlim(false);
    await engine.SetEventCallbackAsync(json =>
    {
        Interlocked.Increment(ref seenEvents);
        lastEvent = json;
        eventGate.Set();
    });
    await engine.EmitTestEventAsync();
    if (!eventGate.Wait(TimeSpan.FromSeconds(5)))
    {
        Console.Error.WriteLine("FAIL: event callback timeout");
        return 1;
    }

    // Every event is also queued for polling, and the queue already holds what came before the
    // callback was installed (initialize_mls emits identity_ready). Drain it in order until the
    // event the callback saw turns up.
    var polledEvents = new List<string>();
    var matched = false;
    while (await engine.PollEventAsync() is { } polled)
    {
        polledEvents.Add(polled);
        if (polled == lastEvent)
        {
            matched = true;
            break;
        }
    }

    if (!matched)
    {
        Console.Error.WriteLine(
            $"FAIL: poll_event never returned the callback's event {lastEvent}; polled [{string.Join(", ", polledEvents)}]");
        return 1;
    }

    var transportSignals = 0;
    await engine.SetBleTransportCallbackAsync(() => Interlocked.Increment(ref transportSignals));
    await engine.SetWifiDirectTransportCallbackAsync(() => Interlocked.Increment(ref transportSignals));
    await engine.SetReticulumTransportCallbackAsync(() => Interlocked.Increment(ref transportSignals));
    await engine.SetNostrTransportCallbackAsync(() => Interlocked.Increment(ref transportSignals));
    var bleSwapCount = 0;
    await engine.SetBleTransportCallbackAsync(() => Interlocked.Increment(ref bleSwapCount));
    await engine.SetBleTransportCallbackAsync(() => Interlocked.Increment(ref bleSwapCount));
    await engine.SetBleTransportCallbackAsync(null);
    if (bleSwapCount != 0)
    {
        Console.Error.WriteLine($"FAIL: BLE callback swap smoke expected 0 notifies, got {bleSwapCount}");
        return 1;
    }

    if (transportSignals != 0)
    {
        Console.Error.WriteLine($"FAIL: transport notify counter expected 0 after install-only smoke, got {transportSignals}");
        return 1;
    }

    // An empty key is refused by the engine's own validation, so nothing is enabled and nothing
    // leaves the machine. Reaching that refusal, and no other, proves the C API filled `os` and
    // the JSON this binding built decoded.
    try
    {
        await engine.EnableTelemetryAsync(
            new TelemetryConfig
            {
                ApiKey = "",
                AppId = "com.offlineprotocol.csharp.consumer",
                OsMajor = TelemetryPlatform.OsMajor,
            });
        Console.Error.WriteLine("FAIL: enable_telemetry accepted an empty api_key");
        return 1;
    }
    catch (ProtocolEngineException ex) when (ex.Variant == "TelemetryConfigInvalid" && ex.Message.Contains("api_key", StringComparison.Ordinal))
    {
    }

    await engine.DisableTelemetryAsync();
    if (await engine.FlushTelemetryBlockingAsync(50))
    {
        Console.Error.WriteLine("FAIL: flush_telemetry_blocking reported a finished upload with telemetry disabled");
        return 1;
    }

    await engine.NotifyAppStateAsync(AppState.Background);
    await engine.NotifyAppStateAsync(AppState.Active);

    await engine.StartAsync();
    await engine.StopAsync();

    await using var fileEngine = await ProtocolEngine.CreateAsync(config);
    await fileEngine.InitializeMlsWithFileStoresAsync(mlsRoot, stateRoot, StoreKeyBase64);
    var fileAddress = await fileEngine.LocalAddressAsync();
    if (!fileAddress.StartsWith("\"off1", StringComparison.Ordinal))
    {
        Console.Error.WriteLine($"FAIL: file-store local_address {fileAddress}");
        return 1;
    }

    // The generated data.* client, through the prefix the C API dispatches on.
    await fileEngine.Data.CreateDocAsync("""{"space_id":"consumer","doc_id":"notes"}""");
    var docs = await fileEngine.Data.ListDocsAsync("""{"space_id":"consumer"}""");
    if (!docs.Contains("\"notes\"", StringComparison.Ordinal))
    {
        Console.Error.WriteLine($"FAIL: data.list_docs {docs}");
        return 1;
    }

    await fileEngine.CloseFileStoresAsync();
    Console.WriteLine("csharp consumer check: ok");
    return 0;
}
catch (Exception ex)
{
    Console.Error.WriteLine($"FAIL: {ex}");
    return 1;
}
finally
{
    try
    {
        Directory.Delete(mlsRoot, recursive: true);
        Directory.Delete(stateRoot, recursive: true);
    }
    catch
    {
        // Best-effort cleanup of temp stores.
    }
}
