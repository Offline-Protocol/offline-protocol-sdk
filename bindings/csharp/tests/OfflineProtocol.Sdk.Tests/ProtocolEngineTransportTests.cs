using OfflineProtocol.Sdk;
using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.Tests;

public sealed class ProtocolEngineTransportTests
{
    [Theory]
    [InlineData(OfflineProtocolTransport.Ble, true)]
    [InlineData(OfflineProtocolTransport.Nostr, true)]
    [InlineData((OfflineProtocolTransport)99, false)]
    public void IsKnownTransportCarrier_matches_offline_protocol_h(OfflineProtocolTransport transport, bool expected)
    {
        Assert.Equal(expected, ProtocolEngine.IsKnownTransportCarrier(transport));
    }
}
