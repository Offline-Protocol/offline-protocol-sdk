using System.Text.Json;
using OfflineProtocol.Sdk.LocalApi;
using OfflineProtocol.Sdk.Model;

namespace OfflineProtocol.Sdk.Tests;

public sealed class LocalApiParamsTests
{
    [Fact]
    public void SendMessage_writes_the_wire_names_and_the_priority_as_its_udl_string()
    {
        var json = LocalApiParams.SendMessage("off1abc", "hi", MessagePriority.High);

        using var doc = JsonDocument.Parse(json);
        Assert.Equal("off1abc", doc.RootElement.GetProperty("recipient").GetString());
        Assert.Equal("hi", doc.RootElement.GetProperty("content").GetString());
        Assert.Equal("High", doc.RootElement.GetProperty("priority").GetString());
    }

    [Fact]
    public void FileStores_serializes_paths_with_quotes_and_backslashes()
    {
        var mlsRoot = @"C:\data""mls";
        var stateRoot = @"C:\state\root";
        var json = LocalApiParams.FileStores(mlsRoot, stateRoot, "AAECAwQ=");

        using var doc = JsonDocument.Parse(json);
        Assert.Equal(mlsRoot, doc.RootElement.GetProperty("mls_root").GetString());
        Assert.Equal(stateRoot, doc.RootElement.GetProperty("state_root").GetString());
        Assert.Equal("AAECAwQ=", doc.RootElement.GetProperty("store_key").GetString());
    }
}
