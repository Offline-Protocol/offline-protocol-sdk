using System.Text.Json;
using OfflineProtocol.Sdk.Model;
using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.LocalApi;

internal static class LocalApiStateCodec
{
    internal static ProtocolState ParseGetStateResponse(string json)
    {
        if (string.IsNullOrWhiteSpace(json))
        {
            throw InvalidResponse("get_state returned empty JSON");
        }

        var trimmed = json.Trim();
        try
        {
            using var document = JsonDocument.Parse(trimmed);
            if (document.RootElement.ValueKind != JsonValueKind.String)
            {
                throw InvalidResponse($"get_state returned JSON that is not a ProtocolState string: {json}");
            }

            var name = document.RootElement.GetString();
            if (name is not null && Enum.TryParse<ProtocolState>(name, ignoreCase: false, out var state))
            {
                return state;
            }

            throw InvalidResponse($"get_state returned unknown ProtocolState value: {json}");
        }
        catch (JsonException)
        {
            if (Enum.TryParse<ProtocolState>(trimmed, ignoreCase: false, out var bare))
            {
                return bare;
            }

            throw InvalidResponse($"get_state returned JSON that is not a ProtocolState: {json}");
        }
    }

    private static ProtocolEngineException InvalidResponse(string message) =>
        new(OfflineProtocolStatus.InvalidArgument, 0, "InvalidResponse", message);
}
