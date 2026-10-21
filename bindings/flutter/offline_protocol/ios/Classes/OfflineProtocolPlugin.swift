import Flutter
import os
import UIKit

/// Flutter host for the Offline Protocol engine (lifecycle, events, transports).
///
/// The counterpart of Android's `OfflineProtocolPlugin.kt`: this class owns the
/// channels, the event stream and the foreground bookkeeping, and holds one
/// `ProtocolEngineHost` (shared with the React Native module's sources) that
/// owns the core, every transport manager, the data store, mesh services,
/// telemetry and the process scheduler. Every method on the channel is
/// dispatched by `FlutterEngineChannel` into `ProtocolEngineHostBridge`.
///
/// UniFFI calls run on `engineQueue`; Flutter callbacks run on the main queue (bridge K2).
/// Nothing on the main thread waits for the engine queue: engine stop can block on
/// transport I/O, so detach queues the teardown and returns.
public class OfflineProtocolPlugin: NSObject, FlutterPlugin, FlutterStreamHandler {
    private var channel: FlutterMethodChannel?
    private var eventsChannel: FlutterEventChannel?

    private static let log = OSLog(subsystem: "com.offlineprotocol.flutter", category: "engine")

    private let host = ProtocolEngineHost(onDiagnostic: { level, message, context in
        let type: OSLogType
        switch level {
        case "error": type = .error
        case "warning": type = .default
        default: type = .info
        }
        // The context can carry user and peer ids, so it is logged private.
        os_log("%{public}@ %{private}@", log: OfflineProtocolPlugin.log, type: type, message, "\(context)")
    })

    /// One serial queue for every plugin instance in the process. Flutter
    /// registers a new instance for every engine, so a queue per instance would
    /// let a new engine's `create` run while the previous engine's queued
    /// teardown is still blocked on transport I/O: two cores under one identity,
    /// both on BLE and the relay. Shared, the teardown always runs first.
    private static let engineQueue = DispatchQueue(
        label: "com.offlineprotocol.flutter.engine",
        qos: .userInitiated
    )
    private let attachmentLock = NSLock()
    private var isPluginAttached = false

    /// Guards `eventSink`, `eventSeq` and the replay ring together. The sink is
    /// written on the main thread and read on the UniFFI callback thread, and an
    /// unsynchronised read of a reference-typed property is a memory-safety race.
    private let eventLock = NSLock()
    private var eventSeq = 0
    private let eventReplay = EventReplayRing()
    private var eventSink: FlutterEventSink?

    /// Inbound message events Dart could not take (shared `InboundEventBuffer`).
    private let inboundEvents = InboundEventBuffer()

    /// Written on the main queue by the lifecycle observers, read on the engine
    /// queue when telemetry is enabled; the lock makes the pair safe.
    private let appStateLock = NSLock()
    private var appStateValue: AppState = .active

    private let foregroundReconnectPolicy = ForegroundReconnectPolicy()
    private var foregroundObserver: NSObjectProtocol?
    private var backgroundObserver: NSObjectProtocol?

    public static func register(with registrar: FlutterPluginRegistrar) {
        let instance = OfflineProtocolPlugin()
        let channel = FlutterMethodChannel(
            name: "com.offlineprotocol/offline_protocol",
            binaryMessenger: registrar.messenger()
        )
        let events = FlutterEventChannel(
            name: "com.offlineprotocol/offline_protocol_events",
            binaryMessenger: registrar.messenger()
        )
        instance.channel = channel
        instance.eventsChannel = events
        registrar.addMethodCallDelegate(instance, channel: channel)
        events.setStreamHandler(instance)
        registrar.publish(instance)
        instance.setAttached(true)
        instance.installForegroundObserver()
    }

    public func detachFromEngine(for registrar: FlutterPluginRegistrar) {
        attachmentLock.lock()
        guard isPluginAttached else {
            attachmentLock.unlock()
            return
        }
        isPluginAttached = false
        attachmentLock.unlock()

        removeForegroundObserver()
        eventsChannel?.setStreamHandler(nil)
        setEventSink(nil)
        channel?.setMethodCallHandler(nil)
        // Queued behind every call already submitted, never waited on. The block
        // holds `self` until the teardown has run.
        Self.engineQueue.async { [self] in
            self.tearDownProtocol()
        }
    }

