using System.Text.Json;
using OfflineProtocol.Sdk.Config;
using OfflineProtocol.Sdk.LocalApi;

namespace OfflineProtocol.Sdk.Tests;

public sealed class ProtocolConfigJsonTests
{
    [Fact]
    public void Serialize_includes_required_fields_and_optional_when_set()
    {
        var json = ProtocolConfigJson.Serialize(
            new ProtocolConfig
            {
                AppId = "app",
                Profile = "p",
                BleEnabled = true,
                WifiDirectEnabled = false,
                InternetEnabled = true,
                ReticulumEnabled = false,
                NostrEnabled = false,
                PreferOnline = false,
                InitialTtl = 8,
                EncryptionEnabled = true,
                AutoKeyExchange = true,
                StorePending = true,
                MaxPendingPerPeer = 1,
                MaxPendingGlobal = 2,
                PendingTtlMs = 3,
                OverflowPolicy = OverflowPolicy.DropNewest,
                DataEnabled = false,
            });

        using var doc = JsonDocument.Parse(json);
        Assert.Equal("app", doc.RootElement.GetProperty("app_id").GetString());
        Assert.Equal("DropNewest", doc.RootElement.GetProperty("overflow_policy").GetString());
        Assert.False(doc.RootElement.GetProperty("data_enabled").GetBoolean());
    }

    [Fact]
    public void Serialize_embeds_mesh_relay_and_custody_objects_when_set()
    {
        using var meshRelay = JsonDocument.Parse("""{"fanout":2}""");
        using var custody = JsonDocument.Parse("""{"enabled":true}""");
        var json = ProtocolConfigJson.Serialize(
            new ProtocolConfig
            {
                AppId = "app",
                Profile = "p",
                BleEnabled = false,
                WifiDirectEnabled = false,
                InternetEnabled = false,
                ReticulumEnabled = false,
                NostrEnabled = false,
                PreferOnline = false,
                InitialTtl = 8,
                EncryptionEnabled = true,
                AutoKeyExchange = true,
                StorePending = true,
                MaxPendingPerPeer = 1,
                MaxPendingGlobal = 2,
                PendingTtlMs = 3,
                OverflowPolicy = OverflowPolicy.DropOldest,
                MeshRelay = meshRelay.RootElement.Clone(),
                Custody = custody.RootElement.Clone(),
            });

        using var doc = JsonDocument.Parse(json);
        Assert.Equal(2, doc.RootElement.GetProperty("mesh_relay").GetProperty("fanout").GetInt32());
        Assert.True(doc.RootElement.GetProperty("custody").GetProperty("enabled").GetBoolean());
    }

    [Fact]
    public void FileStores_serializes_paths_with_quotes_and_backslashes()
    {
        var mlsRoot = @"C:\data""mls";
        var json = LocalApiParams.FileStores(mlsRoot, @"C:\state", "AAECAwQ=");

        using var doc = JsonDocument.Parse(json);
        Assert.Equal(mlsRoot, doc.RootElement.GetProperty("mls_root").GetString());
    }
}
