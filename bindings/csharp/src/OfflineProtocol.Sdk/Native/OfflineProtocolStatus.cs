namespace OfflineProtocol.Sdk.Native;

/// <summary>Matches <c>OfflineProtocolStatus</c> in <c>offline_protocol.h</c>.</summary>
public enum OfflineProtocolStatus : int
{
    Ok = 0,
    EngineError = 1,
    InvalidArgument = 2,
    MethodNotFound = 3,
    InvalidParams = 4,
    Internal = 5,
}
