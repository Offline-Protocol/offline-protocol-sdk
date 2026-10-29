using System.Text.Json;
using OfflineProtocol.Sdk.Json;

namespace OfflineProtocol.Sdk.Config;

public static class ProtocolConfigJson
{
    public static string Serialize(ProtocolConfig config)
    {
        var body = new Dictionary<string, object?>
        {
            ["app_id"] = config.AppId,
            ["profile"] = config.Profile,
            ["ble_enabled"] = config.BleEnabled,
            ["wifi_direct_enabled"] = config.WifiDirectEnabled,
            ["internet_enabled"] = config.InternetEnabled,
            ["reticulum_enabled"] = config.ReticulumEnabled,
            ["nostr_enabled"] = config.NostrEnabled,
            ["prefer_online"] = config.PreferOnline,
            ["initial_ttl"] = config.InitialTtl,
            ["encryption_enabled"] = config.EncryptionEnabled,
            ["auto_key_exchange"] = config.AutoKeyExchange,
            ["store_pending"] = config.StorePending,
            ["max_pending_per_peer"] = config.MaxPendingPerPeer,
            ["max_pending_global"] = config.MaxPendingGlobal,
            ["pending_ttl_ms"] = config.PendingTtlMs,
            ["overflow_policy"] = config.OverflowPolicy.ToString(),
        };
        AddOptional(body, "require_encryption", config.RequireEncryption);
        AddOptional(body, "edge_driven_unreachable_dm", config.EdgeDrivenUnreachableDm);
        AddOptional(body, "max_group_members", config.MaxGroupMembers);
        AddOptional(body, "group_relay_enabled", config.GroupRelayEnabled);
        AddOptional(body, "group_relay_broadcast_enabled", config.GroupRelayBroadcastEnabled);
        AddOptional(body, "group_enforce_admin_commits", config.GroupEnforceAdminCommits);
        AddOptional(body, "require_transport_identity", config.RequireTransportIdentity);
        AddOptional(body, "binary_wire_enabled", config.BinaryWireEnabled);
        AddOptional(body, "nostr_sealing_enabled", config.NostrSealingEnabled);
        AddOptional(body, "nostr_cold_contact_enabled", config.NostrColdContactEnabled);
        AddOptional(body, "nostr_username_discovery_enabled", config.NostrUsernameDiscoveryEnabled);
        AddOptional(body, "compact_envelope_enabled", config.CompactEnvelopeEnabled);
        AddOptional(body, "rich_payload_enabled", config.RichPayloadEnabled);
        AddOptional(body, "crypto_recovery_enabled", config.CryptoRecoveryEnabled);
        AddOptional(body, "data_enabled", config.DataEnabled);
        AddOptional(body, "control_freshness_enforced", config.ControlFreshnessEnforced);
        AddOptional(body, "encrypt_service_messages", config.EncryptServiceMessages);
        AddOptionalJson(body, "mesh_relay", config.MeshRelay);
        AddOptionalJson(body, "custody", config.Custody);
        return JsonSerializer.Serialize(body, SdkJson.SerializeOptions);
    }

    /// <summary>Consumer-check defaults with required UDL fields only (optional fields use engine defaults).</summary>
    public static ProtocolConfig MinimalConsumer =>
        new()
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

    public static string MinimalConsumerJson => Serialize(MinimalConsumer);

    private static void AddOptional<T>(Dictionary<string, object?> target, string key, T? value)
    {
        if (value is not null)
        {
            target[key] = value;
        }
    }

    private static void AddOptionalJson(Dictionary<string, object?> target, string key, JsonElement? value)
    {
        if (value is { ValueKind: not JsonValueKind.Null and not JsonValueKind.Undefined } element)
        {
            target[key] = element;
        }
    }
}
