using OfflineProtocol.Sdk.Model;

namespace OfflineProtocol.Sdk.LocalApi;

/// <summary>Typed helpers on top of the generated local API methods.</summary>
public static class LocalApiTyped
{
    public static async Task<ProtocolState> GetStateAsync(
        ProtocolEngine engine,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(engine);
        var json = await engine.GetStateAsync(cancellationToken: cancellationToken).ConfigureAwait(false);
        return LocalApiStateCodec.ParseGetStateResponse(json);
    }

    /// <summary>Sends a direct message and returns the engine's message id JSON.</summary>
    public static Task<string> SendMessageAsync(
        ProtocolEngine engine,
        string recipient,
        string content,
        MessagePriority priority = MessagePriority.Medium,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(engine);
        return engine.SendMessageAsync(LocalApiParams.SendMessage(recipient, content, priority), cancellationToken);
    }
}
