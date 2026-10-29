using OfflineProtocol.Sdk.Model;

namespace OfflineProtocol.Sdk.LocalApi;

/// <summary>Typed helpers on top of generated local API methods.</summary>
public static class LocalApiTyped
{
    public static async Task<ProtocolState> GetStateAsync(
        ProtocolEngine engine,
        CancellationToken cancellationToken = default)
    {
        var json = await engine.GetStateAsync(cancellationToken: cancellationToken).ConfigureAwait(false);
        return LocalApiStateCodec.ParseGetStateResponse(json);
    }

    public static Task<string> SendMessageAsync(
        ProtocolEngine engine,
        string recipient,
        string content,
        MessagePriority priority = MessagePriority.Medium,
        CancellationToken cancellationToken = default) =>
        engine.SendMessageAsync(LocalApiParams.SendMessage(recipient, content, priority), cancellationToken);

    /// <summary>Send with caller-owned JSON (no extra allocation beyond the engine queue).</summary>
    public static Task<string> SendMessageAsync(
        ProtocolEngine engine,
        string parametersJson,
        CancellationToken cancellationToken = default) =>
        engine.SendMessageAsync(parametersJson, cancellationToken);
}
