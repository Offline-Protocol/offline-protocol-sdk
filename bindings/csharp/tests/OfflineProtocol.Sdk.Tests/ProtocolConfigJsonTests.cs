using System.Text.Json;
using OfflineProtocol.Sdk.Config;

namespace OfflineProtocol.Sdk.Tests;

public sealed class ProtocolConfigJsonTests
{
    private static readonly string[] RequiredKeys =
    [
        "app_id", "profile", "ble_enabled", "wifi_direct_enabled", "internet_enabled",
        "reticulum_enabled", "nostr_enabled", "prefer_online", "initial_ttl", "encryption_enabled",
        "auto_key_exchange", "store_pending", "max_pending_per_peer", "max_pending_global",
        "pending_ttl_ms", "overflow_policy",
    ];

    internal static ProtocolConfig Required(OverflowPolicy policy = OverflowPolicy.DropOldest) =>
        new()
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
            OverflowPolicy = policy,
        };

    private static string[] Keys(string json)
    {
        using var doc = JsonDocument.Parse(json);
        return doc.RootElement.EnumerateObject().Select(p => p.Name).Order(StringComparer.Ordinal).ToArray();
    }

    [Fact]
    public void Unset_optional_fields_are_left_out_so_the_engine_applies_its_own_defaults()
    {
        // C6: the SDK writes no default of its own, so an unset field never reaches the engine.
        Assert.Equal(RequiredKeys.Order(StringComparer.Ordinal), Keys(ProtocolConfigJson.Serialize(Required())));
    }

    [Fact]
    public void A_set_optional_field_is_written_even_when_it_equals_the_default()
    {
        var json = ProtocolConfigJson.Serialize(
            Required(OverflowPolicy.DropNewest) with { DataEnabled = true, RequireEncryption = false, MaxGroupMembers = 7 });

        using var doc = JsonDocument.Parse(json);
        Assert.Equal("DropNewest", doc.RootElement.GetProperty("overflow_policy").GetString());
        Assert.True(doc.RootElement.GetProperty("data_enabled").GetBoolean());
        Assert.False(doc.RootElement.GetProperty("require_encryption").GetBoolean());
        Assert.Equal(7u, doc.RootElement.GetProperty("max_group_members").GetUInt32());
    }

    [Fact]
    public void Nested_sections_are_embedded_and_a_json_null_is_left_out()
    {
        using var meshRelay = JsonDocument.Parse("""{"fanout":2}""");
        using var custody = JsonDocument.Parse("null");
        var json = ProtocolConfigJson.Serialize(
            Required() with { MeshRelay = meshRelay.RootElement.Clone(), Custody = custody.RootElement.Clone() });

        using var doc = JsonDocument.Parse(json);
        Assert.Equal(2, doc.RootElement.GetProperty("mesh_relay").GetProperty("fanout").GetInt32());
        Assert.False(doc.RootElement.TryGetProperty("custody", out _));
    }

    [Fact]
    public void An_undefined_enum_value_is_refused_rather_than_written_as_a_number()
    {
        Assert.Throws<ArgumentOutOfRangeException>(
            () => ProtocolConfigJson.Serialize(Required((OverflowPolicy)42)));
    }
}
