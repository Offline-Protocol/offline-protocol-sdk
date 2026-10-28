namespace OfflineProtocol.Sdk;

/// <summary>Minimal <c>ProtocolConfig</c> JSON for scaffold and consumer-check (PR1).</summary>
public static class ProtocolConfigJson
{
    /// <summary>Fields without a UDL default, with definition defaults for the rest (see bindings/c/README.md).</summary>
    public const string MinimalConsumer =
        """
        {
          "app_id": "com.offlineprotocol.csharp.consumer",
          "profile": "csharp-consumer",
          "ble_enabled": false,
          "wifi_direct_enabled": false,
          "internet_enabled": true,
          "reticulum_enabled": false,
          "nostr_enabled": false,
          "prefer_online": false,
          "initial_ttl": 8,
          "encryption_enabled": true,
          "auto_key_exchange": true,
          "store_pending": true,
          "max_pending_per_peer": 64,
          "max_pending_global": 4096,
          "pending_ttl_ms": 60000,
          "overflow_policy": "DropOldest"
        }
        """;

    public static string FileStores(string mlsRoot, string stateRoot, string storeKeyBase64) =>
        $$"""
        {"mls_root":"{{Escape(mlsRoot)}}","state_root":"{{Escape(stateRoot)}}","store_key":"{{storeKeyBase64}}"}
        """;

    private static string Escape(string path) => path.Replace("\\", "\\\\", StringComparison.Ordinal);
}
