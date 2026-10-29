using OfflineProtocol.Sdk;
using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk.Tests;

public sealed class ProtocolEngineTransportSwapTests
{
    [Fact]
    public async Task Ble_transport_callback_swap_when_native_library_is_present()
    {
        if (string.IsNullOrWhiteSpace(Environment.GetEnvironmentVariable("OFFLINE_PROTOCOL_LIBRARY_DIR")))
        {
            return;
        }

        NativeLibraryLoader.EnsureRegistered();
        var swapCount = 0;
        await using var engine = await ProtocolEngine.CreateAsync(ProtocolEngineTests.Config);
        await engine.SetBleTransportCallbackAsync(() => Interlocked.Increment(ref swapCount));
        await engine.SetBleTransportCallbackAsync(() => Interlocked.Increment(ref swapCount));
        await engine.SetBleTransportCallbackAsync(null);
        Assert.Equal(0, swapCount);
    }
}
