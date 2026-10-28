namespace OfflineProtocol.Sdk.Storage;

/// <summary>
/// Managed storage backend for <see cref="NativeStorageProvider"/> (MLS secure material and protocol state).
/// </summary>
public interface IProtocolStorage
{
    OfflineProtocolStorageStatus Store(string keyType, string keyId, ReadOnlySpan<byte> data);

    /// <summary>
    /// Returns <see langword="null"/> when the key is absent (success with no data).
    /// </summary>
    byte[]? Load(string keyType, string keyId);

    OfflineProtocolStorageStatus Remove(string keyType, string keyId);

    /// <summary>Key ids under <paramref name="keyType"/> (used to build the JSON array for the native layer).</summary>
    IReadOnlyList<string> ListKeyIds(string keyType);
}
