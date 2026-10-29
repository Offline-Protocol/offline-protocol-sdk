using System.Text.Json;

namespace OfflineProtocol.Sdk.Config;

/// <summary>
/// Typed <c>ProtocolConfig</c> for engine creation (C6). Optional UDL defaults are omitted from JSON unless set.
/// </summary>
public sealed class ProtocolConfig
{
    public required string AppId { get; init; }

    public required string Profile { get; init; }

    public required bool BleEnabled { get; init; }

    public required bool WifiDirectEnabled { get; init; }

    public required bool InternetEnabled { get; init; }

    public required bool ReticulumEnabled { get; init; }

    public required bool NostrEnabled { get; init; }

    public required bool PreferOnline { get; init; }

    public required byte InitialTtl { get; init; }

    public required bool EncryptionEnabled { get; init; }

    public required bool AutoKeyExchange { get; init; }

    public required bool StorePending { get; init; }

    public required ulong MaxPendingPerPeer { get; init; }

    public required ulong MaxPendingGlobal { get; init; }

    public required ulong PendingTtlMs { get; init; }

    public required OverflowPolicy OverflowPolicy { get; init; }

    public bool? RequireEncryption { get; init; }

    public bool? EdgeDrivenUnreachableDm { get; init; }

    public uint? MaxGroupMembers { get; init; }

    public bool? GroupRelayEnabled { get; init; }

    public bool? GroupRelayBroadcastEnabled { get; init; }

    public bool? GroupEnforceAdminCommits { get; init; }

    public bool? RequireTransportIdentity { get; init; }

    public bool? BinaryWireEnabled { get; init; }

    public bool? NostrSealingEnabled { get; init; }

    public bool? NostrColdContactEnabled { get; init; }

    public bool? NostrUsernameDiscoveryEnabled { get; init; }

    public bool? CompactEnvelopeEnabled { get; init; }

    public bool? RichPayloadEnabled { get; init; }

    public bool? CryptoRecoveryEnabled { get; init; }

    public bool? DataEnabled { get; init; }

    public bool? ControlFreshnessEnforced { get; init; }

    public bool? EncryptServiceMessages { get; init; }

    /// <summary>
    /// Optional <c>MeshRelayConfig</c> object (UDL nested dictionary). Omitted from JSON when null.
    /// </summary>
    public JsonElement? MeshRelay { get; init; }

    /// <summary>
    /// Optional <c>CustodyConfig</c> object (UDL nested dictionary). Omitted from JSON when null.
    /// </summary>
    public JsonElement? Custody { get; init; }
}
