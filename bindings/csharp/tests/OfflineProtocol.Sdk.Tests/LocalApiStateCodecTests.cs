using OfflineProtocol.Sdk.LocalApi;
using OfflineProtocol.Sdk.Model;
using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.Tests;

public sealed class LocalApiStateCodecTests
{
    [Theory]
    [InlineData("\"Stopped\"", ProtocolState.Stopped)]
    [InlineData("Stopped", ProtocolState.Stopped)]
    [InlineData("\"Running\"", ProtocolState.Running)]
    [InlineData("\"Paused\"", ProtocolState.Paused)]
    public void ParseGetStateResponse_accepts_udl_enum_json(string json, ProtocolState expected) =>
        Assert.Equal(expected, LocalApiStateCodec.ParseGetStateResponse(json));

    [Theory]
    [InlineData("{}")]
    [InlineData("""{"state":"Running"}""")]
    [InlineData("[]")]
    [InlineData("0")]
    [InlineData("")]
    [InlineData("   ")]
    public void ParseGetStateResponse_rejects_unexpected_shapes(string json)
    {
        var ex = Assert.Throws<ProtocolEngineException>(() => LocalApiStateCodec.ParseGetStateResponse(json));
        Assert.Equal(OfflineProtocolStatus.InvalidArgument, ex.Status);
        Assert.Equal("InvalidResponse", ex.Variant);
    }
}
