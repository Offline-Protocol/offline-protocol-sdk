using System.Text.Json.Serialization;
using OfflineProtocol.Sdk.LocalApi;

namespace OfflineProtocol.Sdk.Json;

// Wire names come from [JsonPropertyName] on each payload; enums cross as their UDL spelling.
[JsonSerializable(typeof(SendMessagePayload))]
[JsonSerializable(typeof(FileStoresPayload))]
[JsonSourceGenerationOptions(UseStringEnumConverter = true)]
internal partial class LocalApiJsonContext : JsonSerializerContext;
