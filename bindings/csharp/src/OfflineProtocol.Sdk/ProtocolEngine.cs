using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk;

/// <summary>
/// Host for one in-process engine. All native calls run on a dedicated engine thread (CS1 / C K2).
/// </summary>
public sealed class ProtocolEngine : IAsyncDisposable
{
    private readonly BlockingCollection<Action> _queue = new();
    private readonly Thread _engineThread;
    private IntPtr _handle;
    private bool _disposed;

    private ProtocolEngine()
    {
        _engineThread = new Thread(RunEngineLoop)
        {
            Name = "offline-protocol-engine",
            IsBackground = true,
        };
        _engineThread.Start();
    }

    /// <summary>Creates an engine from a <c>ProtocolConfig</c> JSON object.</summary>
    public static async Task<ProtocolEngine> CreateAsync(string configJson, CancellationToken cancellationToken = default)
    {
        NativeLibraryLoader.EnsureRegistered();
        var engine = new ProtocolEngine();
        await engine.EnqueueAsync(
            () =>
            {
                var err = default(OfflineProtocolError);
                var status = OfflineProtocolNative.offline_protocol_create(configJson, out var handle, ref err);
                if (status != OfflineProtocolStatus.Ok)
                {
                    throw FromError(status, ref err);
                }

                engine._handle = handle;
            },
            cancellationToken);
        return engine;
    }

    public Task CallAsync(string method, string? paramsJson = null, CancellationToken cancellationToken = default) =>
        EnqueueAsync(
            () =>
            {
                var err = default(OfflineProtocolError);
                var status = OfflineProtocolNative.offline_protocol_call(_handle, method, paramsJson, out var resultPtr, ref err);
                if (status != OfflineProtocolStatus.Ok)
                {
                    throw FromError(status, ref err);
                }

                if (resultPtr != IntPtr.Zero)
                {
                    OfflineProtocolNative.offline_protocol_string_free(resultPtr);
                }
            },
            cancellationToken);

    public Task<string> CallWithResultAsync(
        string method,
        string? paramsJson = null,
        CancellationToken cancellationToken = default) =>
        EnqueueAsync(
            () =>
            {
                var err = default(OfflineProtocolError);
                var status = OfflineProtocolNative.offline_protocol_call(_handle, method, paramsJson, out var resultPtr, ref err);
                if (status != OfflineProtocolStatus.Ok)
                {
                    throw FromError(status, ref err);
                }

                return OfflineProtocolNative.TakeUtf8String(resultPtr);
            },
            cancellationToken);

    public Task InitializeMlsWithFileStoresAsync(
        string mlsRoot,
        string stateRoot,
        string storeKeyBase64,
        CancellationToken cancellationToken = default) =>
        CallAsync(
            "initialize_mls_with_file_stores",
            ProtocolConfigJson.FileStores(mlsRoot, stateRoot, storeKeyBase64),
            cancellationToken);

    public Task StartAsync(CancellationToken cancellationToken = default) =>
        CallAsync("start", null, cancellationToken);

    public Task StopAsync(CancellationToken cancellationToken = default) =>
        CallAsync("stop", null, cancellationToken);

    public Task<string> LocalAddressAsync(CancellationToken cancellationToken = default) =>
        CallWithResultAsync("local_address", null, cancellationToken);

    public ValueTask DisposeAsync()
    {
        if (_disposed)
        {
            return ValueTask.CompletedTask;
        }

        _disposed = true;
        var completion = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        _queue.Add(() =>
        {
            try
            {
                if (_handle != IntPtr.Zero)
                {
                    OfflineProtocolNative.offline_protocol_free(_handle);
                    _handle = IntPtr.Zero;
                }

                completion.SetResult();
            }
            catch (Exception ex)
            {
                completion.SetException(ex);
            }
            finally
            {
                _queue.CompleteAdding();
            }
        });
        return new ValueTask(completion.Task.ContinueWith(_ => _engineThread.Join(), TaskScheduler.Default));
    }

    private Task EnqueueAsync(Action work, CancellationToken cancellationToken)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        cancellationToken.ThrowIfCancellationRequested();
        var tcs = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        cancellationToken.Register(() => tcs.TrySetCanceled(cancellationToken));
        _queue.Add(() =>
        {
            try
            {
                work();
                tcs.TrySetResult();
            }
            catch (Exception ex)
            {
                tcs.TrySetException(ex);
            }
        });
        return tcs.Task;
    }

    private Task<T> EnqueueAsync<T>(Func<T> work, CancellationToken cancellationToken)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        cancellationToken.ThrowIfCancellationRequested();
        var tcs = new TaskCompletionSource<T>(TaskCreationOptions.RunContinuationsAsynchronously);
        cancellationToken.Register(() => tcs.TrySetCanceled(cancellationToken));
        _queue.Add(() =>
        {
            try
            {
                tcs.TrySetResult(work());
            }
            catch (Exception ex)
            {
                tcs.TrySetException(ex);
            }
        });
        return tcs.Task;
    }

    private void RunEngineLoop()
    {
        foreach (var work in _queue.GetConsumingEnumerable())
        {
            work();
        }
    }

    private static ProtocolEngineException FromError(OfflineProtocolStatus status, ref OfflineProtocolError err)
    {
        string? variant = err.Variant != IntPtr.Zero ? Marshal.PtrToStringUTF8(err.Variant) : null;
        string message = err.Message != IntPtr.Zero ? Marshal.PtrToStringUTF8(err.Message) ?? "" : "";
        var code = err.Code;
        err.Clear();
        return new ProtocolEngineException(status, code, variant, message);
    }
}
