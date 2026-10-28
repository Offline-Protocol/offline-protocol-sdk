using System.Collections.Concurrent;

namespace OfflineProtocol.Sdk.Storage;

/// <summary>
/// Thread-safe in-memory storage for tests, consumer-check, and hosts without a platform keystore yet.
/// </summary>
public sealed class MemoryProtocolStorage : IProtocolStorage
{
    private readonly ConcurrentDictionary<string, ConcurrentDictionary<string, byte[]>> _byType = new();

    public OfflineProtocolStorageStatus Store(string keyType, string keyId, ReadOnlySpan<byte> data)
    {
        try
        {
            var bucket = _byType.GetOrAdd(keyType, static _ => new ConcurrentDictionary<string, byte[]>());
            bucket[keyId] = data.ToArray();
            return OfflineProtocolStorageStatus.Ok;
        }
        catch
        {
            return OfflineProtocolStorageStatus.StoreFailed;
        }
    }

    public byte[]? Load(string keyType, string keyId)
    {
        if (!_byType.TryGetValue(keyType, out var bucket))
        {
            return null;
        }

        return bucket.TryGetValue(keyId, out var data) ? data : null;
    }

    public OfflineProtocolStorageStatus Remove(string keyType, string keyId)
    {
        if (_byType.TryGetValue(keyType, out var bucket))
        {
            bucket.TryRemove(keyId, out _);
        }

        return OfflineProtocolStorageStatus.Ok;
    }

    public IReadOnlyList<string> ListKeyIds(string keyType)
    {
        if (!_byType.TryGetValue(keyType, out var bucket))
        {
            return Array.Empty<string>();
        }

        return bucket.Keys.OrderBy(id => id, StringComparer.Ordinal).ToArray();
    }
}
