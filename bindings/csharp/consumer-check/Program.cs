using OfflineProtocol.Sdk;
using OfflineProtocol.Sdk.Native;

const string StoreKeyBase64 = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

NativeLibraryLoader.EnsureRegistered();
var version = OfflineProtocolNative.Version();
Console.WriteLine($"library version: {version}");

var mlsRoot = Path.Combine(Path.GetTempPath(), "op-csharp-mls-" + Guid.NewGuid().ToString("N"));
var stateRoot = Path.Combine(Path.GetTempPath(), "op-csharp-state-" + Guid.NewGuid().ToString("N"));
Directory.CreateDirectory(mlsRoot);
Directory.CreateDirectory(stateRoot);

try
{
    await using var engine = await ProtocolEngine.CreateAsync(ProtocolConfigJson.MinimalConsumer);
    await engine.InitializeMlsWithFileStoresAsync(mlsRoot, stateRoot, StoreKeyBase64);
    var addressJson = await engine.LocalAddressAsync();
    if (!addressJson.StartsWith("\"off1", StringComparison.Ordinal))
    {
        Console.Error.WriteLine($"FAIL: local_address expected off1… prefix, got {addressJson}");
        return 1;
    }

    await engine.StartAsync();
    await engine.StopAsync();
    await engine.CallAsync("close_file_stores");
    Console.WriteLine("csharp consumer check: ok");
    return 0;
}
catch (Exception ex)
{
    Console.Error.WriteLine($"FAIL: {ex.Message}");
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