    deinit {
        removeForegroundObserver()
        // Directly, never `engineQueue.sync`: every block on that queue holds
        // `self` strongly or weakly, so deinit can run on the engine queue
        // itself, where a sync onto it deadlocks. No block can be running
        // against this instance by the time it is deallocating.
        host.tearDown()
    }

    private func installForegroundObserver() {
        let center = NotificationCenter.default
        backgroundObserver = center.addObserver(
            forName: UIApplication.didEnterBackgroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.setAppState(.background)
            OfflineProtocolPlugin.engineQueue.async {
                self?.foregroundReconnectPolicy.didEnterBackground(
                    nowMs: MonotonicClock.nowMs()
                )
            }
        }
        foregroundObserver = center.addObserver(
            forName: UIApplication.didBecomeActiveNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.setAppState(.active)
            OfflineProtocolPlugin.engineQueue.async {
                self?.handleAppBecameActive()
            }
        }
    }

    private func removeForegroundObserver() {
        let center = NotificationCenter.default
        if let token = foregroundObserver {
            center.removeObserver(token)
            foregroundObserver = nil
        }
        if let token = backgroundObserver {
            center.removeObserver(token)
            backgroundObserver = nil
        }
    }

    private func handleAppBecameActive() {
        if foregroundReconnectPolicy.shouldReconnectOnForeground(nowMs: MonotonicClock.nowMs()) {
            host.internetManager?.forceReconnect()
        }
        DispatchQueue.main.async { [weak self] in
            self?.restateInternetSupersededIfNeeded()
            self?.flushInboundEvents()
        }
    }

    /// Re-emits `internet_session_superseded` while the internet transport latch
    /// is set. Same contract as React Native iOS `restateInternetSupersededIfNeeded`:
    /// the event is level-triggered on foreground, not edge-triggered once.
    private func restateInternetSupersededIfNeeded() {
        guard let eventJson = host.internetManager?.supersedeRestatementJson() else { return }
        emitEventJson(eventJson)
    }

    private func setAppState(_ state: AppState) {
        appStateLock.lock()
        appStateValue = state
        appStateLock.unlock()
    }

    private func currentAppState() -> AppState {
        appStateLock.lock()
        defer { appStateLock.unlock() }
        return appStateValue
    }

    public func onListen(withArguments arguments: Any?, eventSink events: @escaping FlutterEventSink) -> FlutterError? {
        setEventSink(events)
        DispatchQueue.main.async { [weak self] in
            self?.flushInboundEvents()
        }
        return nil
    }

    public func onCancel(withArguments arguments: Any?) -> FlutterError? {
        setEventSink(nil)
        return nil
    }

    private func setEventSink(_ sink: FlutterEventSink?) {
        eventLock.lock()
        eventSink = sink
        eventLock.unlock()
    }

    /// What the bridge needs beyond the host: the pieces that also touch this
    /// plugin's event bookkeeping. Closures hold `self` weakly so a retained
    /// environment never keeps a detached plugin alive.
    private var channelEnvironment: EngineBridgeEnvironment {
        EngineBridgeEnvironment(
            appState: { [weak self] in self?.currentAppState() ?? .active },
            replayEvents: { [weak self] fromSeq, toSeq in
                self?.replayEventsOnEngine(fromSeq: fromSeq, toSeq: toSeq) ?? []
            },
            tearDownProtocol: { [weak self] in self?.tearDownProtocol() },
            createOnEngine: { [weak self] configJson in try self?.createOnEngine(configJson) },
            startOnEngine: { [weak self] in try self?.startOnEngine() },
            stopOnEngine: { [weak self] in try self?.stopOnEngine() }
        )
    }

