using System.Text.Json.Serialization;
using OfflineProtocol.Sdk.LocalApi;
using OfflineProtocol.Sdk.Model;

namespace OfflineProtocol.Sdk.Json;

[JsonSerializable(typeof(ProtocolState))]
[JsonSerializable(typeof(SendMessagePayload))]
[JsonSerializable(typeof(FileStoresPayload))]
[JsonSourceGenerationOptions(
    PropertyNamingPolicy = null,
    UseStringEnumConverter = true)]
internal partial class LocalApiJsonContext : JsonSerializerContext;
