using System.Text.Json;

namespace OfflineProtocol.Sdk;

/// <summary>
/// JSON parameter builders for the calls the SDK wraps.
/// </summary>
/// <remarks>
/// The SDK carries no <c>ProtocolConfig</c> of its own: a value written here would be a default
/// the interface definition can change underneath it (C6). Hosts pass their configuration to
/// <see cref="ProtocolEngine.CreateAsync"/>; the consumer check shows the required fields.
/// </remarks>
public static class ProtocolConfigJson
{
    public static string FileStores(string mlsRoot, string stateRoot, string storeKeyBase64) =>
        JsonSerializer.Serialize(
            new Dictionary<string, string>
            {
                ["mls_root"] = mlsRoot,
                ["state_root"] = stateRoot,
                ["store_key"] = storeKeyBase64,
            });
}
