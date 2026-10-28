using System.Text.Json;
using OfflineProtocol.Sdk.Storage;

namespace OfflineProtocol.Sdk.Tests;

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
    public void Remove_drops_empty_key_type_bucket()
    {
        var storage = new MemoryProtocolStorage();
        storage.Store("only", "k", [0]);
        storage.Remove("only", "k");
        Assert.Empty(storage.ListKeyIds("only"));
    }

    [Fact]
    public void ListKeyIds_is_scoped_to_key_type()
    {
        var storage = new MemoryProtocolStorage();
        storage.Store("a", "1", [0]);
        storage.Store("b", "2", [0]);
        var aKeys = storage.ListKeyIds("a");
        Assert.Single(aKeys);
        Assert.Equal("1", aKeys[0]);
    }

    [Fact]
    public void ListKeyIds_returns_json_safe_ids()
    {
        var storage = new MemoryProtocolStorage();
        storage.Store("kt", "a", [0]);
        storage.Store("kt", "b\"c", [0]);
        var json = JsonSerializer.Serialize(storage.ListKeyIds("kt"));
        Assert.Contains("\"a\"", json, StringComparison.Ordinal);
        Assert.Contains("\"b\\\"c\"", json, StringComparison.Ordinal);
    }
}
