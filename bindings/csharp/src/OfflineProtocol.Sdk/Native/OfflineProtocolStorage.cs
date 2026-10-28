using System.Runtime.InteropServices;

namespace OfflineProtocol.Sdk.Native;

[StructLayout(LayoutKind.Sequential)]
internal struct OfflineProtocolStorage
{
    public IntPtr Ctx;
    public IntPtr Store;
    public IntPtr Load;
    public IntPtr Remove;
    public IntPtr ListKeys;
    public IntPtr Release;
    public IntPtr Drop;
}
