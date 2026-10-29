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
    public void Removing_the_last_key_leaves_no_ids_and_the_type_still_takes_writes()
    {
        var storage = new MemoryProtocolStorage();
        storage.Store("only", "k", [0]);
        storage.Remove("only", "k");
        Assert.Empty(storage.ListKeyIds("only"));

        // The emptied bucket is kept, so a write after the remove is never discarded.
        storage.Store("only", "k2", [1]);
        Assert.Equal("k2", Assert.Single(storage.ListKeyIds("only")));
    }

    [Fact]
    public void Removing_an_absent_key_is_success()
    {
        var storage = new MemoryProtocolStorage();
        Assert.Equal(OfflineProtocolStorageStatus.Ok, storage.Remove("none", "k"));
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

        // Parsed back rather than matched as text: the encoder may write a quote as \" or
        // \u0022, and both are the same JSON string to the native parser.
        Assert.Equal(new[] { "a", "b\"c" }, JsonSerializer.Deserialize<string[]>(json));
    }
}
