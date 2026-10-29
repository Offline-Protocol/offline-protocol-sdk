using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.Tests;

public sealed class OfflineProtocolTransportTests
{
    [Fact]
    public void Values_match_offline_protocol_h()
    {
        Assert.Equal(0, (int)OfflineProtocolTransport.Ble);
        Assert.Equal(1, (int)OfflineProtocolTransport.WifiDirect);
        Assert.Equal(2, (int)OfflineProtocolTransport.Reticulum);
        Assert.Equal(3, (int)OfflineProtocolTransport.Nostr);
    }
}
