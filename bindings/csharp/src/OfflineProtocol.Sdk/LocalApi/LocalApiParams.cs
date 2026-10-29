using System.Text.Json;
using OfflineProtocol.Sdk.Json;
using OfflineProtocol.Sdk.Model;

namespace OfflineProtocol.Sdk.LocalApi;

/// <summary>JSON parameter builders for common local API calls.</summary>
public static class LocalApiParams
{
    public static string FileStores(string mlsRoot, string stateRoot, string storeKeyBase64) =>
        JsonSerializer.Serialize(
            new FileStoresPayload { MlsRoot = mlsRoot, StateRoot = stateRoot, StoreKey = storeKeyBase64 },
            LocalApiJsonContext.Default.FileStoresPayload);

    /// <summary>Parameters for <c>send_message</c>; <c>reply_to_msg</c> is left out, so it is null.</summary>
    public static string SendMessage(
        string recipient,
        string content,
        MessagePriority priority = MessagePriority.Medium) =>
        JsonSerializer.Serialize(
            new SendMessagePayload { Recipient = recipient, Content = content, Priority = priority },
            LocalApiJsonContext.Default.SendMessagePayload);
}
