using System.Text.Json;
using System.Text.Json.Serialization;

namespace OfflineProtocol.Sdk.Json;

internal static class SdkJson
{
    internal static JsonSerializerOptions SerializeOptions { get; } = CreateSerializeOptions();

    private static JsonSerializerOptions CreateSerializeOptions()
    {
        var options = new JsonSerializerOptions(JsonSerializerDefaults.General);
        options.Converters.Add(new JsonStringEnumConverter());
        return options;
    }
}
