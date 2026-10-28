using System.Collections.Concurrent;
using System.Text.Json;

namespace OfflineProtocol.Sdk.Storage;

/// <summary>
/// Thread-safe in-memory storage for tests, consumer-check, and hosts without a platform keystore yet.
/// </summary>
public sealed class MemoryProtocolStorage : IProtocolStorage
{
    private readonly ConcurrentDictionary<(string Type, string Id), byte[]> _entries = new();

    public OfflineProtocolStorageStatus Store(string keyType, string keyId, ReadOnlySpan<byte> data)
    {
        try
        {
            _entries[(keyType, keyId)] = data.ToArray();
            return OfflineProtocolStorageStatus.Ok;
        }
        catch
        {
            return OfflineProtocolStorageStatus.StoreFailed;
        }
    }

    public byte[]? Load(string keyType, string keyId)
    {
        return _entries.TryGetValue((keyType, keyId), out var data) ? data : null;
    }

    public OfflineProtocolStorageStatus Remove(string keyType, string keyId)
    {
        _entries.TryRemove((keyType, keyId), out _);
        return OfflineProtocolStorageStatus.Ok;
    }

    public IReadOnlyList<string> ListKeyIds(string keyType)
    {
        return _entries.Keys
            .Where(k => k.Type == keyType)
            .Select(k => k.Id)
            .OrderBy(id => id, StringComparer.Ordinal)
            .ToArray();
    }

    internal string ListKeysJson(string keyType) =>
        JsonSerializer.Serialize(ListKeyIds(keyType));
}
