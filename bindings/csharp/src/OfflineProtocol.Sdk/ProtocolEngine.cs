using System.Runtime.InteropServices;
using System.Threading.Channels;
using OfflineProtocol.Sdk.Native;
using OfflineProtocol.Sdk.Storage;
using OfflineProtocol.Sdk.Telemetry;

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
    private readonly List<NativeStorageProvider> _attachedStorageProviders = [];
    private EventCallbackBridge? _eventBridge;

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

    /// <summary>Instance-less local API call (no engine handle).</summary>
    public static Task<string> CallStaticAsync(
        string method,
        string? paramsJson = null,
        CancellationToken cancellationToken = default) =>
        OffThreadExecution.RunAsync(
            () =>
            {
                NativeLibraryLoader.EnsureRegistered();
                var err = default(OfflineProtocolError);
                var status = OfflineProtocolNative.offline_protocol_call_static(method, paramsJson, out var resultPtr, ref err);
                if (status != OfflineProtocolStatus.Ok)
                {
                    throw FromError(status, ref err);
                }

                return OfflineProtocolNative.TakeUtf8String(resultPtr);
            },
            cancellationToken);

    /// <summary>Runs the protocol-state storage conformance suite against a provider.</summary>
    public static Task<string> RunStorageConformanceAsync(
        NativeStorageProvider storage,
        CancellationToken cancellationToken = default) =>
        OffThreadExecution.RunAsync(
            () =>
            {
                NativeLibraryLoader.EnsureRegistered();
                var err = default(OfflineProtocolError);
                var native = storage.ToNative();
                var status = OfflineProtocolNative.offline_protocol_run_storage_conformance(
                    in native,
                    out var resultPtr,
                    ref err);
                if (status != OfflineProtocolStatus.Ok)
                {
                    throw FromError(status, ref err);
                }

                return OfflineProtocolNative.TakeUtf8String(resultPtr);
            },
            cancellationToken);

    public Task InitializeMlsAsync(
        NativeStorageProvider secureStorage,
        NativeStorageProvider protocolStateStorage,
        CancellationToken cancellationToken = default) =>
        EnqueueAsync<object?>(
            () =>
            {
                var err = default(OfflineProtocolError);
                var secureNative = secureStorage.ToNative();
                var stateNative = protocolStateStorage.ToNative();
                var status = OfflineProtocolNative.offline_protocol_initialize_mls(
                    _handle,
                    in secureNative,
                    in stateNative,
                    ref err);
                if (status != OfflineProtocolStatus.Ok)
                {
                    throw FromError(status, ref err);
                }

                _attachedStorageProviders.Add(secureStorage);
                _attachedStorageProviders.Add(protocolStateStorage);
                return null;
            },
            cancellationToken);

    /// <summary>
    /// Delivers events on a dedicated delivery thread. Keep handlers short and non-blocking:
    /// dispose and callback removal wait for the current handler to finish on that thread.
    /// The handler must not call back into this engine synchronously.
    /// </summary>
    public Task SetEventCallbackAsync(Action<string>? handler, CancellationToken cancellationToken = default) =>
        EnqueueAsync<object?>(
            () =>
            {
                var err = default(OfflineProtocolError);
                if (handler is null)
                {
                    var previous = _eventBridge;
                    _eventBridge = null;
                    previous?.Deactivate();
                    var status = OfflineProtocolNative.offline_protocol_set_event_callback(
                        _handle,
                        IntPtr.Zero,
                        IntPtr.Zero,
                        IntPtr.Zero,
                        ref err);
                    if (status != OfflineProtocolStatus.Ok)
                    {
                        throw FromError(status, ref err);
                    }

                    return null;
                }

                var bridge = new EventCallbackBridge(handler);
                var install = OfflineProtocolNative.offline_protocol_set_event_callback(
                    _handle,
                    bridge.OnEventPointer,
                    bridge.DropPointer,
                    bridge.Context,
                    ref err);
                if (install != OfflineProtocolStatus.Ok)
                {
                    bridge.ReleaseLocal();
                    throw FromError(install, ref err);
                }

                _eventBridge = bridge;
                return null;
            },
            cancellationToken);

    /// <summary>
    /// Events dropped because the delivery queue was full since the current callback was installed.
    /// Use <see cref="PollEventAsync"/> if the app must not miss events under burst load.
    /// </summary>
    public long DroppedEventDeliveryCount => _eventBridge?.DroppedEventCount ?? 0;

    public Task<string?> PollEventAsync(CancellationToken cancellationToken = default) =>
        EnqueueAsync<string?>(
            () =>
            {
                var ptr = OfflineProtocolNative.offline_protocol_poll_event(_handle);
                return ptr == IntPtr.Zero ? null : OfflineProtocolNative.TakeUtf8String(ptr);
            },
            cancellationToken);

    public Task EnableTelemetryAsync(
        TelemetryConfig config,
        AppState initialAppState = AppState.Active,
        CancellationToken cancellationToken = default) =>
        CallAsync(
            "enable_telemetry",
            TelemetryConfigJson.EnableTelemetry(config, initialAppState),
            cancellationToken);

    public Task NotifyAppStateAsync(AppState state, CancellationToken cancellationToken = default) =>
        CallAsync("notify_app_state", $"{{\"state\":\"{state}\"}}", cancellationToken);

    public Task DisableTelemetryAsync(CancellationToken cancellationToken = default) =>
        CallAsync("disable_telemetry", null, cancellationToken);

    public Task EndTelemetrySessionAsync(CancellationToken cancellationToken = default) =>
        CallAsync("end_telemetry_session", null, cancellationToken);

    /// <summary>Blocks the engine thread until telemetry flush completes. Do not await from a UI thread.</summary>
    public Task FlushTelemetryBlockingAsync(CancellationToken cancellationToken = default) =>
        CallAsync("flush_telemetry_blocking", null, cancellationToken);

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
                        var err = default(OfflineProtocolError);
                        var previous = _eventBridge;
                        _eventBridge = null;
                        previous?.Deactivate();
                        OfflineProtocolNative.offline_protocol_set_event_callback(
                            _handle,
                            IntPtr.Zero,
                            IntPtr.Zero,
                            IntPtr.Zero,
                            ref err);
                        OfflineProtocolNative.offline_protocol_free(_handle);
                        _handle = IntPtr.Zero;
                    }

                    _attachedStorageProviders.Clear();
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
