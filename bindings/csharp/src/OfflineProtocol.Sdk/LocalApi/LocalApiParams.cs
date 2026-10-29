using System.Buffers;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization.Metadata;
using OfflineProtocol.Sdk.Json;
using OfflineProtocol.Sdk.Model;

namespace OfflineProtocol.Sdk.LocalApi;

/// <summary>JSON parameter builders for common local API calls.</summary>
public static class LocalApiParams
{
    private const int TypicalFileStoresJsonBytes = 192;
    private const int TypicalSendMessageJsonBytes = 256;

    public static string FileStores(string mlsRoot, string stateRoot, string storeKeyBase64) =>
        Serialize(
            new FileStoresPayload
            {
                mls_root = mlsRoot,
                state_root = stateRoot,
                store_key = storeKeyBase64,
            },
            LocalApiJsonContext.Default.FileStoresPayload,
            TypicalFileStoresJsonBytes);

    /// <summary>
    /// Builds JSON for <c>send_message</c>. One UTF-16 string is allocated for the
    /// engine queue. For high throughput, serialize once and call
    /// <see cref="LocalApiTyped.SendMessageAsync(ProtocolEngine, string, CancellationToken)"/>.
    /// </summary>
    public static string SendMessage(
        string recipient,
        string content,
        MessagePriority priority = MessagePriority.Medium) =>
        Serialize(
            new SendMessagePayload
            {
                recipient = recipient,
                content = content,
                priority = priority,
            },
            LocalApiJsonContext.Default.SendMessagePayload,
            TypicalSendMessageJsonBytes);

    private static string Serialize<T>(T payload, JsonTypeInfo<T> typeInfo, int initialCapacity)
    {
        var buffer = new ArrayBufferWriter<byte>(initialCapacity);
        JsonSerializer.Serialize(buffer, payload, typeInfo);
        return Encoding.UTF8.GetString(buffer.WrittenSpan);
    }
}