    public func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
        guard isAttached() else {
            postDetachedError(to: result)
            return
        }
        Self.engineQueue.async { [self] in
            guard self.isAttached() else {
                self.postDetachedError(to: result)
                return
            }
            do {
                let payload = try FlutterEngineChannel.dispatch(
                    host: self.host,
                    env: self.channelEnvironment,
                    method: call.method,
                    arguments: call.arguments
                )
                DispatchQueue.main.async { result(payload) }
            } catch {
                os_log(
                    "%{public}@ failed: %{public}@",
                    log: OfflineProtocolPlugin.log,
                    type: .error,
                    call.method,
                    "\(error)"
                )
                DispatchQueue.main.async {
                    if let mapped = mapProtocolBridgeError(error) {
                        result(FlutterError(code: mapped.code, message: mapped.message, details: nil))
                    } else {
                        result(FlutterError(code: "ERROR", message: error.localizedDescription, details: nil))
                    }
                }
            }
        }
    }

    private func isAttached() -> Bool {
        attachmentLock.lock()
        defer { attachmentLock.unlock() }
        return isPluginAttached
    }

    private func setAttached(_ attached: Bool) {
        attachmentLock.lock()
        isPluginAttached = attached
        attachmentLock.unlock()
    }

    private func postDetachedError(to result: @escaping FlutterResult) {
        DispatchQueue.main.async {
            result(FlutterError(code: "ERROR", message: "Plugin detached", details: nil))
        }
    }

    @discardableResult
    private func emitEventJson(_ eventJson: String) -> Bool {
        eventLock.lock()
        guard let sink = eventSink else {
            eventLock.unlock()
            return false
        }
        let seq = eventSeq
        eventReplay.record(seq: seq, eventJson: eventJson)
        eventSeq += 1
        eventLock.unlock()
        DispatchQueue.main.async {
            sink([
                "eventJson": eventJson,
                "seq": seq,
            ])
        }
        return true
    }

    private func holdInboundEventIfBuffered(_ eventJson: String, generation: Int) {
        guard let key = inboundEvents.key(forEventJson: eventJson) else { return }
        inboundEvents.hold(key: key, eventJson: eventJson, generation: generation)
    }

    private func flushInboundEvents() {
        guard eventSink != nil, !inboundEvents.isEmpty else { return }
        let drained = inboundEvents.drain()
        var undelivered: [InboundEventBuffer.Entry] = []
        for entry in drained {
            guard entry.generation == inboundEvents.currentGeneration() else { continue }
            if !emitEventJson(entry.eventJson) {
                undelivered.append(entry)
            }
        }
        inboundEvents.restore(undelivered)
    }

    /// Core `EventCallback` delivery with inbound hold when Dart is not listening.
    fileprivate func deliverCoreEvent(_ eventJson: String) {
        let generation = inboundEvents.currentGeneration()
        if !emitEventJson(eventJson) {
            holdInboundEventIfBuffered(eventJson, generation: generation)
        }
    }

    private func replayEventsOnEngine(fromSeq: Int, toSeq: Int) -> [[String: Any]] {
        eventLock.lock()
        let replayed = eventReplay.replay(fromSeq: fromSeq, toSeq: toSeq)
        eventLock.unlock()
        return replayed.map { ["seq": $0.seq, "eventJson": $0.eventJson] as [String: Any] }
    }

    private func createOnEngine(_ configJson: String) throws {
        // Validated before the running engine is torn down: a config this
        // plugin refuses must leave the current session as it was.
        _ = try ProtocolConfigParser.parse(configJson)
        tearDownProtocol()
        try host.create(
            configJson: configJson,
            eventCallback: FlutterEventCallback(plugin: self),
            emitEventJson: { [weak self] json in
                _ = self?.emitEventJson(json)
            }
        )
    }

    private func startOnEngine() throws {
        try host.startProtocolCore()
        try host.startBleTransportIfPresent()
    }

    /// The scheduler and the transports stop first and cannot skip each other;
    /// the first failure reaches Dart, as Android's `TeardownSequence` reports.
    private func stopOnEngine() throws {
        try host.stopTransportsAndProtocol()
    }

    private func tearDownProtocol() {
        host.tearDown()
        eventLock.lock()
        eventSeq = 0
        eventReplay.clear()
        eventLock.unlock()
        inboundEvents.bumpGeneration()
    }
}

private final class FlutterEventCallback: EventCallback, @unchecked Sendable {
    weak var plugin: OfflineProtocolPlugin?

    init(plugin: OfflineProtocolPlugin) {
        self.plugin = plugin
    }

    func onEvent(eventJson: String) {
        let deliver = { [weak self] in
            self?.plugin?.deliverCoreEvent(eventJson)
        }
        if Thread.isMainThread {
            deliver()
        } else {
            DispatchQueue.main.async(execute: deliver)
        }
    }
}
