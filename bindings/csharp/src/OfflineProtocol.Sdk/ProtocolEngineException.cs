using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk;

public sealed class ProtocolEngineException : Exception
{
    public ProtocolEngineException(OfflineProtocolStatus status, int code, string? variant, string message)
        : base(message)
    {
        Status = status;
        Code = code;
        Variant = variant;
    }

    public OfflineProtocolStatus Status { get; }
    public int Code { get; }
    public string? Variant { get; }
}
