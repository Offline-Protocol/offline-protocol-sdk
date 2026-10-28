using System.Runtime.InteropServices;
using System.Threading.Channels;
using OfflineProtocol.Sdk.Native;

namespace OfflineProtocol.Sdk;

/// <summary>
/// Host for one in-process engine. All native calls run on a dedicated engine thread (CS1 / C K2).
/// </summary>
public sealed class ProtocolEngine : IAsyncDisposable
{
    /// <summary>Pending engine calls; backpressure when the app outruns the native thread.</summary>
    public const int DefaultQueueCapacity = 512;

    private readonly Channel<IEngineWorkItem> _channel;
    private readonly ChannelWriter<IEngineWorkItem> _writer;
    private readonly ChannelReader<IEngineWorkItem> _reader;
    private readonly Thread _engineThread;
    private IntPtr _handle;
    private int _disposePassStarted;
    private int _rejectNewCalls;

    private ProtocolEngine(int queueCapacity)
    {
        _channel = Channel.CreateBounded<IEngineWorkItem>(
            new BoundedChannelOptions(queueCapacity)
            {
                FullMode = BoundedChannelFullMode.Wait,
                SingleReader = true,
                SingleWriter = false,
            });
        _writer = _channel.Writer;
        _reader = _channel.Reader;
        _engineThread = new Thread(RunEngineLoop)
        {
            Name = "offline-protocol-engine",
            // Background so a forgotten host does not block process exit; UI hosts should
            // call DisposeAsync before shutdown (see docs/bridges/csharp.md).
            IsBackground = true,
        };
        _engineThread.Start();
    }

    /// <summary>Creates an engine from a <c>ProtocolConfig</c> JSON object.</summary>
    public static async Task<ProtocolEngine> CreateAsync(
        string configJson,
        CancellationToken cancellationToken = default,
        int queueCapacity = DefaultQueueCapacity)
    {
        NativeLibraryLoader.EnsureRegistered();
        var engine = new ProtocolEngine(queueCapacity);
        try
        {
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
                cancellationToken).ConfigureAwait(false);
            return engine;
        }
        catch
        {
            engine.ShutdownEngineLoopAfterFailedCreate();
            throw;
        }
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

    public async ValueTask DisposeAsync()
    {
        if (Interlocked.CompareExchange(ref _disposePassStarted, 1, 0) != 0)
        {
            return;
        }

        Volatile.Write(ref _rejectNewCalls, 1);

        try
        {
            await EnqueueOnEngineThreadAsync(
                () =>
                {
                    if (_handle != IntPtr.Zero)
                    {
                        OfflineProtocolNative.offline_protocol_free(_handle);
                        _handle = IntPtr.Zero;
                    }
                },
                CancellationToken.None).ConfigureAwait(false);
        }
        catch (ChannelClosedException)
        {
            // Engine loop already stopped (e.g. after a failed create cleanup).
        }

        _writer.TryComplete();
        _engineThread.Join();
    }

    private void ShutdownEngineLoopAfterFailedCreate()
    {
        Interlocked.CompareExchange(ref _disposePassStarted, 1, 0);
        Volatile.Write(ref _rejectNewCalls, 1);
        _writer.TryComplete();
        _engineThread.Join();
    }

    private Task EnqueueAsync(Action work, CancellationToken cancellationToken)
    {
        ThrowIfEngineClosedToCallers();
        return EnqueueOnEngineThreadAsync(work, cancellationToken);
    }

    private Task<T> EnqueueAsync<T>(Func<T> work, CancellationToken cancellationToken)
    {
        ThrowIfEngineClosedToCallers();
        return EnqueueOnEngineThreadAsync(work, cancellationToken);
    }

    private async Task EnqueueOnEngineThreadAsync(Action work, CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        var item = new VoidEngineWorkItem(work, cancellationToken);
        await WriteWorkAsync(item, cancellationToken).ConfigureAwait(false);
        await item.Completion.ConfigureAwait(false);
    }

    private async Task<T> EnqueueOnEngineThreadAsync<T>(Func<T> work, CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        var item = new ResultEngineWorkItem<T>(work, cancellationToken);
        await WriteWorkAsync(item, cancellationToken).ConfigureAwait(false);
        return await item.Completion.ConfigureAwait(false);
    }

    private async Task WriteWorkAsync(IEngineWorkItem item, CancellationToken cancellationToken)
    {
        try
        {
            await _writer.WriteAsync(item, cancellationToken).ConfigureAwait(false);
        }
        catch
        {
            item.DisposeRegistration();
            throw;
        }
    }

    private void RunEngineLoop()
    {
        try
        {
            while (_reader.WaitToReadAsync().AsTask().GetAwaiter().GetResult())
            {
                while (_reader.TryRead(out var item))
                {
                    item.Run();
                }
            }
        }
        catch (ChannelClosedException)
        {
        }
    }

    private void ThrowIfEngineClosedToCallers()
    {
        if (Volatile.Read(ref _rejectNewCalls) != 0)
        {
            throw new ObjectDisposedException(nameof(ProtocolEngine));
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

    private interface IEngineWorkItem
    {
        void Run();
        void DisposeRegistration();
    }

    private sealed class VoidEngineWorkItem : IEngineWorkItem
    {
        private readonly Action _work;
        private readonly TaskCompletionSource _tcs = new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly CancellationTokenRegistration _cancelRegistration;

        public VoidEngineWorkItem(Action work, CancellationToken cancellationToken)
        {
            _work = work;
            _cancelRegistration = Register(cancellationToken, _tcs);
        }

        public Task Completion => _tcs.Task;

        public void Run()
        {
            try
            {
                _work();
                _tcs.TrySetResult();
            }
            catch (Exception ex)
            {
                _tcs.TrySetException(ex);
            }
            finally
            {
                DisposeRegistration();
            }
        }

        public void DisposeRegistration()
        {
            if (_cancelRegistration != default)
            {
                _cancelRegistration.Dispose();
            }
        }
    }

    private sealed class ResultEngineWorkItem<T> : IEngineWorkItem
    {
        private readonly Func<T> _work;
        private readonly TaskCompletionSource<T> _tcs = new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly CancellationTokenRegistration _cancelRegistration;

        public ResultEngineWorkItem(Func<T> work, CancellationToken cancellationToken)
        {
            _work = work;
            _cancelRegistration = Register(cancellationToken, _tcs);
        }

        public Task<T> Completion => _tcs.Task;

        public void Run()
        {
            try
            {
                _tcs.TrySetResult(_work());
            }
            catch (Exception ex)
            {
                _tcs.TrySetException(ex);
            }
            finally
            {
                DisposeRegistration();
            }
        }

        public void DisposeRegistration()
        {
            if (_cancelRegistration != default)
            {
                _cancelRegistration.Dispose();
            }
        }
    }

    private static CancellationTokenRegistration Register(CancellationToken cancellationToken, TaskCompletionSource tcs) =>
        cancellationToken.CanBeCanceled
            ? cancellationToken.Register(static state =>
            {
                var pair = ((TaskCompletionSource Tcs, CancellationToken Token))state!;
                pair.Tcs.TrySetCanceled(pair.Token);
            },
            (tcs, cancellationToken))
            : default;

    private static CancellationTokenRegistration Register<T>(CancellationToken cancellationToken, TaskCompletionSource<T> tcs) =>
        cancellationToken.CanBeCanceled
            ? cancellationToken.Register(static state =>
            {
                var pair = ((TaskCompletionSource<T> Tcs, CancellationToken Token))state!;
                pair.Tcs.TrySetCanceled(pair.Token);
            },
            (tcs, cancellationToken))
            : default;
}
