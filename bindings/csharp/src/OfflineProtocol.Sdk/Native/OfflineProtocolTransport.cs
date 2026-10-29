namespace OfflineProtocol.Sdk.Native;

/// <summary>
/// Matches <c>OfflineProtocolTransport</c> in <c>offline_protocol.h</c> (append-only).
/// </summary>
public enum OfflineProtocolTransport : int
{
    Ble = 0,
    WifiDirect = 1,
    Reticulum = 2,
    Nostr = 3,
}
