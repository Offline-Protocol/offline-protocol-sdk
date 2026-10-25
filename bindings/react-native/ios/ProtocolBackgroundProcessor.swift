//
// ProtocolBackgroundProcessor.swift
//
// Fallback tick that drives `OfflineProtocol.process()` and drains
// `OfflineProtocol.receiveMessage()`. Shared by the React Native module and the
// Flutter plugin through `ProtocolEngineHost` (ADR 0025). Mirrors
// android/ ProtocolBackgroundProcessor.kt: keep the interval, the drain cap and
// the fixed-delay scheduling in sync. No React import: the Swift package
// assembly refuses one.
//
// Latency-sensitive work is still event-driven through the transport callbacks;
// this scheduler is the safety net.
//

import Foundation

final class ProtocolBackgroundProcessor {
    /// Matches Android's 100ms tick.
    static let processIntervalMs = 100
    static let maxReceiveDrainPerTick = 100
    private static let shutdownTimeoutSeconds: TimeInterval = 2.0

    private let protocolProvider: () -> OfflineProtocol?
    private let onDiagnostic: ProtocolEngineHost.DiagnosticHandler

    /// Serial, so a block enqueued behind a tick cannot run until that tick has
    /// returned: `stopProcessScheduler` relies on it to wait out a straggler.
    private let processQueue = DispatchQueue(
        label: "com.offlineprotocol.engine.process",
        qos: .utility
    )
    private let lock = NSLock()
    private var timer: DispatchSourceTimer?

    init(
        protocolProvider: @escaping () -> OfflineProtocol?,
        onDiagnostic: @escaping ProtocolEngineHost.DiagnosticHandler
    ) {
        self.protocolProvider = protocolProvider
        self.onDiagnostic = onDiagnostic
    }

    /// Starts the process scheduler, replacing a running one.
    ///
    /// Fixed *delay*, not fixed rate. A repeating timer targets a constant
    /// frequency, so when a tick overruns the period the next one fires the
    /// moment it returns, with no gap. Every tick takes the core protocol mutex
    /// for its whole body (`process()` plus a `receiveMessage()` drain), so an
    /// overrunning tick on a slow device holds that mutex essentially
    /// continuously and starves every other FFI caller, the BLE fragment drain
    /// included (OFF-2123). The timer here is one-shot and re-armed after each
    /// tick, which guarantees a full interval of slack between ticks however
    /// long one takes.
    func startProcessScheduler() {
        stopProcessScheduler()
        let source = DispatchSource.makeTimerSource(queue: processQueue)
        source.setEventHandler { [weak self, weak source] in
            guard let self = self, let source = source, !source.isCancelled else { return }
            self.processProtocol()
            guard !source.isCancelled else { return }
            source.schedule(
                deadline: .now() + .milliseconds(Self.processIntervalMs),
                leeway: .milliseconds(10)
            )
        }
        source.schedule(deadline: .now())
        lock.lock()
        timer = source
        lock.unlock()
        source.resume()
    }

    /// Stops the scheduler and waits, bounded, for a tick already running.
    ///
    /// `cancel()` only stops *future* firings; a handler mid-flight keeps going
    /// and holds the protocol instance, so it can still persist after the
    /// caller has gone on to release or wipe it. Never called from
    /// `processQueue`. Not waited on from the main thread, where a stall behind
    /// the core mutex is an App Hang; the teardown paths run on the engine
    /// queue, and only a plugin deinit can reach this from main.
    func stopProcessScheduler() {
        lock.lock()
        let source = timer
        timer = nil
        lock.unlock()
        guard let source = source else { return }
        source.cancel()
        guard !Thread.isMainThread else { return }
        let drained = DispatchSemaphore(value: 0)
        processQueue.async { drained.signal() }
        _ = drained.wait(timeout: .now() + Self.shutdownTimeoutSeconds)
    }

    private func processProtocol() {
        guard let instance = protocolProvider() else { return }
        do {
            try instance.process()
            var drained = 0
            while drained < Self.maxReceiveDrainPerTick {
                guard instance.receiveMessage() != nil else { break }
                drained += 1
            }
            if drained >= Self.maxReceiveDrainPerTick {
                onDiagnostic(
                    "warning",
                    "Capped receiveMessage drain for this process tick",
                    ["maxBatch": Self.maxReceiveDrainPerTick]
                )
            }
        } catch {
            onDiagnostic(
                "error",
                "Protocol process error",
                [
                    "error": error.localizedDescription,
                    "exception": String(describing: type(of: error)),
                ]
            )
        }
    }
}
