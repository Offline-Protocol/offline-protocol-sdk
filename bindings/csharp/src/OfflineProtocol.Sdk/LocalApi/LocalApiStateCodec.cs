using System.Text.Json;
using OfflineProtocol.Sdk.Model;
using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.LocalApi;

internal static class LocalApiStateCodec
{
    /// <summary>
    /// <c>get_state</c> answers a JSON string naming a <see cref="ProtocolState"/>. Anything else
    /// is refused rather than coerced, so a change in the dispatcher's encoding fails here.
    /// </summary>
    internal static ProtocolState ParseGetStateResponse(string json)
    {
        string? name;
        try
        {
            using var document = JsonDocument.Parse(json);
            if (document.RootElement.ValueKind != JsonValueKind.String)
            {
                throw InvalidResponse($"get_state returned JSON that is not a ProtocolState string: {json}");
            }

            name = document.RootElement.GetString();
        }
        catch (JsonException)
        {
            throw InvalidResponse($"get_state returned text that is not JSON: {json}");
        }

        // TryParse also takes "1" or "Stopped, Running"; only an exact member name is a state.
        if (name is not null && Enum.TryParse<ProtocolState>(name, ignoreCase: false, out var state)
            && state.ToString() == name)
        {
            return state;
        }

        throw InvalidResponse($"get_state returned unknown ProtocolState value: {json}");
    }

    private static ProtocolEngineException InvalidResponse(string message) =>
        new(OfflineProtocolStatus.InvalidArgument, 0, "InvalidResponse", message);
}
