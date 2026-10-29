using System.Runtime.InteropServices;

namespace OfflineProtocol.Sdk.Native;

[StructLayout(LayoutKind.Sequential)]
internal struct OfflineProtocolError
{
    public OfflineProtocolStatus Status;
    public int Code;
    public IntPtr Variant;
    public IntPtr Message;

    public void Clear()
    {
        OfflineProtocolNative.offline_protocol_error_clear(ref this);
    }
}
