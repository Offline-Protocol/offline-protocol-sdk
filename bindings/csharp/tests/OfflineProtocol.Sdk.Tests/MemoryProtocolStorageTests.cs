namespace OfflineProtocol.Sdk.Tests;

using OfflineProtocol.Sdk.Storage;

public sealed class MemoryProtocolStorageTests
{
    [Fact]
    public void Store_load_remove_round_trip()
    {
        var storage = new MemoryProtocolStorage();
        Assert.Equal(OfflineProtocolStorageStatus.Ok, storage.Store("t", "id1", [1, 2, 3]));
        var loaded = storage.Load("t", "id1");
        Assert.NotNull(loaded);
        Assert.Equal(new byte[] { 1, 2, 3 }, loaded);
        Assert.Null(storage.Load("t", "missing"));
        Assert.Equal(OfflineProtocolStorageStatus.Ok, storage.Remove("t", "id1"));
        Assert.Null(storage.Load("t", "id1"));
    }

    [Fact]
    public void ListKeyIds_returns_json_safe_ids()
    {
        var storage = new MemoryProtocolStorage();
        storage.Store("kt", "a", [0]);
        storage.Store("kt", "b\"c", [0]);
        var json = storage.ListKeysJson("kt");
        Assert.Contains("\"a\"", json, StringComparison.Ordinal);
        Assert.Contains("\"b\\\"c\"", json, StringComparison.Ordinal);
    }
}
