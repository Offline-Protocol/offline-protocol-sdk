namespace OfflineProtocol.Sdk.Storage;

/// <summary>
/// Storage operation result codes (append-only; matches the C API and UDL order).
/// </summary>
public enum OfflineProtocolStorageStatus
{
    Ok = 0,
    StoreFailed = 1,
    LoadFailed = 2,
    DeleteFailed = 3,
    KeyNotFound = 4,
    CorruptedData = 5,
}
