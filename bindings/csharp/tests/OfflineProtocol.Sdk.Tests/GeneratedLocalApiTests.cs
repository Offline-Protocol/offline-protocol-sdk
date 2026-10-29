using OfflineProtocol.Sdk.LocalApi;
using OfflineProtocol.Sdk.Model;

namespace OfflineProtocol.Sdk.Tests;

/// <summary>
/// The generated wrappers against the real native library: each reaches the method its name
/// says, through the prefix the C API dispatches on.
/// </summary>
public sealed class GeneratedLocalApiTests
{
    [Fact]
    public async Task A_typed_config_creates_an_engine_that_reports_its_state()
    {
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolConfigJsonTests.Required());

        Assert.Equal("\"Stopped\"", await engine.GetStateAsync());
        Assert.Equal(ProtocolState.Stopped, await LocalApiTyped.GetStateAsync(engine));
    }

    [Fact]
    public async Task Poll_event_binds_to_the_hand_written_method_and_returns_null_when_empty()
    {
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);

        // A generated PollEventAsync(string?, CancellationToken) beside the hand-written one made
        // this call ambiguous; the hand-written one maps an empty queue to null.
        string? polled = await engine.PollEventAsync();
        Assert.Null(polled);
    }

    [Fact]
    public async Task A_namespace_function_needs_no_engine()
    {
        var address = await ProtocolEngine.DeriveAddressAsync(
            """{"public_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}""");

        Assert.StartsWith("\"off1", address, StringComparison.Ordinal);
    }

    [Fact]
    public async Task The_data_and_services_clients_reach_their_prefixed_methods()
    {
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);

        await AssertDispatched(() => engine.Data.ListDocsAsync("{}"));
        await AssertDispatched(() => engine.Services.DiscoverServicesAsync("{}"));
    }

    // Success or a refusal of the parameters both prove the wire name resolved; only an
    // unknown method means the generated prefix is wrong.
    private static async Task AssertDispatched(Func<Task> call)
    {
        try
        {
            await call();
        }
        catch (ProtocolEngineException ex)
        {
            Assert.NotEqual(Native.OfflineProtocolStatus.MethodNotFound, ex.Status);
        }
    }
}
