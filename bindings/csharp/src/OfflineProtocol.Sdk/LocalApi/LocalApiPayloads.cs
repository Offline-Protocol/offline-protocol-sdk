using OfflineProtocol.Sdk.Model;

namespace OfflineProtocol.Sdk.LocalApi;

internal sealed class SendMessagePayload
{
    public required string recipient { get; init; }

    public required string content { get; init; }

    public MessagePriority priority { get; init; } = MessagePriority.Medium;
}

internal sealed class FileStoresPayload
{
    public required string mls_root { get; init; }

    public required string state_root { get; init; }

    public required string store_key { get; init; }
}
