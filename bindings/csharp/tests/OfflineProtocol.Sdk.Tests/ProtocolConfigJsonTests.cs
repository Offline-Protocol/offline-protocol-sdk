using System.Text.Json;

namespace OfflineProtocol.Sdk.Tests;

public sealed class ProtocolConfigJsonTests
{
    [Fact]
    public void FileStores_serializes_paths_with_quotes_and_backslashes()
    {
        var mlsRoot = @"C:\data""mls";
        var stateRoot = @"C:\state\root";
        var json = ProtocolConfigJson.FileStores(mlsRoot, stateRoot, "AAECAwQ=");

        using var doc = JsonDocument.Parse(json);
        Assert.Equal(mlsRoot, doc.RootElement.GetProperty("mls_root").GetString());
        Assert.Equal(stateRoot, doc.RootElement.GetProperty("state_root").GetString());
        Assert.Equal("AAECAwQ=", doc.RootElement.GetProperty("store_key").GetString());
    }
}
