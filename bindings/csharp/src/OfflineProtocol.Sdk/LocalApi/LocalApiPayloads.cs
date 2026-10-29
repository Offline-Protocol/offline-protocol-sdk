using System.Text.Json.Serialization;
using OfflineProtocol.Sdk.Model;

namespace OfflineProtocol.Sdk.LocalApi;

internal sealed class SendMessagePayload
{
    [JsonPropertyName("recipient")]
    public required string Recipient { get; init; }

    [JsonPropertyName("content")]
    public required string Content { get; init; }

    [JsonPropertyName("priority")]
    public required MessagePriority Priority { get; init; }
}

internal sealed class FileStoresPayload
{
    [JsonPropertyName("mls_root")]
    public required string MlsRoot { get; init; }

    [JsonPropertyName("state_root")]
    public required string StateRoot { get; init; }

    [JsonPropertyName("store_key")]
    public required string StoreKey { get; init; }
}
