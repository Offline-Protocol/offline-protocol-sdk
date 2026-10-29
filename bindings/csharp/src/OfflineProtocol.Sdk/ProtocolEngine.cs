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

    private readonly ChannelWriter<IEngineWorkItem> _writer;
    private readonly ChannelReader<IEngineWorkItem> _reader;
    private readonly TaskCompletionSource _engineThreadExited = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private readonly TaskCompletionSource _outboundDrained = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private IntPtr _handle;
    private Task? _disposal;
    private int _rejectNewCalls;

    /// <summary>
    /// Calls accepted and not yet finished: waiting to enter the channel, queued, or running.
    /// </summary>
    private int _outboundWork;

    private ProtocolEngine(int queueCapacity)
    {
        var channel = Channel.CreateBounded<IEngineWorkItem>(
            new BoundedChannelOptions(queueCapacity)
            {
                FullMode = BoundedChannelFullMode.Wait,
                SingleReader = true,
                SingleWriter = false,
            });
        _writer = channel.Writer;
        _reader = channel.Reader;
        var engineThread = new Thread(RunEngineLoop)
        {
            Name = "offline-protocol-engine",
            // Background so a forgotten host does not block process exit; UI hosts should
            // call DisposeAsync before shutdown (see docs/bridges/csharp.md).
            IsBackground = true,
        };
        engineThread.Start();
    }

    /// <summary>The native library's version, such as <c>"0.31.0"</c>. Needs no engine.</summary>
    public static string LibraryVersion => OfflineProtocolNative.Version();

    /// <summary>Creates an engine from a <c>ProtocolConfig</c> JSON object.</summary>
    /// <remarks>
    /// On any failure, a cancellation included, the engine thread is shut down and a handle the
    /// native create still produced after the caller stopped waiting is freed, not leaked.
    /// </remarks>
    public static async Task<ProtocolEngine> CreateAsync(
        string configJson,
        CancellationToken cancellationToken = default,
        int queueCapacity = DefaultQueueCapacity)
    {
        NativeLibraryLoader.EnsureRegistered();
        var engine = new ProtocolEngine(queueCapacity);
        try
        {
            await engine.EnqueueAsync<object?>(
                () =>
                {
                    var err = default(OfflineProtocolError);
                    var status = OfflineProtocolNative.offline_protocol_create(configJson, out var handle, ref err);
                    if (status != OfflineProtocolStatus.Ok)
                    {
                        throw FromError(status, ref err);
                    }

                    engine._handle = handle;
                    return null;
                },
                cancellationToken).ConfigureAwait(false);
            return engine;
        }
        catch
        {
            await engine.DisposeAsync().ConfigureAwait(false);
            throw;
        }
    }

    /// <summary>Calls a method named as the local API names it, discarding its result.</summary>
    public async Task CallAsync(string method, string? paramsJson = null, CancellationToken cancellationToken = default) =>
        _ = await CallWithResultAsync(method, paramsJson, cancellationToken).ConfigureAwait(false);

    /// <summary>Calls a method named as the local API names it and returns its result as JSON.</summary>
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

    /// <summary>
    /// Refuses new calls, lets every accepted call finish on the engine thread, then frees the
    /// native handle there. Never blocks the calling thread: a UI thread may await it. Every
    /// caller, the first and any concurrent one, awaits the same shutdown.
    /// </summary>
    public ValueTask DisposeAsync()
    {
        var disposal = Volatile.Read(ref _disposal);
        if (disposal is null)
        {
            var started = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
            disposal = Interlocked.CompareExchange(ref _disposal, started.Task, null);
            if (disposal is null)
            {
                disposal = started.Task;
                _ = RunDisposalAsync(started);
            }
        }

        return new ValueTask(disposal);
    }

    private async Task RunDisposalAsync(TaskCompletionSource done)
    {
        try
        {
            Volatile.Write(ref _rejectNewCalls, 1);
            await WaitForNoOutboundWorkAsync().ConfigureAwait(false);

            // The channel is FIFO with one reader and every accepted call has finished, so the
            // free runs after the last call that could use the handle.
            var free = new EngineWorkItem<object?>(
                () =>
                {
                    if (_handle != IntPtr.Zero)
                    {
                        OfflineProtocolNative.offline_protocol_free(_handle);
                        _handle = IntPtr.Zero;
                    }

                    return null;
                },
                CancellationToken.None,
                static () => { });
            await _writer.WriteAsync(free, CancellationToken.None).ConfigureAwait(false);
            await free.Completion.ConfigureAwait(false);

            _writer.TryComplete();
            await _engineThreadExited.Task.ConfigureAwait(false);
            done.TrySetResult();
        }
        catch (Exception ex)
        {
            done.TrySetException(ex);
        }
    }

    private async Task<T> EnqueueAsync<T>(Func<T> work, CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        if (!TryBeginOutboundWork())
        {
            throw new ObjectDisposedException(nameof(ProtocolEngine));
        }

        var item = new EngineWorkItem<T>(work, cancellationToken, CompleteOutboundWork);
        try
        {
            await _writer.WriteAsync(item, cancellationToken).ConfigureAwait(false);
        }
        catch
        {
            // Never entered the channel, so it will never run to complete itself.
            item.DisposeRegistration();
            CompleteOutboundWork();
            throw;
        }

        return await item.Completion.ConfigureAwait(false);
    }

    /// <summary>
    /// Dedicated engine thread: block here (not on the thread pool) while waiting for channel items (CS1).
    /// </summary>
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
        finally
        {
            _engineThreadExited.TrySetResult();
        }
    }

    private bool TryBeginOutboundWork()
    {
        if (Volatile.Read(ref _rejectNewCalls) != 0)
        {
            return false;
        }

        Interlocked.Increment(ref _outboundWork);
        // Disposal may have started between the check and the increment; back out so it does
        // not wait on a call it never accepted.
        if (Volatile.Read(ref _rejectNewCalls) != 0)
        {
            CompleteOutboundWork();
            return false;
        }

        return true;
    }

    private void CompleteOutboundWork()
    {
        if (Interlocked.Decrement(ref _outboundWork) == 0 && Volatile.Read(ref _rejectNewCalls) != 0)
        {
            _outboundDrained.TrySetResult();
        }
    }

    /// <summary>
    /// Completes once no accepted call is outstanding. Called after new calls are refused, so the
    /// count only falls: whichever of this check and the last decrement sees zero second signals.
    /// </summary>
    private Task WaitForNoOutboundWorkAsync()
    {
        if (Volatile.Read(ref _outboundWork) == 0)
        {
            _outboundDrained.TrySetResult();
        }

        return _outboundDrained.Task;
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
    }

    private sealed class EngineWorkItem<T> : IEngineWorkItem
    {
        private readonly Func<T> _work;
        private readonly Action _onRunComplete;
        private readonly TaskCompletionSource<T> _tcs = new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly CancellationTokenRegistration _cancelRegistration;

        public EngineWorkItem(Func<T> work, CancellationToken cancellationToken, Action onRunComplete)
        {
            _work = work;
            _onRunComplete = onRunComplete;
            // Cancellation ends the caller's wait only. A queued item still runs (CS1), so a
            // native create or free is never skipped behind the caller's back.
            _cancelRegistration = cancellationToken.CanBeCanceled
                ? cancellationToken.Register(
                    static state =>
                    {
                        var (tcs, token) = ((TaskCompletionSource<T>, CancellationToken))state!;
                        tcs.TrySetCanceled(token);
                    },
                    (_tcs, cancellationToken))
                : default;
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
                _onRunComplete();
            }
        }

        public void DisposeRegistration() => _cancelRegistration.Dispose();
    }
}
