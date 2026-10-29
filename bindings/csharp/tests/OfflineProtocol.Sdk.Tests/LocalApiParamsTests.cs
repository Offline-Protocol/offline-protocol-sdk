using System.Text.Json;
using OfflineProtocol.Sdk.LocalApi;
using OfflineProtocol.Sdk.Model;

namespace OfflineProtocol.Sdk.Tests;

public sealed class LocalApiParamsTests
{
    [Fact]
    public void SendMessage_serializes_priority_as_udl_enum_string()
    {
        var json = LocalApiParams.SendMessage("off1abc", "hi", MessagePriority.High);

        using var doc = JsonDocument.Parse(json);
        Assert.Equal("High", doc.RootElement.GetProperty("priority").GetString());
    }
}
