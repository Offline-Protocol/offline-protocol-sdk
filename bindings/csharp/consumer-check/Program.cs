using OfflineProtocol.Sdk;
using OfflineProtocol.Sdk.Storage;

// Every ProtocolConfig field without a default in the interface definition; the rest take the
// definition's defaults. An example program's values, as in bindings/c/README.md, never the SDK's.
const string Config =
    """
    {
      "app_id": "com.offlineprotocol.csharp.consumer",
      "profile": "csharp-consumer",
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

// A fixed placeholder key for a throwaway store; a real host keeps its key out of the source.
const string StoreKeyBase64 = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

Console.WriteLine($"library version: {ProtocolEngine.LibraryVersion}");

var derive = await ProtocolEngine.CallStaticAsync(
    "derive_address",
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
    await using var engine = await ProtocolEngine.CreateAsync(Config);

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
    await engine.CallAsync("emit_test_event");
    if (!eventGate.Wait(TimeSpan.FromSeconds(5)))
    {
        Console.Error.WriteLine("FAIL: event callback timeout");
        return 1;
    }

    var polled = await engine.PollEventAsync();
    if (polled is null || lastEvent is null || polled != lastEvent)
    {
        Console.Error.WriteLine($"FAIL: poll_event mismatch polled={polled} callback={lastEvent}");
        return 1;
    }

    await engine.StartAsync();
    await engine.StopAsync();

    await using var fileEngine = await ProtocolEngine.CreateAsync(Config);
    await fileEngine.InitializeMlsWithFileStoresAsync(mlsRoot, stateRoot, StoreKeyBase64);
    var fileAddress = await fileEngine.LocalAddressAsync();
    if (!fileAddress.StartsWith("\"off1", StringComparison.Ordinal))
    {
        Console.Error.WriteLine($"FAIL: file-store local_address {fileAddress}");
        return 1;
    }

    await fileEngine.CallAsync("close_file_stores");
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
