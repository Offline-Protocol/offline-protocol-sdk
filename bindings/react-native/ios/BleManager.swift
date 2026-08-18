//
// BleManager.swift
// OfflineProtocol
//
// BLE transport implementation using CoreBluetooth
// Supports iOS ↔ Android cross-platform communication
//

import Foundation
import CoreBluetooth
import UIKit

private final class LogThrottler {
    private var timestamps: [String: Date] = [:]
    private let lock = NSLock()
    private let defaultInterval: TimeInterval
    
    init(defaultInterval: TimeInterval = 5.0) {
        self.defaultInterval = defaultInterval
    }
    
    func shouldLog(key: String, interval: TimeInterval? = nil, now: Date = Date()) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        let threshold = interval ?? defaultInterval
        if let last = timestamps[key], now.timeIntervalSince(last) < threshold {
            return false
        }
        timestamps[key] = now
        return true
    }
}

/// BLE Manager implementing TransportManager for Bluetooth Low Energy communication
public class BleManager: NSObject, TransportManager {
    
    // MARK: - TransportManager Protocol
    
    public let transportId = "ble"
    public let transportName = "Bluetooth Low Energy"
    public private(set) var state: TransportState = .unavailable
    public weak var delegate: TransportManagerDelegate?
    
    // MARK: - BLE Constants (matching Rust core and Android)
    
    private let SERVICE_UUID = CBUUID(string: "6E400001-B5A3-F393-E0A9-E50E24DCCA9E")
    private let MESSAGE_CHAR_UUID = CBUUID(string: "6E400002-B5A3-F393-E0A9-E50E24DCCA9E")
    private let DEVICE_ID_CHAR_UUID = CBUUID(string: "6E400003-B5A3-F393-E0A9-E50E24DCCA9E")
    private let IDENTITY_CHAR_UUID = CBUUID(string: "6E400004-B5A3-F393-E0A9-E50E24DCCA9E")
    
    // Fragment sizing is fully owned by the Rust transport now: it stores
    // a per-peer maximum usable payload seeded from
    // `CBPeripheral.maximumWriteValueLength(for: .withoutResponse)` via
    // `bleSetPeerMtu` on device-id resolution, and falls back to its
    // internal BLE_MAX_FRAGMENT_SIZE (185) for any peer whose MTU has not
    // been reported yet. Keeping the constant here would duplicate the
    // floor and go stale the first time Rust changes it.
    private let CONNECTION_TIMEOUT: TimeInterval = 10.0
    private let MAX_CONNECTIONS_PER_DEVICE = 4
    private let ADVERTISE_RESTART_MIN: TimeInterval = 0.2
    private let ADVERTISE_RESTART_MAX: TimeInterval = 1.2
    private let MIN_ADVERTISE_INTERVAL: TimeInterval = 1.5
    private let LOAD_SATURATION_COUNT = 20
    private let MESH_OBSERVATION_TTL: TimeInterval = 120.0
    
    // MARK: - Adaptive Scan Configuration
    
    /// Minimum RSSI to consider for connection (filter weak signals early)
    private let ADAPTIVE_MIN_RSSI: Int16 = -85
    /// Peer count threshold below which we process all advertisements
    private let ADAPTIVE_LOW_DENSITY_THRESHOLD = 10
    /// Peer count threshold above which we apply maximum throttling
    private let ADAPTIVE_HIGH_DENSITY_THRESHOLD = 50
    /// Maximum connection attempts per minute in dense networks
    private let ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE = 6
    /// Minimum interval between connection attempts to the same peripheral
    private let ADAPTIVE_COOLDOWN_PER_PERIPHERAL: TimeInterval = 30.0
    /// Interval for updating visible peer count estimate
    private let ADAPTIVE_PEER_COUNT_WINDOW: TimeInterval = 5.0
    
    // MARK: - Properties
    
    // Thread-safe: OfflineProtocol uses Mutex/RwLock internally (see offline-protocol-uniffi)
    private let protocolInstance: OfflineProtocol
    private let deviceId: String
    private let meshController: MeshController
    
    // Central (scanner/client) components
    private var centralManager: CBCentralManager?
    private let connections = MeshConnectionRegistry()
    
    /// Public accessor for the Bluetooth state
    var bluetoothState: CBManagerState {
        return centralManager?.state ?? .unknown
    }
    private var discoveredPeripherals: [UUID: CBPeripheral] = [:]
    private var peripheralRSSI: [UUID: Int16] = [:]
    
    // Peripheral (advertiser/server) components
    private var peripheralManager: CBPeripheralManager?
    private var messageCharacteristic: CBMutableCharacteristic?
    private var deviceIdCharacteristic: CBMutableCharacteristic?
    private var identityCharacteristic: CBMutableCharacteristic?
    
    /// Cached signed identity and local address for serving via GATT.
    ///
    /// Both are produced by UniFFI calls that take the core protocol mutex, so
    /// they are computed on `fragmentQueue` and cached here rather than being
    /// fetched inline from `setupGattServer` — which runs on the main queue,
    /// where that mutex wait is an App Hang (OFF-2123).
    ///
    /// Caching the address is safe for the same reason the identity refresh is:
    /// `initialize_mls` is idempotent and refuses to run once the protocol has
    /// started, so an instance's address is fixed for its lifetime. See the
    /// long-form rationale on `updateSignedIdentity`.
    private let identityLock = NSLock()
    private var cachedSignedIdentity: SignedIdentityData?
    private var cachedLocalAddress: String?
    /// Guards against piling up refreshes while one is already in flight.
    private var identityRefreshInFlight = false
    /// Set when a refresh is requested while one is in flight, so the request
    /// is coalesced into one more pass instead of being dropped — see
    /// `updateSignedIdentity`.
    private var identityRefreshRequested = false

    private func currentSignedIdentity() -> SignedIdentityData? {
        identityLock.lock()
        defer { identityLock.unlock() }
        return cachedSignedIdentity
    }

    private func currentLocalAddress() -> String? {
        identityLock.lock()
        defer { identityLock.unlock() }
        return cachedLocalAddress
    }

    /// Verified peer identities (peripheral UUID -> SignedIdentityData)
    private var verifiedPeerIdentities: [UUID: SignedIdentityData] = [:]

    /// Half-finished handshakes: what a peripheral advertised in `DEVICE_ID`,
    /// before its `IDENTITY` has been read and verified.
    ///
    /// The two reads are issued together in `didDiscoverCharacteristicsFor`
    /// and complete in either order, so neither handler can act alone — each
    /// records its half and calls `completePeerHandshake`, which fires exactly
    /// once, when both are in. Nothing is announced from either half.
    private var advertisedDeviceIds: [UUID: String] = [:]

    /// Addresses derived from a peripheral's *verified* `IDENTITY` key — the
    /// other half of the join above. Present only when the signature checked
    /// out, so its presence is the proof, not the blob's arrival.
    private var verifiedPeerAddresses: [UUID: String] = [:]

    /// Peripherals already announced via `blePeerDiscovered`, so a re-read of
    /// either characteristic on a live link cannot announce twice.
    private var announcedPeripherals: Set<UUID> = []

    // Fragment sending (event-driven, no polling)
    private let fragmentQueue = DispatchQueue(label: "com.offlineprotocol.ble.fragments")
    
    // Pending fragments waiting for device ID.
    //
    // Thread-safety contract: MUTATIONS are owned by fragmentQueue and must
    // occur inside fragmentQueue.async — that serial queue is what preserves
    // the FIFO ordering #59 established. evictPeer() dispatches removals to
    // fragmentQueue to honour this contract, and the stores assert it in debug
    // builds via their `queueCheck`.
    //
    // READS are safe from any thread: both stores guard their state with an
    // NSLock held only across a dictionary operation, never across a UniFFI
    // call or a BLE write. This is the load-bearing half of the OFF-2123 fix —
    // main-queue readers (the connection monitor, the metrics refresher) used
    // to `fragmentQueue.sync` for these snapshots, which parked the main thread
    // behind whatever multi-second core-protocol lock wait that queue happened
    // to be inside.
    private lazy var inboundFragments = InboundFragmentBuffer(
        queueCheck: { [weak self] in self?.assertOnFragmentQueue() },
        maxPerPeer: MAX_PENDING_FRAGMENTS_PER_PEER,
        timeout: PENDING_FRAGMENT_TIMEOUT,
        onDropped: { [weak self] id, reason, count in
            self?.emitDiagnostic("warning", "Inbound BLE fragments dropped", context: [
                "central": id.uuidString,
                "reason": reason == .expired ? "expired" : "capped",
                "dropped": count
            ])
        }
    )
    // Idle window for incoming fragments waiting for the sender's device-id to
    // resolve (a GATT connect+read, throttled to ~5s and prone to retries). 5s was
    // too short: a first-contact multi-fragment MLS Welcome arriving in a burst
    // could be evicted before resolution completed and be lost before reassembly.
    // 15s exceeds the reverse-resolution worst case; the per-peer cap bounds memory.
    private let PENDING_FRAGMENT_TIMEOUT: TimeInterval = 15.0
    private let PENDING_OUTBOUND_FRAGMENT_TIMEOUT: TimeInterval = 30.0 // For outbound fragments that failed to send
    private let MAX_PENDING_FRAGMENTS_PER_PEER = 100
    // Outbound fragments that could not be sent immediately — same contract as
    // `inboundFragments` above.
    private lazy var outboundFragments = OutboundFragmentQueue(
        queueCheck: { [weak self] in self?.assertOnFragmentQueue() },
        maxPerPeer: MAX_PENDING_FRAGMENTS_PER_PEER,
        timeout: PENDING_OUTBOUND_FRAGMENT_TIMEOUT,
        onDropped: { [weak self] recipientId, reason, count in
            guard let self = self else { return }
            if reason == .expired {
                if self.logThrottler.shouldLog(key: "fragments_expired_\(recipientId)", interval: 10) {
                    print("[BleManager] ⚠️ Removed \(count) expired outbound fragments for \(recipientId)")
                    self.emitDiagnostic("warning", "Outbound fragments expired",
                                        context: ["recipientId": recipientId, "dropped": count])
                }
            } else {
                self.emitDiagnostic("warning", "Pending outbound fragment queue capped, dropping oldest",
                                    context: ["recipientId": recipientId, "dropped": count,
                                              "max": self.MAX_PENDING_FRAGMENTS_PER_PEER])
            }
        }
    )
    private struct MeshObservation {
        let advertisement: MeshAdvertisementData
        let rssi: Int?
        let timestamp: Date
    }
    private var lastSeenMeshAdvertisements: [UUID: MeshObservation] = [:]
    private var pendingAdvertiseRestart: DispatchWorkItem?
    private var lastAdvertiseRestartAt: Date?
    private var transportStartAt: Date?
    
    // State tracking
    private var isScanning = false
    private var isAdvertising = false
    private var centralReady = false
    private var peripheralReady = false
    private var isGattServiceReady = false
    private var pendingAdvertiseAfterServiceReady = false
    // Peripheral-role NOTIFY egress (the iOS mirror of Android's
    // PeripheralGattServer.notifyFragment path, PR #120/#121). Lets iOS reply to a
    // peer that connected to US as central — so we hold no `CBPeripheral` for it —
    // by pushing a notification over the link IT opened, instead of trying to
    // reverse-connect as central (which iOS backgrounding routinely blocks). Absent
    // this, iOS can only send over a link it opened, so iOS→Android stalls whenever
    // Android is central / iOS is peripheral.
    //
    // `subscribedCentralsById` is guarded by `notifyLock` so the fragment drain
    // (fragmentQueue) can test notify-reachability while CoreBluetooth's peripheral
    // delegates (main queue) mutate it. The `notifyOutbound` queue and the actual
    // `peripheralManager.updateValue` call stay main-queue-only — CBPeripheralManager
    // must be driven from its delegate queue (init'd with `queue: nil` ⇒ main) — so
    // the drain hands fragments to the main-queue pump via `enqueueNotifyOutbound`.
    private let notifyLock = NSLock()
    private var subscribedCentralsById: [UUID: CBCentral] = [:]
    /// Per-recipient NOTIFY outbound queue, drained by `pumpNotifyOutbound`. Main-queue only.
    private var notifyOutbound: [String: [(data: Data, timestamp: Date)]] = [:]
    private var lastMeshAdvertisement: MeshAdvertisementData?

    // Logging & monitoring
    private let logThrottler = LogThrottler()
    private var discoveryLogTimestamps: [UUID: Date] = [:]
    private var scanStateMonitor: DispatchSourceTimer?
    private var lastDiscoveryDate: Date?
    private var scanStartDate: Date?
    
    // Adaptive scan state
    /// Timestamps of recent peripheral discoveries for density estimation
    private var recentDiscoveryTimestamps: [Date] = []
    /// Last connection attempt timestamps per peripheral for rate limiting
    private var peripheralConnectionAttempts: [UUID: Date] = [:]
    /// Global connection attempts in the last minute for rate limiting
    private var globalConnectionAttempts: [Date] = []
    /// Current estimated visible peer count
    private var estimatedVisiblePeerCount: Int = 0
    /// Last time we updated the peer count estimate
    private var lastPeerCountUpdate: Date?
    private let SCAN_HEARTBEAT_INTERVAL: TimeInterval = 10.0
    private let SCAN_RESTART_INTERVAL: TimeInterval = 30.0
    /// Force a complete BLE stack refresh periodically even when things seem healthy
    private let FORCED_BLE_REFRESH_INTERVAL: TimeInterval = 120.0
    private var lastForcedBleRefresh: Date?
    private var connectionMonitor: DispatchSourceTimer?
    private var connectionAttemptTimestamps: [UUID: Date] = [:]
    private var connectionRetryCount: [UUID: Int] = [:]
    private let CONNECTION_MONITOR_INTERVAL: TimeInterval = 5.0
    private let MIN_RECONNECT_INTERVAL: TimeInterval = 5.0
    private let MAX_RECONNECT_INTERVAL: TimeInterval = 60.0
    private let MAX_CONNECTION_RETRIES = 5
    private var scanRestartCount: Int = 0
    private var lastCentralReset: Date?
    private let MAX_CONSECUTIVE_SCAN_RESTARTS = 3
    private let CENTRAL_RESET_BACKOFF: TimeInterval = 45.0
    private let MINIMUM_RSSI_TO_CONNECT: Int16 = -90
    /// Cooldown between provisional bootstrap attempts for unknown peripherals
    private let UNKNOWN_BOOTSTRAP_RATE_LIMIT: TimeInterval = 12.0
    /// Minimum RSSI for provisional bootstrap when advertisement keys are present
    private let UNKNOWN_BOOTSTRAP_MIN_RSSI: Int16 = -75
    /// Stricter RSSI threshold when expected advertisement keys are missing
    private let UNKNOWN_BOOTSTRAP_MIN_RSSI_WITH_MISSING_KEYS: Int16 = -68
    /// Max provisional unknown bootstrap attempts per minute
    private let MAX_UNKNOWN_BOOTSTRAP_ATTEMPTS_PER_MINUTE = 4
    /// Proactive scan refresh interval even when discoveries are occurring
    private let PROACTIVE_SCAN_REFRESH_INTERVAL: TimeInterval = 60.0
    private var lastProactiveScanRefresh: Date?
    /// Tracks recently seen advertisement hashes to avoid duplicate processing
    private var recentAdvertisementHashes: [UUID: (hash: Int, timestamp: Date)] = [:]
    /// Initial aggressive discovery phase duration - more frequent scanning initially
    private let AGGRESSIVE_DISCOVERY_PHASE: TimeInterval = 30.0
    /// Tracks when aggressive discovery phase started
    private var aggressiveDiscoveryStarted: Date?
    /// Negative cache: devices verified via GATT as non-mesh (identifier -> timestamp)
    private var verifiedNonMeshDevices: [UUID: Date] = [:]
    /// Rate limiter for provisional unknown bootstrap attempts
    private var unknownBootstrapAttempts: [UUID: Date] = [:]
    private let NON_MESH_CACHE_TTL: TimeInterval = 300.0 // 5 minutes

    // MARK: - Thread helpers
    @inline(__always)
    private func performOnMain<T>(_ work: () throws -> T) rethrows -> T {
        if Thread.isMainThread {
            return try work()
        }
        return try DispatchQueue.main.sync(execute: work)
    }

    /// Asserts the caller owns `fragmentQueue`, the mutation contract for the
    /// two fragment stores.
    ///
    /// DEBUG-only on purpose. `dispatchPrecondition` traps, and this fix exists
    /// to remove hangs — shipping a brand-new crash vector to production to
    /// police an internal invariant trades one incident class for a worse one.
    /// Android's mirror (`mainThreadCheck`) fails fast in release because a
    /// Kotlin `check()` throws where a trap does not.
    @inline(__always)
    private func assertOnFragmentQueue() {
        #if DEBUG
        dispatchPrecondition(condition: .onQueue(fragmentQueue))
        #endif
    }

    /// Runs `work` off the main thread, on the queue that owns this transport's
    /// protocol calls.
    ///
    /// EVERY call into `protocolInstance` must go through here or already be
    /// running on `fragmentQueue`. The one exception is `verifySignature`,
    /// which takes no lock in the core (it is a static Ed25519 check).
    ///
    /// The reason is the whole of OFF-2123: the UniFFI layer serialises the
    /// entire protocol behind one `Mutex<CoreProtocol>`, and the BLE inbound
    /// path holds it across MLS decrypt, secure-storage callbacks (Keychain,
    /// on whichever thread called in) and ACK sends. A CoreBluetooth delegate
    /// is a main-queue callback — `CBCentralManager`/`CBPeripheralManager` are
    /// both initialised with `queue: nil` — so any FFI call made directly from
    /// one parks the main thread on that mutex for as long as the holder needs.
    ///
    /// Reusing `fragmentQueue` rather than adding a second queue is deliberate:
    /// one serial queue gives these calls a total order, so a status change
    /// cannot overtake the fragment traffic it relates to.
    @inline(__always)
    private func onProtocolQueue(_ work: @escaping () -> Void) {
        fragmentQueue.async {
            #if DEBUG
            dispatchPrecondition(condition: .notOnQueue(.main))
            #endif
            work()
        }
    }

    // MARK: - Diagnostics
    private static let diagnosticDateFormatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()
    
    private func sanitizeDiagnosticValue(_ value: Any) -> Any {
        switch value {
        case let dict as [String: Any]:
            var sanitized: [String: Any] = [:]
            for (key, nested) in dict {
                sanitized[key] = sanitizeDiagnosticValue(nested)
            }
            return sanitized
        case let dict as [AnyHashable: Any]:
            var sanitized: [String: Any] = [:]
            for (key, nested) in dict {
                sanitized[String(describing: key)] = sanitizeDiagnosticValue(nested)
            }
            return sanitized
        case let dict as NSDictionary:
            var sanitized: [String: Any] = [:]
            dict.forEach { key, nested in
                sanitized[String(describing: key)] = sanitizeDiagnosticValue(nested)
            }
            return sanitized
        case let array as [Any]:
            return array.map { sanitizeDiagnosticValue($0) }
        case let array as NSArray:
            return array.map { sanitizeDiagnosticValue($0) }
        case let number as NSNumber:
            if CFNumberIsFloatType(number) {
                let doubleValue = number.doubleValue
                if !doubleValue.isFinite {
                    return String(describing: doubleValue)
                }
            }
            return number
        case let double as Double:
            return double.isFinite ? double : String(describing: double)
        case let float as Float:
            return float.isFinite ? float : String(describing: float)
        case let int as Int:
            return int
        case let int32 as Int32:
            return int32
        case let int64 as Int64:
            return int64
        case let uint as UInt:
            return uint
        case let string as String:
            return string
        case let bool as Bool:
            return bool
        case let uuid as UUID:
            return uuid.uuidString
        case let cbUuid as CBUUID:
            return cbUuid.uuidString
        case let date as Date:
            return BleManager.diagnosticDateFormatter.string(from: date)
        case let data as Data:
            return data.base64EncodedString()
        case let error as NSError:
            return [
                "domain": error.domain,
                "code": error.code,
                "userInfo": sanitizeDiagnosticValue(error.userInfo)
            ]
        case is NSNull:
            return NSNull()
        default:
            return String(describing: value)
        }
    }
    
    private func emitDiagnostic(_ level: String, _ message: String, context: [String: Any] = [:]) {
        let sanitizedContext = sanitizeDiagnosticValue(context) as? [String: Any] ?? [:]
        delegate?.transportManager(self, didEmitDiagnostic: level, message: message, context: sanitizedContext)
    }
    
    // MARK: - Initialization
    
    public init(protocol protocolInstance: OfflineProtocol, deviceId: String) {
        self.protocolInstance = protocolInstance
        self.deviceId = deviceId
        self.meshController = MeshController(selfId: deviceId)
        super.init()
        meshController.markPeerActive(deviceId)
        refreshSelfMetrics()
    }
    
    deinit {
        stop()
    }
    
    // MARK: - TransportManager Implementation
    
    public func isAvailable() -> Bool {
        // BLE is available on all iOS devices (iPhone 4S+, iPad 3+)
        return true
    }
    
    public func start() throws {
        try performOnMain {
            try self.startUnsafe()
        }
    }
    
    private func startUnsafe() throws {
        guard state != .running else {
            throw TransportError.alreadyRunning
        }
        
        guard isAvailable() else {
            throw TransportError.notAvailable("BLE not available on this device")
        }
        
        // Check authorization status on iOS 13.1+
        if #available(iOS 13.1, *) {
            let centralAuth = CBCentralManager.authorization
            let peripheralAuth = CBPeripheralManager.authorization
            
            print("[BleManager] 🔐 Checking Bluetooth permissions:")
            print("[BleManager]   Central authorization: \(centralAuth.rawValue)")
            print("[BleManager]   Peripheral authorization: \(peripheralAuth.rawValue)")
            
            emitDiagnostic("info", "Checking Bluetooth permissions", context: [
                "centralAuth": centralAuth.rawValue,
                "peripheralAuth": peripheralAuth.rawValue
            ])
            
            // If already denied, inform the user immediately
            if centralAuth == .denied || peripheralAuth == .denied {
                let msg = "Bluetooth permission was denied. Please enable Bluetooth access in Settings > \(Bundle.main.displayName ?? "App") > Bluetooth"
                print("[BleManager] ❌ \(msg)")
                emitDiagnostic("error", msg, context: [
                    "centralAuth": centralAuth.rawValue,
                    "peripheralAuth": peripheralAuth.rawValue
                ])
            } else if centralAuth == .restricted || peripheralAuth == .restricted {
                let msg = "Bluetooth permission is restricted by device management or parental controls"
                print("[BleManager] ⚠️ \(msg)")
                emitDiagnostic("error", msg, context: [
                    "centralAuth": centralAuth.rawValue,
                    "peripheralAuth": peripheralAuth.rawValue
                ])
            } else if centralAuth == .notDetermined || peripheralAuth == .notDetermined {
                print("[BleManager] 🔔 Bluetooth permission will be requested")
                emitDiagnostic("info", "Bluetooth permission will be requested")
            } else {
                print("[BleManager] ✅ Bluetooth permissions already granted")
                emitDiagnostic("info", "Bluetooth permissions already granted")
            }
        }
        
        print("[BleManager] 🚀 Starting BLE transport for device: \(deviceId)")
        emitDiagnostic("info", "Starting BLE transport", context: ["deviceId": deviceId])
        updateState(.starting)
        transportStartAt = Date()
        
        // Initialize Central Manager (for scanning) with state restoration support.
        // The restore identifier allows iOS to relaunch the app and restore BLE
        // connections after the app has been terminated by the OS.
        print("[BleManager] 📱 Initializing Central Manager...")
        centralManager = CBCentralManager(
            delegate: self,
            queue: nil,
            options: [
                CBCentralManagerOptionShowPowerAlertKey: true,
                CBCentralManagerOptionRestoreIdentifierKey: "com.offlineprotocol.central"
            ]
        )
        
        // Initialize Peripheral Manager (for advertising) with state restoration.
        print("[BleManager] 📡 Initializing Peripheral Manager...")
        peripheralManager = CBPeripheralManager(
            delegate: self,
            queue: nil,
            options: [
                CBPeripheralManagerOptionShowPowerAlertKey: true,
                CBPeripheralManagerOptionRestoreIdentifierKey: "com.offlineprotocol.peripheral"
            ]
        )
        
        print("[BleManager] ⏳ Waiting for Bluetooth to power on and permissions to be granted...")
        emitDiagnostic("info", "Waiting for Bluetooth to power on and permissions")
        // Note: Actual start happens in delegate callbacks when ready
    }
    
    public func stop() {
        performOnMain {
            self.stopUnsafe()
        }
    }
    
    private func stopUnsafe() {
        guard state == .running || state == .starting else {
            return
        }
        
        updateState(.stopping)
        
        // Stop scanning
        stopScanning(reason: "stop")
        
        // Stop advertising
        stopAdvertising()
        
        // Disconnect all peripherals
        for peripheral in connections.allPeripherals() {
            centralManager?.cancelPeripheralConnection(peripheral)
        }
        connections.reset()
        discoveredPeripherals.removeAll()
        peripheralRSSI.removeAll()
        inboundFragments.clear()
        outboundFragments.clear()
        // Then chase the clear on the owning queue. The clears above are
        // immediate (nothing here waits on `fragmentQueue`), but a fragment
        // block dispatched just before this point still runs AFTER them and
        // would repopulate a store that nothing sweeps once we are stopped.
        // `fragmentQueue` is serial and FIFO, so this drops exactly the
        // in-flight backlog and nothing enqueued later.
        //
        // Binds the stores to locals instead of capturing `self`: `stop()` is
        // reachable from `deinit`, and a `[weak self]` capture list is
        // evaluated when the closure is CREATED — forming a weak reference to
        // a deallocating object is a hard abort (see BRIDGE_MAINTENANCE.md).
        let inbound = inboundFragments
        let outbound = outboundFragments
        fragmentQueue.async {
            inbound.clear()
            outbound.clear()
        }
        lastSeenMeshAdvertisements.removeAll()
        unknownBootstrapAttempts.removeAll()
        verifiedNonMeshDevices.removeAll()
        recentAdvertisementHashes.removeAll()
        pendingAdvertiseRestart?.cancel()
        pendingAdvertiseRestart = nil
        lastAdvertiseRestartAt = nil
        transportStartAt = nil
        lastProactiveScanRefresh = nil
        lastForcedBleRefresh = nil
        aggressiveDiscoveryStarted = nil
        notifyLock.lock()
        subscribedCentralsById.removeAll()
        notifyLock.unlock()
        notifyOutbound.removeAll()

        // Clean up managers
        centralManager = nil
        peripheralManager = nil
        
        centralReady = false
        peripheralReady = false
        isGattServiceReady = false
        pendingAdvertiseAfterServiceReady = false
        
        updateState(.stopped)
        emitDiagnostic("info", "BLE transport stopped")
    }
    
    public func pause() {
        performOnMain {
            self.pauseUnsafe()
        }
    }
    
    private func pauseUnsafe() {
        // For iOS background mode — stop scanning but keep connections alive.
        // Fragment sending remains event-driven via the Rust callback and
        // CoreBluetooth delegate methods, which iOS delivers even in background.
        stopScanning(reason: "pause")
    }
    
    public func resume() {
        performOnMain {
            self.resumeUnsafe()
        }
    }
    
    private func resumeUnsafe() {
        // Resume from background — restart scanning.
        // Fragment sending is event-driven and does not need restart.
        if state == .running {
            startScanning(reason: "resume")
            // Drain any fragments that accumulated while backgrounded
            drainAndSendFragments()
        }
    }
    
    // MARK: - Private Methods
    
    private func updateState(_ newState: TransportState) {
        state = newState
        delegate?.transportManager(self, didChangeState: newState)
    }
    
    private func startScanning(reason: String = "manual") {
        guard let central = centralManager else {
            if logThrottler.shouldLog(key: "scan_missing_central") {
                print("[BleManager] Cannot start scanning – central manager not initialized")
            }
            return
        }
        
        guard central.state == .poweredOn else {
            if logThrottler.shouldLog(key: "scan_not_powered") {
                print("[BleManager] Skipping scan start – central state: \(central.state.rawValue)")
                emitDiagnostic("info", "Scan start skipped", context: ["state": central.state.rawValue, "reason": reason])
            }
            return
        }
        
        guard !isScanning else {
            if logThrottler.shouldLog(key: "scan_already_running") {
                print("[BleManager] Scan already running (reason: \(reason))")
            }
            return
        }

        if reason != "watchdog" {
            scanRestartCount = 0
        }
        
        // Scan without service UUID filter for iOS ↔ Android interoperability
        // iOS's scanForPeripherals(withServices:) has known issues recognizing 128-bit
        // service UUIDs from Android advertisements. Scanning with nil allows us to see
        // all peripherals and filter in the discovery callback instead.
        central.scanForPeripherals(
            withServices: nil,
            options: [CBCentralManagerScanOptionAllowDuplicatesKey: true]
        )
        isScanning = true
        let now = Date()
        scanStartDate = now
        lastDiscoveryDate = scanStartDate
        lastProactiveScanRefresh = now
        // Start aggressive discovery phase for initial faster connection
        if aggressiveDiscoveryStarted == nil {
            aggressiveDiscoveryStarted = now
            print("[BleManager] Starting aggressive discovery phase (\(AGGRESSIVE_DISCOVERY_PHASE)s)")
            emitDiagnostic("info", "Starting aggressive discovery phase", context: [
                "duration": AGGRESSIVE_DISCOVERY_PHASE
            ])
        }
        startScanMonitor()
        if logThrottler.shouldLog(key: "scan_started") {
            let context: [String: Any] = [
                "reason": reason,
                "allowDuplicates": true
            ]
            print("[BleManager] Started scanning (reason: \(reason))")
            emitDiagnostic("info", "Started BLE scanning", context: context)
        }
        startConnectionMonitor()

        // Rehydrate previously connected peripherals to avoid waiting for advertisements
        let retainedPeripherals = central.retrieveConnectedPeripherals(withServices: [SERVICE_UUID])
        for peripheral in retainedPeripherals {
            discoveredPeripherals[peripheral.identifier] = peripheral
            attemptConnection(to: peripheral, reason: "retrieve_connected")
        }
    }
    
    private func stopScanning(reason: String = "manual") {
        guard isScanning else { return }
        centralManager?.stopScan()
        isScanning = false
        stopScanMonitor()
        stopConnectionMonitor()
        scanStartDate = nil
        lastDiscoveryDate = nil
        connectionAttemptTimestamps.removeAll()
        connectionRetryCount.removeAll()
        if logThrottler.shouldLog(key: "scan_stopped") {
            print("[BleManager] Stopped scanning (reason: \(reason))")
        }
        emitDiagnostic("info", "Stopped BLE scanning", context: ["reason": reason])
    }
    
    private func startScanMonitor() {
        guard scanStateMonitor == nil else { return }
        let timer = DispatchSource.makeTimerSource(queue: DispatchQueue.main)
        timer.schedule(deadline: .now() + SCAN_HEARTBEAT_INTERVAL, repeating: SCAN_HEARTBEAT_INTERVAL)
        timer.setEventHandler { [weak self] in
            guard let self = self else { return }
            guard self.isScanning else { return }
            let now = Date()
            let lastActivity = self.lastDiscoveryDate ?? self.scanStartDate ?? now
            let idleDuration = now.timeIntervalSince(lastActivity)
            
            // Check for inactivity-based restart
            if idleDuration >= self.SCAN_RESTART_INTERVAL {
                if self.logThrottler.shouldLog(key: "scan_watchdog", interval: self.SCAN_RESTART_INTERVAL) {
                    print("[BleManager] Restarting scan after \(Int(idleDuration))s of inactivity")
                    self.emitDiagnostic("warning", "Restarting BLE scan due to inactivity", context: ["idle_seconds": Int(idleDuration)])
                }
                self.restartScanningDueToInactivity()
                return
            }
            
            // Proactive scan refresh even when discoveries are occurring
            // This ensures we don't miss devices due to BLE stack issues
            let lastRefresh = self.lastProactiveScanRefresh ?? self.scanStartDate ?? now
            if now.timeIntervalSince(lastRefresh) >= self.PROACTIVE_SCAN_REFRESH_INTERVAL {
                if self.logThrottler.shouldLog(key: "proactive_scan_refresh", interval: self.PROACTIVE_SCAN_REFRESH_INTERVAL) {
                    print("[BleManager] Proactively refreshing BLE scan")
                    self.emitDiagnostic("info", "Proactive scan refresh")
                }
                self.lastProactiveScanRefresh = now
                self.restartScanningDueToInactivity()
            }
            
            // Forced complete BLE refresh - more aggressive than proactive refresh
            // This helps recover from edge cases where the BLE stack becomes stuck
            let lastForced = self.lastForcedBleRefresh ?? self.transportStartAt ?? now
            if now.timeIntervalSince(lastForced) >= self.FORCED_BLE_REFRESH_INTERVAL {
                self.lastForcedBleRefresh = now
                if self.logThrottler.shouldLog(key: "forced_ble_refresh", interval: self.FORCED_BLE_REFRESH_INTERVAL) {
                    print("[BleManager] Performing forced BLE refresh for reliability")
                    self.emitDiagnostic("info", "Forced BLE refresh for reliability", context: [
                        "connectedPeers": self.connections.connectedPeripheralCount(),
                        "discoveredPeers": self.discoveredPeripherals.count
                    ])
                }
                // Stop and restart both scanning and advertising
                self.stopScanning(reason: "forced_refresh")
                self.refreshAdvertising(reason: "forced_refresh")
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                    self?.startScanning(reason: "forced_refresh")
                }
            }
            
            // After aggressive phase ends, do a targeted scan with service UUID filter
            // This can help discover Android devices that might have been missed
            if let started = self.aggressiveDiscoveryStarted,
               now.timeIntervalSince(started) >= self.AGGRESSIVE_DISCOVERY_PHASE,
               now.timeIntervalSince(started) < self.AGGRESSIVE_DISCOVERY_PHASE + self.SCAN_HEARTBEAT_INTERVAL {
                print("[BleManager] Aggressive phase ended, performing targeted service scan")
                self.emitDiagnostic("info", "Aggressive phase ended, performing targeted service scan", context: [
                    "discoveredPeers": self.discoveredPeripherals.count,
                    "connectedPeers": self.connections.connectedPeripheralCount()
                ])
                // Brief targeted scan with service UUID
                self.performTargetedServiceScan()
            }
        }
        timer.resume()
        scanStateMonitor = timer
    }
    
    private func stopScanMonitor() {
        scanStateMonitor?.cancel()
        scanStateMonitor = nil
    }
    
    private func restartScanningDueToInactivity() {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            // Restart scan only if still marked as scanning
            guard self.isScanning else { return }
            self.centralManager?.stopScan()
            self.isScanning = false
            self.scanRestartCount += 1
            self.startScanning(reason: "watchdog")
            self.evaluateCentralHealthAfterRestart()
        }
    }
    
    /// Performs a brief targeted scan with service UUID filter.
    /// This helps discover Android devices that might not advertise our service UUID
    /// in the main advertisement packet but include it in scan response.
    private func performTargetedServiceScan() {
        guard let central = centralManager, central.state == .poweredOn else { return }
        guard isScanning else { return }
        
        // Brief stop and restart with service filter
        central.stopScan()
        
        // Scan with service UUID filter for 5 seconds
        central.scanForPeripherals(
            withServices: [SERVICE_UUID],
            options: [CBCentralManagerScanOptionAllowDuplicatesKey: true]
        )
        
        // After 5 seconds, go back to filterless scanning
        DispatchQueue.main.asyncAfter(deadline: .now() + 5.0) { [weak self] in
            guard let self = self, self.isScanning else { return }
            self.centralManager?.stopScan()
            self.centralManager?.scanForPeripherals(
                withServices: nil,
                options: [CBCentralManagerScanOptionAllowDuplicatesKey: true]
            )
        }
    }
    
    private func evaluateCentralHealthAfterRestart() {
        guard scanRestartCount >= MAX_CONSECUTIVE_SCAN_RESTARTS else { return }
        let now = Date()
        if let lastReset = lastCentralReset, now.timeIntervalSince(lastReset) < CENTRAL_RESET_BACKOFF {
            return
        }
        emitDiagnostic("warning", "Resetting BLE central due to repeated scan stalls", context: [
            "restartCount": scanRestartCount
        ])
        centralReady = false
        centralManager?.stopScan()
        centralManager = CBCentralManager(
            delegate: self,
            queue: nil,
            options: [
                CBCentralManagerOptionShowPowerAlertKey: true,
                CBCentralManagerOptionRestoreIdentifierKey: "com.offlineprotocol.central"
            ]
        )
        lastCentralReset = now
        scanRestartCount = 0
    }
    
    private func markDiscoveryEvent() {
        lastDiscoveryDate = Date()
    }
    
    private func startConnectionMonitor() {
        guard connectionMonitor == nil else { return }
        let timer = DispatchSource.makeTimerSource(queue: DispatchQueue.main)
        timer.schedule(deadline: .now() + CONNECTION_MONITOR_INTERVAL, repeating: CONNECTION_MONITOR_INTERVAL)
        timer.setEventHandler { [weak self] in
            guard let self = self else { return }
            let now = Date()
            for peripheral in self.discoveredPeripherals.values {
                if self.connections.connectedPeripheral(for: peripheral.identifier) == nil {
                    self.attemptConnection(to: peripheral, reason: "monitor")
                }
            }
            let pendingKeys: [UUID] = self.inboundFragments.pendingIds()
            for centralId in pendingKeys {
                if self.connections.centralDeviceId(for: centralId) == nil && self.connections.peripheralDeviceId(for: centralId) == nil {
                    // Ensure we periodically try to resolve device IDs for pending fragments
                    if let last = self.connectionAttemptTimestamps[centralId], now.timeIntervalSince(last) < self.MIN_RECONNECT_INTERVAL {
                        continue
                    }
                    self.connectionAttemptTimestamps[centralId] = now
                    self.ensureDeviceId(for: centralId)
                }
            }
        }
        timer.resume()
        connectionMonitor = timer
    }
    
    private func stopConnectionMonitor() {
        connectionMonitor?.cancel()
        connectionMonitor = nil
    }
    
    private func attemptConnection(to peripheral: CBPeripheral, reason: String, rssi: Int16? = nil, desiredRole: MeshController.MeshRole? = nil) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.performConnectionAttempt(to: peripheral, reason: reason, rssi: rssi, desiredRole: desiredRole)
        }
    }

    /// Core connection logic — must be called on the main thread.
    private func performConnectionAttempt(to peripheral: CBPeripheral, reason: String, rssi: Int16? = nil, desiredRole: MeshController.MeshRole? = nil) {
        // Atomic check-and-connect: all checks must pass before proceeding
        // This prevents race conditions when multiple discovery callbacks try to connect

        // 1. Already connected check
        if connections.connectedPeripheral(for: peripheral.identifier) != nil {
            return
        }

        // 2. Recent attempt cooldown check
        let now = Date()
        if let lastAttempt = connectionAttemptTimestamps[peripheral.identifier], now.timeIntervalSince(lastAttempt) < MIN_RECONNECT_INTERVAL {
            return
        }

        // 3. RSSI threshold check
        if let effectiveRSSI = rssi ?? peripheralRSSI[peripheral.identifier], effectiveRSSI < MINIMUM_RSSI_TO_CONNECT {
            if logThrottler.shouldLog(key: "rssi_skip_\(peripheral.identifier.uuidString)", interval: 10) {
                emitDiagnostic("debug", "Skipping BLE connect due to weak RSSI", context: [
                    "rssi": effectiveRSSI,
                    "threshold": MINIMUM_RSSI_TO_CONNECT,
                    "reason": reason
                ])
            }
            return
        }

        // 4. Connection capacity check (atomic with the connect call)
        if currentConnectionCount() >= MAX_CONNECTIONS_PER_DEVICE {
            if logThrottler.shouldLog(key: "mesh_conn_cap_ios", interval: 10) {
                print("[BleManager] Connection cap reached, not connecting to \(peripheral.identifier)")
            }
            return
        }

        // 5. Double-check peripheral state before connecting
        guard peripheral.state != .connecting else {
            if logThrottler.shouldLog(key: "already_connecting_\(peripheral.identifier.uuidString)", interval: 5) {
                print("[BleManager] Already connecting to \(peripheral.identifier)")
            }
            return
        }

        // All checks passed - proceed with connection
        connectionAttemptTimestamps[peripheral.identifier] = now
        if let desiredRole = desiredRole {
            connections.setPendingRole(desiredRole, for: peripheral.identifier)
        } else if connections.pendingRole(for: peripheral.identifier) == nil {
            connections.setPendingRole(.member, for: peripheral.identifier)
        }
        peripheral.delegate = self

        if peripheral.state == .connected {
            connections.registerPeripheral(peripheral)
            if let service = peripheral.services?.first(where: { $0.uuid == SERVICE_UUID }) {
                peripheral.discoverCharacteristics([MESSAGE_CHAR_UUID, DEVICE_ID_CHAR_UUID, IDENTITY_CHAR_UUID], for: service)
            } else {
                peripheral.discoverServices([SERVICE_UUID])
            }
            return
        }

        centralManager?.connect(peripheral, options: nil)
        if logThrottler.shouldLog(key: "connect_attempt_\(peripheral.identifier.uuidString)", interval: 10) {
            print("[BleManager] Attempting connection to \(peripheral.identifier) (reason: \(reason))")
            var context: [String: Any] = [
                "identifier": peripheral.identifier.uuidString,
                "reason": reason
            ]
            if let rssi = rssi {
                context["rssi"] = rssi
            }
            emitDiagnostic("info", "Connecting to BLE peripheral", context: context)
        }
    }

    private func ensureDeviceId(for centralId: UUID) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            // Check again with latest state
            if self.connections.centralDeviceId(for: centralId) != nil || self.connections.peripheralDeviceId(for: centralId) != nil {
                return
            }
            guard let centralManager = self.centralManager else { return }
            
            //  Aggressively try to find and connect to the central to read device ID
            // This is essential for Android → iOS message delivery when iOS doesn't know Android's device ID yet
            var candidates = centralManager.retrievePeripherals(withIdentifiers: [centralId])
            if candidates.isEmpty {
                let connected = centralManager.retrieveConnectedPeripherals(withServices: [self.SERVICE_UUID])
                candidates = connected.filter { $0.identifier == centralId }
            }
            
            // If still not found, try to find it in discovered peripherals
            if candidates.isEmpty, let peripheral = self.discoveredPeripherals[centralId] {
                candidates = [peripheral]
            }
            
            guard let peripheral = candidates.first else {
                // If we can't find the peripheral, try scanning for it
                // This is imp for Android → iOS: iOS needs to connect to Android as Central to read device ID
                if self.logThrottler.shouldLog(key: "missing_peripheral_\(centralId.uuidString)", interval: 15) {
                    print("[BleManager] ⚠️ Unable to retrieve peripheral for central \(centralId) - will try scanning")
                    self.emitDiagnostic("warning", "Unable to retrieve peripheral for central - scanning", context: [
                        "central": centralId.uuidString,
                        "reason": "Need to read device ID from Android device"
                    ])
                }
                // Ensure scanning is active so we can discover the Android device
                if !self.isScanning {
                    self.startScanning(reason: "resolve_device_id")
                }
                return
            }
            
            self.discoveredPeripherals[peripheral.identifier] = peripheral
            // Aggressively attempt connection to read device ID
            // This is imp for processing Android → iOS messages
            self.attemptConnection(to: peripheral, reason: "ensure_device_id_android_write")
        }
    }
    
    private func startAdvertising(reason: String = "manual") {
        guard let peripheral = peripheralManager, peripheral.state == .poweredOn else {
            return
        }
        if isAdvertising {
            if logThrottler.shouldLog(key: "advert_already_running", interval: 10) {
                print("[BleManager] Advertising already running (reason: \(reason))")
            }
            return
        }
        
        let servicePublishable = setupGattServer()

        // Wait for GATT service to be ready before advertising
        guard isGattServiceReady else {
            pendingAdvertiseAfterServiceReady = true
            if logThrottler.shouldLog(key: "advert_waiting_gatt", interval: 5) {
                print("[BleManager] Waiting for GATT service to be ready before advertising (reason: \(reason))")
                emitDiagnostic("info", "Waiting for GATT service registration", context: ["reason": reason])
            }
            // `pendingAdvertiseAfterServiceReady` is armed for the `didAdd`
            // callback, which only fires if a service was actually submitted.
            // When `setupGattServer` declined to publish — no local address
            // yet — nothing will ever call back, so schedule the retry that
            // re-enters this path. `scheduleAdvertisingRestart` carries the
            // interval floor and jitter, so an instance that never initializes
            // MLS re-checks at a bounded rate rather than spinning.
            if !servicePublishable {
                scheduleAdvertisingRestart(reason: "awaiting_local_address")
            }
            return
        }
        
        let meshData = meshController.advertisement()
        lastMeshAdvertisement = meshData
        var advertisementData: [String: Any] = [
            CBAdvertisementDataServiceUUIDsKey: [SERVICE_UUID]
        ]
        
        // Note: iOS has strict limitations on advertisement data:
        // - Service data (CBAdvertisementDataServiceDataKey) is not allowed when advertising as a peripheral
        // - Only service UUIDs are reliably advertised
        // - Mesh metadata must be exchanged after connection via GATT characteristics
        // Attempting to include service data causes a crash when CoreBluetooth internally
        // tries to serialize the CBUUID dictionary key
        
        // iOS limitation: We cannot advertise service data, only service UUIDs
        // The mesh advertisement data will need to be read via GATT characteristic after connection
        if logThrottler.shouldLog(key: "advert_no_service_data_ios", interval: 60) {
            print("[BleManager] iOS does not support service data in peripheral advertisements, advertising UUID only")
        }
        
        peripheral.startAdvertising(advertisementData)
        isAdvertising = true
        lastAdvertiseRestartAt = Date()
        emitDiagnostic("info", "Started BLE advertising", context: ["reason": reason])
    }
    
    private func stopAdvertising() {
        guard isAdvertising else { return }
        pendingAdvertiseRestart?.cancel()
        pendingAdvertiseRestart = nil
        peripheralManager?.stopAdvertising()
        isAdvertising = false
        print("[BleManager] Stopped advertising")
        emitDiagnostic("info", "Stopped BLE advertising")
    }

    private func refreshAdvertising(reason: String) {
        guard peripheralManager?.state == .poweredOn else { return }
        stopAdvertising()
        // Update the signed identity to match the new advertisement data
        updateSignedIdentity()
        scheduleAdvertisingRestart(reason: reason)
    }

    private func scheduleAdvertisingRestart(reason: String) {
        pendingAdvertiseRestart?.cancel()
        let now = Date()
        let elapsed = now.timeIntervalSince(lastAdvertiseRestartAt ?? .distantPast)
        let cooldown = max(0, MIN_ADVERTISE_INTERVAL - elapsed)
        let jitter = Double.random(in: ADVERTISE_RESTART_MIN...ADVERTISE_RESTART_MAX)
        let work = DispatchWorkItem { [weak self] in
            guard let self = self else { return }
            self.pendingAdvertiseRestart = nil
            self.startAdvertising(reason: reason)
        }
        pendingAdvertiseRestart = work
        DispatchQueue.main.asyncAfter(deadline: .now() + cooldown + jitter, execute: work)
    }
    
    /// Publishes the GATT service, and reports whether publication is possible
    /// at all.
    ///
    /// Returns `false` only for the one recoverable refusal — no local address
    /// yet — so the caller knows no `didAdd` callback is coming and must
    /// reschedule itself. `true` means the service is published, already
    /// published, or awaiting its `didAdd`. Callers must not infer this from
    /// whether a characteristic is nil: the characteristics outlive a
    /// `stop()`/`start()` cycle, so their nil-ness answers "was one ever
    /// built", not "did this call publish".
    ///
    /// Deliberately NOT `@discardableResult`: ignoring a `false` here is the
    /// bug this return value exists to prevent — advertising then waits on a
    /// `didAdd` that never comes.
    private func setupGattServer() -> Bool {
        guard let peripheral = peripheralManager else { return true }
        if messageCharacteristic != nil && deviceIdCharacteristic != nil && identityCharacteristic != nil && isGattServiceReady {
            return true
        }

        // What this device advertises as its identity is its derived address,
        // never the app-chosen `deviceId` (the profile). A profile is not an
        // identity: it is a local storage selector, commonly a shared constant
        // like "default", and nothing binds it to any key. Serving it here is
        // what let a peer claim any name it liked, and — since the core stamps
        // `localAddress()` as `Message.sender` — it also made every control
        // frame we sent fail the receiver's `validate_transport_sender`, which
        // compares that sender against the id the transport resolved for us.
        //
        // No address means no advertisement. Publishing the service without
        // one would announce a device that no peer can bind to a key, and the
        // cross-check on the central side would refuse it anyway; failing
        // closed here keeps the two ends agreeing about why. `startAdvertising`
        // reschedules, so this self-heals as soon as `initialize_mls` runs.
        // Read from the cache rather than calling into the core: this runs on
        // the main queue, and `localAddress()` takes the core protocol mutex.
        // A miss means the identity refresh has not completed yet, which is the
        // same "not ready" state the address guard already handles — kick the
        // refresh and let the existing reschedule re-enter.
        guard let advertisedAddress = currentLocalAddress(),
              !advertisedAddress.isEmpty else {
            updateSignedIdentity()
            if logThrottler.shouldLog(key: "gatt_no_local_address", interval: 10) {
                print("[BleManager] No local address yet; deferring GATT service registration")
                emitDiagnostic("info", "Deferring GATT registration until the local address exists", context: [
                    "reason": "mls_not_initialized"
                ])
            }
            return false
        }

        // The identity must exist BEFORE `peripheral.add(service)` — see the
        // caching contract below. Publishing without it permanently serves a
        // nil identity, so defer exactly as the address guard does rather than
        // blocking the main thread to compute one.
        guard let signedIdentity = currentSignedIdentity() else {
            updateSignedIdentity()
            if logThrottler.shouldLog(key: "gatt_no_signed_identity", interval: 10) {
                print("[BleManager] No signed identity yet; deferring GATT service registration")
                emitDiagnostic("info", "Deferring GATT registration until the signed identity exists", context: [
                    "reason": "identity_not_ready"
                ])
            }
            return false
        }

        // Reset flag - service registration is asynchronous
        isGattServiceReady = false

        // Create message characteristic (write without response + notify)
        messageCharacteristic = CBMutableCharacteristic(
            type: MESSAGE_CHAR_UUID,
            properties: [.writeWithoutResponse, .notify],
            value: nil,
            permissions: [.writeable]
        )

        // Create device ID characteristic (read)
        let deviceIdData = advertisedAddress.data(using: .utf8)
        deviceIdCharacteristic = CBMutableCharacteristic(
            type: DEVICE_ID_CHAR_UUID,
            properties: [.read],
            value: deviceIdData,
            permissions: [.readable]
        )
        
        // Create identity characteristic (read) - contains public key + signature
        //
        // CACHING CONTRACT — why `updateSignedIdentity()` must run before
        // `peripheral.add(service)` below, and why nothing may reorder them.
        // CoreBluetooth decides once, at `add(service:)`, whether a
        // characteristic is static or dynamic: a non-nil `value` at that moment
        // is cached and served by the framework, and a nil one makes the
        // characteristic dynamic, answered only through
        // `peripheralManager(_:didReceiveRead:)` — which this class does not
        // implement. Assigning `.value` afterwards does not convert one into
        // the other. So publishing with a nil identity means this device serves
        // no identity for the lifetime of the service, and `setupGattServer`'s
        // early return means it is never rebuilt.
        //
        // That used to degrade to "discoverable but unverified". It no longer
        // does: a central that cannot read this blob cannot bind our DEVICE_ID
        // to a key and will not surface us at all. The address guard above is
        // what makes it safe — it holds back publication until
        // `initialize_mls` has run, which is the same precondition
        // `updateSignedIdentity` needs, so the two can no longer disagree.
        identityCharacteristic = CBMutableCharacteristic(
            type: IDENTITY_CHAR_UUID,
            properties: [.read],
            value: nil,
            permissions: [.readable]
        )

        // Fills in identityCharacteristic.value — see the caching contract
        // above. Assigned straight from the cache the guard above proved
        // present, so publication is never racing an async refresh.
        identityCharacteristic?.value = signedIdentity.encode()

        // Create service
        let service = CBMutableService(type: SERVICE_UUID, primary: true)
        service.characteristics = [messageCharacteristic!, deviceIdCharacteristic!, identityCharacteristic!]
        
        // Add service to peripheral manager (asynchronous - callback in peripheralManager(_:didAdd:error:))
        peripheral.add(service)
        print("[BleManager] GATT server setup initiated, waiting for service registration callback...")
        emitDiagnostic("info", "GATT server setup initiated")
        return true
    }
    
    /// Recomputes the cached local address and signed identity, then
    /// republishes the identity characteristic.
    ///
    /// Runs entirely off the main thread. It makes three UniFFI calls —
    /// `isMlsInitialized`, `getIdentityPublicKey`, `signData` — each of which
    /// takes the core protocol mutex, and it used to be called inline from
    /// `refreshAdvertising` and `setupGattServer`, both main-queue paths. That
    /// is the second-largest App Hang cluster in OFF-2123: the scan-monitor
    /// timer and the CoreBluetooth state delegates all reach it.
    ///
    /// Callers that need the values *now* (`setupGattServer`) read the cache
    /// and defer via their existing "not ready, reschedule" path instead of
    /// waiting here.
    ///
    /// A request arriving while a pass is already running is **coalesced**, not
    /// dropped. The body spans three FFI calls that can take seconds under the
    /// very contention this fix exists to remove, and a request landing inside
    /// that window carries newer advertisement data — dropping it would leave
    /// the cache signed over a stale revision until some unrelated trigger
    /// happened to fire. One extra pass settles any burst, because each pass
    /// consumes the flag exactly once.
    private func updateSignedIdentity() {
        identityLock.lock()
        if identityRefreshInFlight {
            identityRefreshRequested = true
            identityLock.unlock()
            return
        }
        identityRefreshInFlight = true
        identityLock.unlock()

        runIdentityRefresh()
    }

    /// One refresh pass, re-dispatching itself if a request arrived while it
    /// ran. The re-dispatch is `async` onto the serial queue, so this appends
    /// to the tail rather than recursing on the stack.
    private func runIdentityRefresh() {
        onProtocolQueue { [weak self] in
            guard let self = self else { return }
            self.computeSignedIdentity()

            self.identityLock.lock()
            let repeatPass = self.identityRefreshRequested
            self.identityRefreshRequested = false
            // Stay marked in-flight while a repeat is pending, so a caller
            // arriving now coalesces into that pass rather than starting a
            // second one alongside it.
            self.identityRefreshInFlight = repeatPass
            self.identityLock.unlock()

            if repeatPass {
                self.runIdentityRefresh()
            }
        }
    }

    /// The three-UniFFI-call body of `updateSignedIdentity`. Split out only so
    /// the refresh-in-flight bookkeeping stays readable; it must never be
    /// called directly from the main queue.
    private func computeSignedIdentity() {
        #if DEBUG
        dispatchPrecondition(condition: .notOnQueue(.main))
        #endif
        do {
            guard protocolInstance.isMlsInitialized() else {
                print("[BleManager] MLS not initialized, cannot create signed identity")
                return
            }

            // Get the public key
            let publicKey = try protocolInstance.getIdentityPublicKey()

            // Get current advertisement data. `MeshController` guards its own
            // state and performs no protocol calls, so this is safe off-main.
            let meshData = meshController.advertisement()
            let advertisementData = meshData.encode()

            // Sign the advertisement data
            let signature = try protocolInstance.signData(data: [UInt8](advertisementData))

            let address = protocolInstance.localAddress()

            // Create the signed identity
            let identity = SignedIdentityData(
                publicKey: Data(publicKey),
                signature: Data(signature),
                advertisementData: advertisementData
            )
            identityLock.lock()
            cachedSignedIdentity = identity
            if let address = address, !address.isEmpty {
                cachedLocalAddress = address
            }
            identityLock.unlock()

            // Update the GATT characteristic value.
            //
            // This only reaches remote readers when it runs BEFORE
            // `peripheral.add(service)` — see the caching contract in
            // `setupGattServer`. On the refresh calls that happen after
            // publication it updates `cachedSignedIdentity` (which the
            // peripheral-role read path serves) but not what centrals read
            // from the published service, which stays frozen at the value
            // captured at `add(service)`.
            //
            // That is safe because the address inside it cannot change: the
            // core's `initialize_mls` is idempotent and refuses to run once
            // the protocol has started, so an instance's address is fixed for
            // its lifetime, and a new identity means a new instance — which on
            // this bridge means `destroy()`, which stops and releases this
            // manager. What DOES go stale is the mesh advertisement the
            // signature covers; that is a clustering hint, not an identity.
            //
            // If a future change ever makes the address mutable in-process,
            // this is the line that will silently serve the old one, and the
            // service must be torn down and rebuilt instead.
            //
            // Assigned on main: `identityCharacteristic` is a CoreBluetooth
            // object belonging to a peripheral manager whose delegate queue is
            // the main queue, and this method now runs off it.
            let encoded = identity.encode()
            DispatchQueue.main.async { [weak self] in
                self?.identityCharacteristic?.value = encoded
                print("[BleManager] Updated signed identity for GATT serving")
            }
        } catch {
            print("[BleManager] Failed to create signed identity: \(error)")
            emitDiagnostic("warning", "Failed to create signed identity", context: ["error": error.localizedDescription])
        }
    }
    
    /// Called by the Rust transport callback when new outgoing fragments are available.
    /// This replaces the timer-based `startFragmentPolling` — iOS delivers this callback
    /// even in background because it originates from within the process (no RunLoop dependency).
    public func onFragmentsAvailable() {
        DispatchQueue.main.async { [weak self] in
            self?.drainAndSendFragments()
        }
    }
    
    /// Drains the Rust fragment queue and sends each fragment over BLE.
    /// Stops when the queue is empty or all target peers are flow-controlled.
    /// Called from `onFragmentsAvailable()` and from CoreBluetooth flow-control delegates.
    private func drainAndSendFragments() {
        guard state == .running else { return }
        
        fragmentQueue.async { [weak self] in
            guard let self = self else { return }
            _ = self.flushPendingOutboundFragments()
            
            var consecutiveSkips = 0
            let maxConsecutiveSkips = 5
            var reconnectAttempted = Set<UUID>()

            while let fragment = self.protocolInstance.bleGetNextFragment() {
                let recipientId = fragment.recipientId
                let data = Data(fragment.data)

                let hasPeripheral = self.findPeripheral(for: recipientId) != nil
                if !hasPeripheral {
                    // No central link to this peer. If it subscribed to OUR GATT
                    // server (the Android-central / iOS-peripheral topology), reply
                    // over THAT link via NOTIFY instead of trying to reverse-connect
                    // as central — which iOS backgrounding routinely blocks, the exact
                    // fragile path Android abandoned in PR #120. Without this, iOS can
                    // only ever send over a link it opened, so iOS→Android stalls in
                    // this (common) topology.
                    if self.notifyTarget(for: recipientId) != nil {
                        self.enqueueNotifyOutbound(recipientId: recipientId, data: data)
                        consecutiveSkips = 0
                        continue
                    }
                    self.enqueuePendingOutboundFragment(recipientId: recipientId, data: data)
                    if let identifier = self.connections.peripheralIdentifier(for: recipientId),
                       reconnectAttempted.insert(identifier).inserted {
                        // Dispatch to main: discoveredPeripherals and connection logic must run on the main thread
                        DispatchQueue.main.async { [weak self] in
                            guard let self = self else { return }
                            if let peripheral = self.discoveredPeripherals[identifier] {
                                self.performConnectionAttempt(to: peripheral, reason: "fragment_drain_reconnect")
                            } else {
                                self.emitDiagnostic("debug", "Known peripheral not in discoveredPeripherals, skipping reconnect",
                                                   context: ["recipientId": recipientId, "identifier": identifier.uuidString])
                            }
                        }
                    }
                    consecutiveSkips += 1
                    if consecutiveSkips >= maxConsecutiveSkips {
                        break
                    }
                    continue
                }
                
                // Maintain FIFO ordering: if this recipient has pending fragments,
                // enqueue instead of sending directly.
                if self.outboundFragments.hasPending(recipientId) {
                    self.enqueuePendingOutboundFragment(recipientId: recipientId, data: data)
                    continue
                }

                consecutiveSkips = 0

                if self.sendFragmentData(recipientId: recipientId, data: data) {
                    self.emitDiagnostic("debug", "Fragment sent successfully", context: ["recipientId": recipientId])
                } else {
                    self.enqueuePendingOutboundFragment(recipientId: recipientId, data: data)
                    break
                }
            }
        }
    }
    
    // pollAndSendFragments and sendFragment removed — replaced by
    // event-driven drainAndSendFragments() triggered via onFragmentsAvailable().

    private func flushPendingOutboundFragments() -> Bool {
        // `flush` takes a non-escaping closure, so this cannot outlive the call
        // and needs no weak capture.
        outboundFragments.flush { recipientId, data in
            self.sendFragmentData(recipientId: recipientId, data: data)
        }
    }

    private func enqueuePendingOutboundFragment(recipientId: String, data: Data) {
        outboundFragments.enqueue(recipientId, data)
    }

    private func currentConnectionCount() -> Int {
        return connections.connectedPeripheralCount() + subscribedCentralCount()
    }

    // MARK: - Peripheral-role NOTIFY egress (iOS mirror of Android #120/#121)

    private func subscribedCentralCount() -> Int {
        notifyLock.lock()
        let count = subscribedCentralsById.count
        notifyLock.unlock()
        return count
    }

    /// Resolves a recipient device-id to a subscribed `CBCentral` we can notify, or
    /// nil if the peer is not notify-subscribed to our GATT server. Cross-references
    /// the retained centrals (under `notifyLock`) with the thread-safe connection
    /// registry, so it is safe to call from any queue (the drain runs on
    /// `fragmentQueue`; the pump runs on main). Mirrors Android's
    /// `subscribedNotifyAddressFor` device-scoped resolution.
    private func notifyTarget(for recipientId: String) -> CBCentral? {
        notifyLock.lock()
        let centrals = subscribedCentralsById
        notifyLock.unlock()
        for (identifier, central) in centrals {
            if connections.centralDeviceId(for: identifier) == recipientId
                || connections.peripheralDeviceId(for: identifier) == recipientId {
                return central
            }
        }
        return nil
    }

    /// Hands a fragment to the main-queue NOTIFY pump. Safe to call from the
    /// fragment drain (`fragmentQueue`): the queue mutation and `updateValue` run on
    /// main, where CBPeripheralManager lives. FIFO is preserved because `main.async`
    /// is ordered and the drain pulls fragments from Rust in order.
    private func enqueueNotifyOutbound(recipientId: String, data: Data) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            var queue = self.notifyOutbound[recipientId] ?? []
            queue.append((data: data, timestamp: Date()))
            if queue.count > self.MAX_PENDING_FRAGMENTS_PER_PEER {
                let overflow = queue.count - self.MAX_PENDING_FRAGMENTS_PER_PEER
                queue.removeFirst(overflow)
                self.emitDiagnostic("warning", "NOTIFY outbound queue capped, dropping oldest",
                                    context: ["recipientId": recipientId, "dropped": overflow,
                                              "max": self.MAX_PENDING_FRAGMENTS_PER_PEER])
            }
            self.notifyOutbound[recipientId] = queue
            self.pumpNotifyOutbound()
        }
    }

    /// Drains queued NOTIFY fragments to their subscribed centrals via
    /// `updateValue`. On a `false` return the controller's transmit queue is full;
    /// we stop and resume from `peripheralManagerIsReadyToUpdateSubscribers` — the
    /// peripheral-side analog of the central `canSendWriteWithoutResponse`
    /// backpressure the write path already honours. Main-queue only.
    private func pumpNotifyOutbound() {
        guard let pm = peripheralManager, let characteristic = messageCharacteristic else { return }
        let now = Date()
        for recipientId in Array(notifyOutbound.keys) {
            guard var queue = notifyOutbound[recipientId] else { continue }
            queue = queue.filter { now.timeIntervalSince($0.timestamp) < PENDING_OUTBOUND_FRAGMENT_TIMEOUT }
            guard let central = notifyTarget(for: recipientId) else {
                // No longer notify-reachable. Drop the queued fragments; the
                // reliability layer (MLS Welcome retransmit / RetryQueue) re-drives
                // delivery over whatever path is available next.
                notifyOutbound.removeValue(forKey: recipientId)
                continue
            }
            var transmitQueueFull = false
            while let head = queue.first {
                if pm.updateValue(head.data, for: characteristic, onSubscribedCentrals: [central]) {
                    queue.removeFirst()
                    meshController.markPeerActive(recipientId)
                    meshController.markPeerActive(deviceId)
                } else {
                    transmitQueueFull = true
                    break
                }
            }
            if queue.isEmpty {
                notifyOutbound.removeValue(forKey: recipientId)
            } else {
                notifyOutbound[recipientId] = queue
            }
            // A `false` return means the shared transmit queue is full for the whole
            // peripheral, not just this central — stop and wait for the ready callback.
            if transmitQueueFull { break }
        }
    }

    /// Reports the MIN usable payload over the links we can egress on (central WRITE
    /// and peripheral NOTIFY) to the Rust fragmenter, which sizes every fragment to
    /// one per-peer value regardless of carrier. Without the min, a fragment sized
    /// for the larger central-write link overflows the NOTIFY link and is truncated
    /// on air — the iOS analog of the bug PR #121 fixed on Android. Values are read
    /// live from CoreBluetooth (both are already ATT-header-adjusted). A sub-floor
    /// value is left for Rust's `set_peer_mtu` to reject and clamp to its 185-byte
    /// floor, so we do not duplicate that constant here. Main-queue only.
    private func reflectEgressMtu(forDeviceId deviceId: String) {
        var candidates: [Int] = []
        if let peripheral = findPeripheral(for: deviceId) {
            candidates.append(peripheral.maximumWriteValueLength(for: .withoutResponse))
        }
        if let central = notifyTarget(for: deviceId) {
            candidates.append(central.maximumUpdateValueLength)
        }
        guard let smallest = candidates.min(), smallest > 0 else { return }
        // The MTU is read from CoreBluetooth on the caller's (main) queue; only
        // the handoff to the core is deferred.
        onProtocolQueue { [weak self] in
            guard let self = self else { return }
            do {
                try self.protocolInstance.bleSetPeerMtu(peerId: deviceId, maxPayload: UInt32(smallest))
            } catch {
                self.emitDiagnostic("warning", "reflectEgressMtu bleSetPeerMtu failed",
                                    context: ["deviceId": deviceId, "error": error.localizedDescription])
            }
        }
    }

    /// Reports BLE availability to the core. Fire-and-forget, and deferred off
    /// the main thread: every caller is a CoreBluetooth state-change delegate.
    /// `fragmentQueue` is serial, so a false→true→false sequence keeps its order.
    private func notifyBleStatus(_ isAvailable: Bool) {
        onProtocolQueue { [weak self] in
            try? self?.protocolInstance.bleStatusChanged(isAvailable: isAvailable)
        }
    }

    /// Tears down the protocol-side state for a peer that has been lost —
    /// routing entries and the BLE peer-lost signal. Every
    /// disconnect/eviction/give-up path funnels through here so the two
    /// UniFFI calls stay in lockstep. Local bookkeeping (`connections`,
    /// `meshController`, `refreshSelfMetrics`, etc.) is intentionally
    /// left at the call site because not every path removes the same
    /// local state — only the protocol-side teardown is uniform.
    ///
    /// `blePeerLost` also drops the per-peer MTU entry inside the Rust
    /// transport, so no separate `bleClearPeerMtu` call is needed here.
    /// For mid-link renegotiation paths that need to drop the MTU
    /// without declaring the peer lost, call `bleClearPeerMtu` directly.
    private func notifyBlePeerLost(deviceId: String) {
        // Every caller is a main-queue disconnect/eviction path, and
        // `blePeerLost` takes the core protocol mutex.
        onProtocolQueue { [weak self] in
            guard let self = self else { return }
            do {
                try self.protocolInstance.blePeerLost(peerId: deviceId)
            } catch {
                self.emitDiagnostic("warning", "blePeerLost failed", context: [
                    "deviceId": deviceId,
                    "error": error.localizedDescription,
                ])
            }
        }
    }

    /// Refresh self metrics.
    ///
    /// The fragment counts used to require either a `fragmentQueue.sync` (from
    /// off-queue callers, which parked the main thread behind whatever the
    /// queue was doing) or hand-threaded parameters (from on-queue callers, to
    /// avoid deadlocking that serial queue). Both stores answer their own
    /// counts from any thread now, so the split is gone.
    private func refreshSelfMetrics() {
        let rssiValues = peripheralRSSI.values.map { Int($0) }
        let averageRssi = rssiValues.isEmpty ? nil : Int(Double(rssiValues.reduce(0, +)) / Double(rssiValues.count))
        let signalQuality = averageRssi.map { rssi -> Int in
            let clamped = max(-100, min(-20, rssi))
            let normalized = Double(clamped + 100) / 80.0
            let scaled = Int((normalized * 100.0).rounded())
            return min(100, max(0, scaled))
        }
        let pc = inboundFragments.totalCount()
        let oc = outboundFragments.totalCount()
        let totalPending = pc + oc
        let stability = max(0.0, 1.0 - min(1.0, Double(pc) / 10.0))
        let loadPercent = min(100, (totalPending * 100) / LOAD_SATURATION_COUNT)
        let uptimeSeconds = transportStartAt.map { max(0, Date().timeIntervalSince($0)) }
        let metrics = MeshController.PeerMetrics(
            rssi: averageRssi,
            batteryPercent: currentBatteryPercent(),
            signalQuality: signalQuality,
            stability: stability,
            uptimeSeconds: uptimeSeconds,
            loadPercent: loadPercent
        )
        meshController.updateSelfMetrics(metrics)
        meshController.markPeerActive(deviceId)
        maybeHandleRebalance(reason: "self_metrics")
    }

    private func currentBatteryPercent() -> Int? {
        UIDevice.current.isBatteryMonitoringEnabled = true
        let level = UIDevice.current.batteryLevel
        guard level >= 0 else { return nil }
        return Int((level * 100).rounded())
    }

    private func evictPeer(_ deviceId: String, reason: String) {
        guard let identifier = connections.peripheralIdentifier(for: deviceId) else {
            if logThrottler.shouldLog(key: "mesh_evict_missing_\(deviceId)") {
                print("[BleManager] Cannot evict \(deviceId): missing identifier")
            }
            return
        }

        if logThrottler.shouldLog(key: "mesh_evict_\(deviceId)", interval: 5) {
            print("[BleManager] Evicting \(deviceId) to reclaim capacity (reason: \(reason))")
        }

        if let peripheral = connections.connectedPeripheral(for: identifier) {
            centralManager?.cancelPeripheralConnection(peripheral)
        }

        _ = connections.removePeripheral(identifier)
        connections.removePeripheralDeviceId(for: identifier)
        connections.removeCentralDeviceId(for: identifier)
        connections.removeConnectionRole(for: deviceId)
        peripheralRSSI.removeValue(forKey: identifier)
        // Removals are synchronous and thread-safe, so `refreshSelfMetrics()`
        // below still sees the post-removal counts — without the
        // `fragmentQueue.sync` that used to park the main thread here.
        inboundFragments.removeAll(identifier)
        outboundFragments.removeAll(deviceId)
        // A fragment block dispatched before this point runs after the removals
        // above and would resurrect the evicted peer's buffer. Chase it on the
        // owning queue: serial and FIFO, so this drops exactly the backlog that
        // was already in flight, while anything for a genuine later reconnect
        // is enqueued behind this block and survives. Locals, not `[weak self]`
        // — same deinit rule as `stopUnsafe`.
        let inbound = inboundFragments
        let outbound = outboundFragments
        fragmentQueue.async {
            inbound.removeAll(identifier)
            outbound.removeAll(deviceId)
        }
        connectionAttemptTimestamps.removeValue(forKey: identifier)
        connectionRetryCount.removeValue(forKey: identifier)
        meshController.registerDisconnection(peerId: deviceId)
        refreshSelfMetrics()

        notifyBlePeerLost(deviceId: deviceId)
        DispatchQueue.main.async {
            self.refreshAdvertising(reason: "evict_\(reason)")
        }
        maybeHandleRebalance(reason: "evict")
    }
    
    private func sendFragmentData(recipientId: String, data: Data) -> Bool {
        guard let peripheral = findPeripheral(for: recipientId) else {
            //  Proactively try to connect if we don't have a connection
            // This helps resolve cases where fragments are queued but connection isn't established
            if logThrottler.shouldLog(key: "missing_peripheral_\(recipientId)", interval: 5.0) {
                print("[BleManager] ⚠️ No connected peripheral for recipient: \(recipientId) - attempting to find and connect")
                emitDiagnostic("warning", "No connected peripheral for BLE fragment - attempting connection", context: ["recipientId": recipientId])
            }
            
            // Try to find the peripheral and connect
            if let identifier = connections.peripheralIdentifier(for: recipientId) {
                // We know the UUID but don't have a connection - try to reconnect.
                //
                // Read `discoveredPeripherals` on main: it is a plain
                // Dictionary the CoreBluetooth delegates mutate there, and both
                // callers of this method run on `fragmentQueue`. The identical
                // reconnect in `drainAndSendFragments` already hops for exactly
                // this reason; this site was missed. Async is fine — the
                // reconnect is advisory and this path returns false either way.
                DispatchQueue.main.async { [weak self] in
                    guard let self = self,
                          let peripheral = self.discoveredPeripherals[identifier] else { return }
                    self.performConnectionAttempt(to: peripheral, reason: "fragment_send")
                }
            } else {
                // We don't even know the UUID - this is a more serious issue
                // The device ID might not be resolved yet
                print("[BleManager] ⚠️ Cannot find peripheral UUID for recipient: \(recipientId)")
            }
            return false
        }
        
        //  Validate connection state before attempting to send
        guard peripheral.state == .connected else {
            if logThrottler.shouldLog(key: "peripheral_not_connected_\(recipientId)", interval: 5.0) {
                print("[BleManager] ⚠️ Peripheral for \(recipientId) is not connected (state: \(peripheral.state.rawValue))")
                emitDiagnostic("warning", "Peripheral not connected", context: [
                    "recipientId": recipientId,
                    "state": peripheral.state.rawValue
                ])
            }
            // Try to reconnect
            attemptConnection(to: peripheral, reason: "fragment_send_reconnect")
            return false
        }
        
        guard let (service, characteristic) = findMessageCharacteristic(on: peripheral) else {
            if logThrottler.shouldLog(key: "missing_char_\(recipientId)", interval: 5.0) {
                print("[BleManager] ⚠️ Message characteristic not found for recipient: \(recipientId) - may need to discover services")
                emitDiagnostic("warning", "Message characteristic not found - discovering services", context: ["recipientId": recipientId])
            }
            // Try to discover services if not already discovered
            if peripheral.services == nil || peripheral.services?.isEmpty == true {
                peripheral.discoverServices([SERVICE_UUID])
            }
            return false
        }
        
        if #available(iOS 11.0, *) {
            if !peripheral.canSendWriteWithoutResponse {
                if logThrottler.shouldLog(key: "write_backpressure_\(recipientId)", interval: 2.0) {
                    print("[BleManager] Cannot send fragment yet, write buffer full for recipient: \(recipientId)")
                    emitDiagnostic("info", "BLE write buffer full, retrying", context: ["recipientId": recipientId])
                }
                return false
            }
        }
        
        peripheral.writeValue(data, for: characteristic, type: .withoutResponse)
        meshController.markPeerActive(recipientId)
        meshController.markPeerActive(deviceId)
        return true
    }
    
    private func findPeripheral(for recipientId: String) -> CBPeripheral? {
        guard let identifier = connections.peripheralIdentifier(for: recipientId) else {
            return nil
        }
        return connections.connectedPeripheral(for: identifier)
    }
    
    private func findMessageCharacteristic(on peripheral: CBPeripheral) -> (CBService, CBCharacteristic)? {
        guard let service = peripheral.services?.first(where: { $0.uuid == SERVICE_UUID }),
              let characteristic = service.characteristics?.first(where: { $0.uuid == MESSAGE_CHAR_UUID }) else {
            return nil
        }
        return (service, characteristic)
    }
    
    private func handleReceivedData(_ data: Data, senderId: String?, centralId: UUID? = nil) {
        fragmentQueue.async { [weak self] in
            guard let self = self else { return }
            
            // If sender ID is missing but we have a central ID, queue the fragment
            if senderId == nil, let centralId = centralId {
                if self.logThrottler.shouldLog(key: "queue_pending_fragment_\(centralId.uuidString)", interval: 10) {
                    print("[BleManager] Queueing fragment while waiting for device ID (central: \(centralId))")
                    self.emitDiagnostic("info", "Queued BLE fragment pending device ID", context: [
                        "central": centralId.uuidString,
                        "length": data.count
                    ])
                }
                // Queue fragment to process later when device ID is available.
                // Overflow policy (whole-buffer drop) lives in the store.
                self.inboundFragments.enqueue(centralId, data)

                // Clean up old pending fragments
                self.cleanupPendingFragments()
                
                // Try to read device ID if not already reading
                if self.connections.centralDeviceId(for: centralId) == nil && self.connections.peripheralDeviceId(for: centralId) == nil {
                    self.ensureDeviceId(for: centralId)
                }
                return
            }
            
            guard let senderId = senderId else {
                // Only log if we don't have a central ID to queue for
                if centralId == nil {
                    if self.logThrottler.shouldLog(key: "missing_sender_fallback", interval: 10) {
                        print("[BleManager] Missing sender ID for received fragment")
                        self.emitDiagnostic("warning", "Dropped BLE fragment without sender ID", context: ["length": data.count])
                    }
                }
                return
            }

            // If there are pending fragments for this sender, append to maintain ordering.
            // processPendingFragments() will handle them all in FIFO order.
            if let centralId = centralId,
               self.inboundFragments.enqueueIfPending(centralId, data) {
                return
            }

            let bytes = [UInt8](data)
            self.meshController.markPeerActive(senderId)
            self.meshController.markPeerActive(self.deviceId)

            do {
                print("[BleManager] 📥 RECEIVED FRAGMENT from \(senderId), size: \(data.count)")
                self.emitDiagnostic("info", "Fragment received from BLE", context: [
                    "senderId": senderId,
                    "fragmentSize": data.count
                ])
                
                try self.protocolInstance.bleFragmentReceived(senderId: senderId, fragment: bytes)
                print("[BleManager] ✅ Fragment processed successfully for sender: \(senderId)")
                
                //  Check for ALL completed messages (not just one)
                // The protocol may have queued multiple messages, so we need to drain the queue
                var messageCount = 0
                while let completedMessage = self.protocolInstance.receiveMessage() {
                    messageCount += 1
                    print("[BleManager] 🎉 COMPLETE MESSAGE #\(messageCount) ASSEMBLED FROM FRAGMENTS!")
                    print("[BleManager] 📬 Received message: \(completedMessage)")
                    self.emitDiagnostic("info", "Complete message assembled from fragments", context: [
                        "senderId": senderId,
                        "messageContent": completedMessage,
                        "messageNumber": messageCount
                    ])
                }
                
                if messageCount == 0 {
                    print("[BleManager] 📦 Fragment processed, waiting for more fragments to complete message")
                } else {
                    print("[BleManager] ✅ Processed \(messageCount) complete message(s) from fragments")
                }
            } catch {
                print("[BleManager] ❌ Error processing fragment from \(senderId): \(error)")
                self.emitDiagnostic("error", "Error processing received fragment", context: [
                    "senderId": senderId,
                    "fragmentSize": data.count,
                    "error": error.localizedDescription
                ])
            }
        }
    }
    
    private func processPendingFragments(for centralId: UUID, deviceId: String) {
        // A device-id just resolved for this link. If the peer is notify-reachable,
        // clamp the Rust fragmenter to the NOTIFY link's payload (min with any
        // central-write link) and flush any queued NOTIFY fragments. Runs on the
        // caller's (main) queue — every caller is a CoreBluetooth delegate — which is
        // required: `pumpNotifyOutbound` drives CBPeripheralManager. This is not
        // gated on inbound fragments existing, so it also covers a central that
        // subscribed before we knew its device-id.
        reflectEgressMtu(forDeviceId: deviceId)
        pumpNotifyOutbound()
        fragmentQueue.async { [weak self] in
            guard let self = self else { return }
            let fragments = self.inboundFragments.drain(centralId)
            if fragments.isEmpty {
                // No fragments for this central ID - this is normal
                return
            }

            print("[BleManager] 🔄 Processing \(fragments.count) pending fragments for device \(deviceId) (central: \(centralId))")
            self.emitDiagnostic("info", "Processing pending fragments", context: [
                "deviceId": deviceId,
                "centralId": centralId.uuidString,
                "fragmentCount": fragments.count
            ])
            
            let role = self.connections.consumePendingRole(for: centralId) ?? self.connections.connectionRole(for: deviceId) ?? .member
            self.meshController.registerConnection(peerId: deviceId, role: role)
            self.connections.setConnectionRole(role, for: deviceId)
            self.meshController.markPeerActive(deviceId)
            self.meshController.markPeerActive(self.deviceId)
            // `refreshSelfMetrics` reads `peripheralRSSI` — a plain Dictionary
            // that the CoreBluetooth delegates mutate on main — and samples
            // `UIDevice.batteryLevel`, which is UIKit. Both were being touched
            // from this queue: an unsynchronised cross-thread read and a UIKit
            // call off the main thread. Hop for both, along with the RSSI read
            // just below it. Fire-and-forget telemetry, so async costs nothing.
            DispatchQueue.main.async {
                self.refreshSelfMetrics()
                if let rssi = self.peripheralRSSI[centralId] {
                    self.meshController.updatePeerMetrics(peerId: deviceId, metrics: MeshController.PeerMetrics(rssi: Int(rssi)))
                }
                self.refreshAdvertising(reason: "membership_change")
            }
            
            //  Process all queued fragments and check for completed messages
            // This is essential for Android → iOS messages that were queued
            for data in fragments {
                let bytes = [UInt8](data)
                do {
                    print("[BleManager] 📥 Processing queued fragment from \(deviceId), size: \(data.count)")
                    try self.protocolInstance.bleFragmentReceived(senderId: deviceId, fragment: bytes)
                    self.meshController.markPeerActive(deviceId)
                    self.meshController.markPeerActive(self.deviceId)
                    
                    // Check for ALL completed messages (not just one)
                    // The protocol may have queued multiple messages
                    var messageCount = 0
                    while let completedMessage = self.protocolInstance.receiveMessage() {
                        messageCount += 1
                        print("[BleManager] 🎉 COMPLETE MESSAGE #\(messageCount) ASSEMBLED FROM QUEUED FRAGMENTS!")
                        print("[BleManager] 📬 Received message: \(completedMessage)")
                        self.emitDiagnostic("info", "Complete message assembled from queued fragments", context: [
                            "senderId": deviceId,
                            "messageContent": completedMessage,
                            "messageNumber": messageCount
                        ])
                    }
                    if messageCount > 0 {
                        print("[BleManager] ✅ Processed \(messageCount) complete message(s) from queued fragments")
                    }
                } catch {
                    print("[BleManager] ❌ Error processing pending fragment from \(deviceId): \(error)")
                    self.emitDiagnostic("error", "Error processing pending fragment", context: [
                        "deviceId": deviceId,
                        "error": error.localizedDescription
                    ])
                }
            }
            
            print("[BleManager] ✅ Finished processing \(fragments.count) pending fragments for device \(deviceId)")
        }
    }
    
    private func cleanupPendingFragments() {
        // Idle-window eviction at WHOLE-BUFFER granularity, keyed on each
        // buffer's newest fragment — the rationale now lives on
        // `InboundFragmentBuffer.evictExpired()`, along with a test that pins
        // it (a still-arriving multi-fragment message must survive).
        inboundFragments.evictExpired()
    }

    private func pruneMeshObservations(now: Date = Date()) {
        lastSeenMeshAdvertisements = lastSeenMeshAdvertisements.filter { now.timeIntervalSince($0.value.timestamp) <= MESH_OBSERVATION_TTL }
        unknownBootstrapAttempts = unknownBootstrapAttempts.filter { now.timeIntervalSince($0.value) <= 60.0 }
        
    }
    
    // MARK: - Adaptive Scan Methods
    
    /// Updates the estimated visible peer count based on recent discoveries.
    private func updateVisiblePeerCount(now: Date) {
        // Only update periodically to avoid overhead
        if let lastUpdate = lastPeerCountUpdate, now.timeIntervalSince(lastUpdate) < 1.0 {
            return
        }
        lastPeerCountUpdate = now
        
        // Clean up old timestamps
        let windowStart = now.addingTimeInterval(-ADAPTIVE_PEER_COUNT_WINDOW)
        recentDiscoveryTimestamps = recentDiscoveryTimestamps.filter { $0 > windowStart }
        
        // Estimate peer count from unique discoveries in window
        // Also consider cached observations as a lower bound
        let recentCount = recentDiscoveryTimestamps.count
        let cachedCount = lastSeenMeshAdvertisements.count
        estimatedVisiblePeerCount = max(recentCount, cachedCount)
    }
    
    /// Records a peripheral discovery for density estimation.
    private func recordDiscoveryForDensity(now: Date) {
        recentDiscoveryTimestamps.append(now)
        updateVisiblePeerCount(now: now)
    }
    
    /// Checks if we should skip this peripheral based on RSSI filtering.
    /// Returns true if the signal is too weak and we're in a dense environment.
    private func shouldFilterByRssi(_ rssi: Int16) -> Bool {
        // During aggressive discovery phase, don't apply density-based filtering
        if let started = aggressiveDiscoveryStarted,
           Date().timeIntervalSince(started) < AGGRESSIVE_DISCOVERY_PHASE {
            // Only filter out extremely weak signals during aggressive phase
            return rssi < MINIMUM_RSSI_TO_CONNECT
        }
        
        // In dense networks, apply stricter RSSI filtering
        let threshold: Int16
        if estimatedVisiblePeerCount > ADAPTIVE_HIGH_DENSITY_THRESHOLD {
            // Very dense - only consider strong signals
            threshold = -70
        } else if estimatedVisiblePeerCount > ADAPTIVE_LOW_DENSITY_THRESHOLD {
            // Moderately dense - standard threshold
            threshold = ADAPTIVE_MIN_RSSI
        } else {
            // Sparse network - accept all signals
            return false
        }
        return rssi < threshold
    }
    
    /// Checks if we should throttle connection attempts based on rate limits.
    /// Returns true if we should skip this connection attempt.
    private func shouldThrottleConnection(to peripheral: UUID, now: Date) -> Bool {
        // During aggressive discovery phase, use much shorter cooldowns
        let isAggressivePhase = aggressiveDiscoveryStarted.map { now.timeIntervalSince($0) < AGGRESSIVE_DISCOVERY_PHASE } ?? false
        
        // Prune old entries
        let oneMinuteAgo = now.addingTimeInterval(-60.0)
        globalConnectionAttempts = globalConnectionAttempts.filter { $0 > oneMinuteAgo }
        
        let effectiveCooldown: TimeInterval = isAggressivePhase ? 5.0 : ADAPTIVE_COOLDOWN_PER_PERIPHERAL
        peripheralConnectionAttempts = peripheralConnectionAttempts.filter { 
            now.timeIntervalSince($0.value) < effectiveCooldown 
        }
        
        // Check per-peripheral cooldown
        if let lastAttempt = peripheralConnectionAttempts[peripheral],
           now.timeIntervalSince(lastAttempt) < effectiveCooldown {
            return true
        }
        
        // During aggressive phase, allow more connection attempts
        if isAggressivePhase {
            // Allow up to 3x the normal rate during aggressive phase
            let maxAttempts = ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE * 3
            if globalConnectionAttempts.count >= maxAttempts {
                return true
            }
            return false
        }
        
        // In dense networks, apply global rate limiting
        if estimatedVisiblePeerCount > ADAPTIVE_LOW_DENSITY_THRESHOLD {
            let maxAttempts = ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE
            if globalConnectionAttempts.count >= maxAttempts {
                if logThrottler.shouldLog(key: "adaptive_rate_limit", interval: 5) {
                    print("[BleManager] Adaptive: rate limiting connections (\(globalConnectionAttempts.count)/\(maxAttempts) in last minute)")
                }
                return true
            }
        }
        
        return false
    }
    
    /// Records a connection attempt for rate limiting.
    private func recordConnectionAttempt(to peripheral: UUID, now: Date) {
        peripheralConnectionAttempts[peripheral] = now
        globalConnectionAttempts.append(now)
    }
    
    /// Returns true if we should apply probabilistic filtering based on network density.
    /// Uses deterministic pseudo-randomness based on peripheral ID to ensure consistency.
    private func shouldProbabilisticallySkip(_ peripheral: UUID) -> Bool {
        guard estimatedVisiblePeerCount > ADAPTIVE_LOW_DENSITY_THRESHOLD else {
            return false
        }
        
        // Calculate skip probability based on density
        // At 50+ peers, skip ~80% of evaluations
        // At 10-50 peers, scale linearly
        let density = Double(estimatedVisiblePeerCount - ADAPTIVE_LOW_DENSITY_THRESHOLD)
        let range = Double(ADAPTIVE_HIGH_DENSITY_THRESHOLD - ADAPTIVE_LOW_DENSITY_THRESHOLD)
        let skipProbability = min(0.8, density / range * 0.8)
        
        // Use peripheral UUID hash for deterministic selection
        let hash = peripheral.hashValue
        let normalizedHash = Double(abs(hash) % 1000) / 1000.0
        
        return normalizedHash < skipProbability
    }
    
    // MARK: - Smart Filtering for iOS ↔ Android Interoperability
    
    /// Determines if a discovered peripheral should be processed.
    /// This implements smart filtering since we scan without a service UUID filter
    /// (required for iOS ↔ Android interoperability).
    ///
    /// Accepts:
    /// - Devices advertising our service UUID (iOS devices)
    /// - Devices with our service data
    /// - Previously discovered mesh devices
    /// - Previously verified peer/device mappings
    /// - Strictly rate-limited bootstrap attempts for unknown connectable peripherals
    private func shouldProcessDiscoveredPeripheral(
        peripheral: CBPeripheral,
        advertisementData: [String: Any],
        rssi: Int16,
        isConnectable: Bool,
        now: Date
    ) -> Bool {
        // 0. Skip devices previously verified as non-mesh via GATT
        if let nonMeshTimestamp = verifiedNonMeshDevices[peripheral.identifier] {
            if now.timeIntervalSince(nonMeshTimestamp) < NON_MESH_CACHE_TTL {
                logDiscoveryRejection(
                    peripheral: peripheral,
                    reason: "non_mesh_cache",
                    now: now,
                    context: ["ageMs": Int(now.timeIntervalSince(nonMeshTimestamp) * 1000)]
                )
                return false
            }
            // Entry expired, remove it and allow re-evaluation
            verifiedNonMeshDevices.removeValue(forKey: peripheral.identifier)
        }
        
        // 1. Check if device is advertising our service UUID
        if let serviceUUIDs = advertisementData[CBAdvertisementDataServiceUUIDsKey] as? [CBUUID] {
            if serviceUUIDs.contains(SERVICE_UUID) {
                return true
            }
        }
        
        // 2. Check for our service data (may come from scan response)
        if let serviceData = advertisementData[CBAdvertisementDataServiceDataKey] as? [CBUUID: Data] {
            if serviceData[SERVICE_UUID] != nil {
                return true
            }
        }
        
        // 2b. Check overflow service UUIDs - Android devices sometimes advertise in overflow area
        //     when the main advertisement packet is full
        if let overflowUUIDs = advertisementData[CBAdvertisementDataOverflowServiceUUIDsKey] as? [CBUUID] {
            if overflowUUIDs.contains(SERVICE_UUID) {
                return true
            }
        }
        
        // 2c. Check solicited service UUIDs - some Android devices use this
        if let solicitedUUIDs = advertisementData[CBAdvertisementDataSolicitedServiceUUIDsKey] as? [CBUUID] {
            if solicitedUUIDs.contains(SERVICE_UUID) {
                return true
            }
        }
        
        // 3. Check if this is a previously discovered mesh device
        if lastSeenMeshAdvertisements[peripheral.identifier] != nil {
            return true
        }
        
        // 4. Check if we already have this peripheral in our discovered list
        //    (previously connected or successfully verified via GATT)
        if discoveredPeripherals[peripheral.identifier] != nil {
            return true
        }
        
        // 5. Check if we already have a device ID mapping for this peripheral
        if connections.peripheralDeviceId(for: peripheral.identifier) != nil ||
           connections.centralDeviceId(for: peripheral.identifier) != nil {
            return true
        }

        // Controlled bootstrap for unknown connectable peripherals.
        // Missing advertisement keys are treated as unknown (not invalid), while
        // strict rate/rssi limits prevent broad probing.
        if shouldAllowUnknownBootstrap(
            peripheral: peripheral,
            advertisementData: advertisementData,
            rssi: rssi,
            isConnectable: isConnectable,
            now: now
        ) {
            if logThrottler.shouldLog(key: "bootstrap_allow_\(peripheral.identifier.uuidString)", interval: 30) {
                print("[BleManager] Allowing provisional bootstrap for \(peripheral.identifier) RSSI=\(rssi)")
                emitDiagnostic("debug", "Allowing provisional bootstrap candidate", context: [
                    "identifier": peripheral.identifier.uuidString,
                    "rssi": rssi,
                    "connectable": isConnectable
                ])
            }
            return true
        }
        
        // Filter out all other devices (not our mesh network)
        logDiscoveryRejection(
            peripheral: peripheral,
            reason: "unknown_candidate_blocked",
            now: now,
            context: [
                "rssi": rssi,
                "connectable": isConnectable,
                "hasServiceUUIDs": advertisementData[CBAdvertisementDataServiceUUIDsKey] != nil,
                "hasServiceData": advertisementData[CBAdvertisementDataServiceDataKey] != nil
            ]
        )
        return false
    }

    private func shouldAllowUnknownBootstrap(
        peripheral: CBPeripheral,
        advertisementData: [String: Any],
        rssi: Int16,
        isConnectable: Bool,
        now: Date
    ) -> Bool {
        let hasAnyServiceKey =
            advertisementData[CBAdvertisementDataServiceUUIDsKey] != nil ||
            advertisementData[CBAdvertisementDataServiceDataKey] != nil ||
            advertisementData[CBAdvertisementDataOverflowServiceUUIDsKey] != nil ||
            advertisementData[CBAdvertisementDataSolicitedServiceUUIDsKey] != nil

        let lastAttempt = unknownBootstrapAttempts[peripheral.identifier]
        let oneMinuteAgo = now.addingTimeInterval(-60.0)
        let recentBootstrapAttempts = unknownBootstrapAttempts.values.filter { $0 > oneMinuteAgo }.count
        let recentConnectionAttempts = globalConnectionAttempts.filter { $0 > oneMinuteAgo }.count
        let shouldAllow = BleDiscoveryBootstrapPolicy.shouldAllowCandidate(
            isConnectable: isConnectable,
            currentConnectionCount: currentConnectionCount(),
            maxConnectionsPerDevice: MAX_CONNECTIONS_PER_DEVICE,
            estimatedVisiblePeerCount: estimatedVisiblePeerCount,
            densePeerThreshold: ADAPTIVE_HIGH_DENSITY_THRESHOLD,
            rssi: rssi,
            hasAnyServiceKey: hasAnyServiceKey,
            minRssiWithServiceKeys: UNKNOWN_BOOTSTRAP_MIN_RSSI,
            minRssiWithoutServiceKeys: UNKNOWN_BOOTSTRAP_MIN_RSSI_WITH_MISSING_KEYS,
            lastAttemptAt: lastAttempt,
            now: now,
            perDeviceCooldown: UNKNOWN_BOOTSTRAP_RATE_LIMIT,
            recentBootstrapAttempts: recentBootstrapAttempts,
            maxBootstrapAttemptsPerMinute: MAX_UNKNOWN_BOOTSTRAP_ATTEMPTS_PER_MINUTE,
            recentConnectionAttempts: recentConnectionAttempts,
            maxConnectionAttemptsPerMinute: ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE
        )
        guard shouldAllow else { return false }

        unknownBootstrapAttempts[peripheral.identifier] = now
        return true
    }

    private func logDiscoveryRejection(
        peripheral: CBPeripheral,
        reason: String,
        now: Date,
        context: [String: Any] = [:]
    ) {
        let key = "reject_\(reason)_\(peripheral.identifier.uuidString)"
        guard logThrottler.shouldLog(key: key, interval: 30, now: now) else { return }
        print("[BleManager] Skipping discovered peripheral \(peripheral.identifier) (\(reason))")
        emitDiagnostic("debug", "Skipping discovered BLE peripheral", context: context.merging([
            "identifier": peripheral.identifier.uuidString,
            "reason": reason
        ]) { current, _ in current })
    }

    private func identifierForNodeHash(_ nodeHash: UInt64) -> UUID? {
        for (identifier, observation) in lastSeenMeshAdvertisements where observation.advertisement.nodeIdHash == nodeHash {
            return identifier
        }
        return nil
    }
    
    /// Computes a hash of the advertisement data for duplicate detection.
    /// Uses peripheral ID, RSSI bucket, and key advertisement data.
    private func computeAdvertisementHash(peripheral: CBPeripheral, advertisementData: [String: Any], rssi: Int16) -> Int {
        var hasher = Hasher()
        hasher.combine(peripheral.identifier)
        // Use RSSI buckets of 5 dBm to avoid hash changes from minor signal fluctuations
        hasher.combine(rssi / 5)
        
        // Include service UUIDs if present
        if let serviceUUIDs = advertisementData[CBAdvertisementDataServiceUUIDsKey] as? [CBUUID] {
            for uuid in serviceUUIDs {
                hasher.combine(uuid.uuidString)
            }
        }
        
        // Include service data if present
        if let serviceData = advertisementData[CBAdvertisementDataServiceDataKey] as? [CBUUID: Data] {
            for (uuid, data) in serviceData {
                hasher.combine(uuid.uuidString)
                hasher.combine(data)
            }
        }
        
        return hasher.finalize()
    }

    private func maybeHandleRebalance(reason: String) {
        pruneMeshObservations()
        guard let directive = meshController.evaluateRebalance() else { return }

        if let evictPeerId = directive.decision.evictPeerId {
            self.evictPeer(evictPeerId, reason: "rebalance_\(reason)")
        }

        guard meshController.connectionBudgetAvailable() || directive.decision.evictPeerId != nil else { return }
        guard currentConnectionCount() < MAX_CONNECTIONS_PER_DEVICE else { return }

        guard let identifier = identifierForNodeHash(directive.candidate.nodeIdHash),
              let peripheral = discoveredPeripherals[identifier] else {
            return
        }

        let desiredRole: MeshController.MeshRole = directive.decision.intent == .interCluster ? .bridge : .member
        connections.setPendingRole(desiredRole, for: identifier)
        attemptConnection(to: peripheral, reason: "rebalance", desiredRole: desiredRole)
    }
    
    private func readDeviceId(from peripheral: CBPeripheral) {
        guard let service = peripheral.services?.first(where: { $0.uuid == SERVICE_UUID }),
              let characteristic = service.characteristics?.first(where: { $0.uuid == DEVICE_ID_CHAR_UUID }) else {
            return
        }
        
        peripheral.readValue(for: characteristic)
    }
}

// MARK: - CBCentralManagerDelegate

extension BleManager: CBCentralManagerDelegate {
    
    /// State restoration: called before `centralManagerDidUpdateState` when iOS
    /// relaunches the app after termination. Restores previously connected peripherals
    /// so the mesh can resume without waiting for new advertisements.
    public func centralManager(_ central: CBCentralManager, willRestoreState dict: [String: Any]) {
        print("[BleManager] Restoring central manager state")
        emitDiagnostic("info", "Central manager restoring state", context: [
            "keys": Array(dict.keys)
        ])
        
        if let peripherals = dict[CBCentralManagerRestoredStatePeripheralsKey] as? [CBPeripheral] {
            for peripheral in peripherals {
                peripheral.delegate = self
                discoveredPeripherals[peripheral.identifier] = peripheral
                connections.registerPeripheral(peripheral)
                
                print("[BleManager] Restored peripheral: \(peripheral.identifier), state: \(peripheral.state.rawValue)")
                emitDiagnostic("info", "Restored peripheral from state restoration", context: [
                    "identifier": peripheral.identifier.uuidString,
                    "state": peripheral.state.rawValue
                ])
                
                // If the peripheral was connected, rediscover services
                if peripheral.state == .connected {
                    peripheral.discoverServices([SERVICE_UUID])
                } else {
                    // Try to reconnect
                    central.connect(peripheral, options: nil)
                }
            }
        }
    }
    
    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        let stateString: String
        var authStatus = "unknown"
        
        if #available(iOS 13.1, *) {
            let authorization = CBCentralManager.authorization
            switch authorization {
            case .notDetermined:
                authStatus = "notDetermined"
            case .restricted:
                authStatus = "restricted"
            case .denied:
                authStatus = "denied"
            case .allowedAlways:
                authStatus = "allowedAlways"
            @unknown default:
                authStatus = "unknown"
            }
        }
        
        switch central.state {
        case .unknown:
            stateString = "unknown"
        case .resetting:
            stateString = "resetting"
        case .unsupported:
            stateString = "unsupported"
        case .unauthorized:
            stateString = "unauthorized"
        case .poweredOff:
            stateString = "poweredOff"
        case .poweredOn:
            stateString = "poweredOn"
        @unknown default:
            stateString = "unknown"
        }
        
        print("[BleManager] Central state: \(stateString), authorization: \(authStatus)")
        emitDiagnostic("info", "Central manager state changed", context: [
            "state": stateString,
            "stateRaw": central.state.rawValue,
            "authorization": authStatus
        ])
        
        switch central.state {
        case .poweredOn:
            // Check authorization status on iOS 13.1+
            if #available(iOS 13.1, *) {
                let authorization = CBCentralManager.authorization
                switch authorization {
                case .denied, .restricted:
                    print("[BleManager] ⚠️ Bluetooth permission denied or restricted")
                    emitDiagnostic("error", "Bluetooth permission denied", context: ["authorization": authStatus])
                    centralReady = false
                    updateState(.unavailable)
                    notifyBleStatus(false)
                    return
                    
                case .notDetermined:
                    print("[BleManager] 🔔 Bluetooth permission not determined yet, waiting for user response...")
                    emitDiagnostic("info", "Waiting for Bluetooth permission", context: ["authorization": authStatus])
                    // Permission prompt should be showing now
                    // Will get called again when user responds
                    return
                    
                case .allowedAlways:
                    print("[BleManager] ✅ Bluetooth permission granted")
                    emitDiagnostic("info", "Bluetooth permission granted", context: ["authorization": authStatus])
                    
                @unknown default:
                    print("[BleManager] ⚠️ Unknown authorization state")
                    emitDiagnostic("warning", "Unknown authorization state", context: ["authorization": authStatus])
                }
            }
            
            centralReady = true
            startScanning(reason: "central_powered_on")
            emitDiagnostic("info", "Central manager powered on and ready")
            
            // Drain any fragments that may have queued while BLE was unavailable
            drainAndSendFragments()
            
            // If both central and peripheral are ready, mark as running
            if peripheralReady && state == .starting {
                updateState(.running)
                print("[BleManager] ✅ BLE Manager ready - dispatching bleStatusChanged(true)")
                // "dispatched", not "called": the FFI now runs on the protocol
                // queue, so this returns before the core has been told. Reading
                // these as a completion is how you mis-time a hang.
                notifyBleStatus(true)
                emitDiagnostic("info", "Dispatched protocol.bleStatusChanged(true)")
            }
            
        case .poweredOff:
            print("[BleManager] ⚠️ Bluetooth is powered off")
            centralReady = false
            stopScanning(reason: "central_powered_off")
            updateState(.unavailable)
            notifyBleStatus(false)
            emitDiagnostic("warning", "Bluetooth is powered off", context: ["state": stateString])
            
        case .unauthorized:
            print("[BleManager] ⚠️ Bluetooth is unauthorized")
            centralReady = false
            stopScanning(reason: "central_unauthorized")
            updateState(.unavailable)
            notifyBleStatus(false)
            emitDiagnostic("error", "Bluetooth is unauthorized", context: ["state": stateString, "authorization": authStatus])
            
        case .unsupported:
            print("[BleManager] ⚠️ Bluetooth is not supported on this device")
            centralReady = false
            stopScanning(reason: "central_unsupported")
            updateState(.unavailable)
            notifyBleStatus(false)
            emitDiagnostic("error", "Bluetooth is not supported", context: ["state": stateString])
            
        case .resetting:
            print("[BleManager] 🔄 Bluetooth is resetting...")
            emitDiagnostic("info", "Bluetooth is resetting", context: ["state": stateString])
            
        case .unknown:
            print("[BleManager] ❓ Bluetooth state is unknown")
            emitDiagnostic("info", "Bluetooth state is unknown", context: ["state": stateString])
            
        @unknown default:
            print("[BleManager] ❓ Bluetooth state is unknown (default)")
            emitDiagnostic("warning", "Unknown Bluetooth state", context: ["state": stateString])
        }
    }
    
    public func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral, advertisementData: [String: Any], rssi RSSI: NSNumber) {
        let rssiValue = RSSI.int16Value
        markDiscoveryEvent()
        
        let now = Date()
        
        // Adaptive scanning: track discoveries for density estimation
        recordDiscoveryForDensity(now: now)
        
        // Duplicate advertisement detection - avoid processing identical advertisements
        // This improves performance in dense networks where the same device may be seen many times
        let advertHash = computeAdvertisementHash(peripheral: peripheral, advertisementData: advertisementData, rssi: rssiValue)
        if let cached = recentAdvertisementHashes[peripheral.identifier] {
            // If we've seen this exact advertisement recently, skip processing
            if cached.hash == advertHash && now.timeIntervalSince(cached.timestamp) < 1.0 {
                return
            }
        }
        recentAdvertisementHashes[peripheral.identifier] = (hash: advertHash, timestamp: now)
        
        // Prune old advertisement cache entries periodically
        if recentAdvertisementHashes.count > 100 {
            let cutoff = now.addingTimeInterval(-30.0)
            recentAdvertisementHashes = recentAdvertisementHashes.filter { $0.value.timestamp > cutoff }
        }
        
        // Smart filtering for iOS ↔ Android interoperability
        // Since we scan without a service UUID filter (for Android compatibility),
        // we need to filter discovered peripherals here instead.
        let isConnectable: Bool
        if #available(iOS 13.0, *) {
            isConnectable = (advertisementData[CBAdvertisementDataIsConnectable] as? NSNumber)?.boolValue ?? true
        } else {
            isConnectable = true
        }

        let shouldProcess = shouldProcessDiscoveredPeripheral(
            peripheral: peripheral,
            advertisementData: advertisementData,
            rssi: rssiValue,
            isConnectable: isConnectable,
            now: now
        )
        
        if !shouldProcess {
            return
        }
        
        // Adaptive scanning: early RSSI filtering in dense networks
        if shouldFilterByRssi(rssiValue) {
            if logThrottler.shouldLog(key: "adaptive_rssi_filter", interval: 10) {
                print("[BleManager] Adaptive: filtering weak signal (\(rssiValue)dBm) in dense network (\(estimatedVisiblePeerCount) peers)")
            }
            return
        }
        
        // Adaptive scanning: probabilistic filtering in very dense networks
        if shouldProbabilisticallySkip(peripheral.identifier) {
            return // Silently skip to reduce log spam in dense networks
        }
        
        discoveredPeripherals[peripheral.identifier] = peripheral
        peripheralRSSI[peripheral.identifier] = rssiValue
        
        if discoveryLogTimestamps[peripheral.identifier] == nil || (now.timeIntervalSince(discoveryLogTimestamps[peripheral.identifier]!) > 30) {
            discoveryLogTimestamps[peripheral.identifier] = now
            print("[BleManager] Discovered peripheral: \(peripheral.identifier) RSSI=\(rssiValue) (density: \(estimatedVisiblePeerCount))")
            emitDiagnostic("info", "Discovered BLE peripheral", context: [
                "identifier": peripheral.identifier.uuidString,
                "rssi": rssiValue,
                "connectable": isConnectable,
                "visiblePeers": estimatedVisiblePeerCount
            ])
        }

        let serviceData = (advertisementData[CBAdvertisementDataServiceDataKey] as? [CBUUID: Data])?[SERVICE_UUID]
        let meshMetadata = MeshAdvertisementData.decode(serviceData)
        if let metadata = meshMetadata {
            lastSeenMeshAdvertisements[peripheral.identifier] = MeshObservation(advertisement: metadata, rssi: Int(rssiValue), timestamp: now)
        }
        pruneMeshObservations(now: now)
        meshController.observeAdvertisement(meshMetadata, rssi: Int(rssiValue))

        // When there's no metadata (iOS/Android advertising without service data),
        // still try to connect - metadata will be exchanged via GATT after connection
        let decision: MeshController.MeshDecision
        if meshMetadata == nil {
            // No metadata in advertisement - allow basic connection to exchange info via GATT
            decision = MeshController.MeshDecision(
                intent: .intraCluster,
                reason: "no_metadata_in_advert",
                evictPeerId: nil
            )
        } else {
            decision = meshController.shouldInitiateOutbound(metadata: meshMetadata, rssi: Int(rssiValue))
        }
        
        guard decision.intent != .rejected else {
            if logThrottler.shouldLog(key: "mesh_skip_\(peripheral.identifier.uuidString)", interval: 15) {
                print("[BleManager] Skipping \(peripheral.identifier) due to \(decision.reason)")
            }
            return
        }
        
        // Adaptive scanning: rate limit connection attempts
        // Skip throttling for first-time discoveries with strong signals for faster connection
        let notSeenBefore = lastSeenMeshAdvertisements[peripheral.identifier] == nil
        let notConnected = connections.connectedPeripheral(for: peripheral.identifier) == nil
        let isFirstDiscovery = notSeenBefore && notConnected
        let hasStrongSignal = rssiValue >= -70
        
        if !isFirstDiscovery || !hasStrongSignal {
            if shouldThrottleConnection(to: peripheral.identifier, now: now) {
                if logThrottler.shouldLog(key: "adaptive_throttle_\(peripheral.identifier.uuidString)", interval: 30) {
                    print("[BleManager] Adaptive: throttling connection to \(peripheral.identifier)")
                }
                return
            }
        } else if isFirstDiscovery && hasStrongSignal {
            print("[BleManager] Fast-tracking first discovery with strong signal: \(peripheral.identifier) RSSI=\(rssiValue)")
            emitDiagnostic("info", "Fast-tracking first discovery", context: [
                "identifier": peripheral.identifier.uuidString,
                "rssi": rssiValue
            ])
        }

        let desiredRole: MeshController.MeshRole = (decision.intent == .interCluster) ? .bridge : .member

        if !meshController.connectionBudgetAvailable(), let evictPeerId = decision.evictPeerId {
            self.evictPeer(evictPeerId, reason: decision.reason)
        }

        guard meshController.connectionBudgetAvailable() else {
            if logThrottler.shouldLog(key: "mesh_budget_exhausted_ios", interval: 5) {
                print("[BleManager] Connection budget exhausted, skipping \(peripheral.identifier)")
            }
            return
        }
        
        // Record the connection attempt for rate limiting
        recordConnectionAttempt(to: peripheral.identifier, now: now)

        guard currentConnectionCount() < MAX_CONNECTIONS_PER_DEVICE else {
            if logThrottler.shouldLog(key: "mesh_conn_cap_ios", interval: 10) {
                print("[BleManager] Max connections reached, skipping \(peripheral.identifier)")
            }
            return
        }

        connections.setPendingRole(desiredRole, for: peripheral.identifier)
        attemptConnection(to: peripheral, reason: "discovery", rssi: rssiValue, desiredRole: desiredRole)
        maybeHandleRebalance(reason: "scan")
    }
    
    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        print("[BleManager] Connected to peripheral: \(peripheral.identifier)")
        emitDiagnostic("info", "Connected to BLE peripheral", context: ["identifier": peripheral.identifier.uuidString])
        
        connections.registerPeripheral(peripheral)
        connectionAttemptTimestamps.removeValue(forKey: peripheral.identifier)
        connectionRetryCount.removeValue(forKey: peripheral.identifier) // Reset retry count on successful connection
        
        // Discover services
        peripheral.discoverServices([SERVICE_UUID])
    }
    
    public func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        print("[BleManager] Failed to connect to peripheral: \(error?.localizedDescription ?? "unknown")")
        
        // Increment retry count and calculate backoff
        let retryCount = (connectionRetryCount[peripheral.identifier] ?? 0) + 1
        connectionRetryCount[peripheral.identifier] = retryCount
        
        emitDiagnostic("error", "Failed to connect to BLE peripheral", context: [
            "identifier": peripheral.identifier.uuidString,
            "error": error?.localizedDescription ?? "unknown",
            "retryCount": retryCount
        ])
        connectionAttemptTimestamps.removeValue(forKey: peripheral.identifier)
        _ = connections.consumePendingRole(for: peripheral.identifier)
        
        // Give up after max retries
        guard retryCount <= MAX_CONNECTION_RETRIES else {
            print("[BleManager] Max retries (\(MAX_CONNECTION_RETRIES)) exceeded for \(peripheral.identifier), giving up")
            emitDiagnostic("warning", "Max connection retries exceeded", context: [
                "identifier": peripheral.identifier.uuidString,
                "retryCount": retryCount
            ])
            connectionRetryCount.removeValue(forKey: peripheral.identifier)
            return
        }
        
        // Exponential backoff: 5s, 10s, 20s, 40s, 60s (capped)
        let backoffInterval = min(MAX_RECONNECT_INTERVAL, MIN_RECONNECT_INTERVAL * pow(2.0, Double(retryCount - 1)))
        
        DispatchQueue.main.asyncAfter(deadline: .now() + backoffInterval) { [weak self] in
            guard let self = self, self.state == .running else { return }
            self.attemptConnection(to: peripheral, reason: "retry_fail")
        }
    }
    
    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        let wasConnected = connections.connectedPeripheral(for: peripheral.identifier) != nil
        _ = connections.removePeripheral(peripheral.identifier)

        // Drop the handshake state with the link. Keeping it would let a
        // reconnect pair a device id read from the previous connection with an
        // identity read from this one — a cross-connection join the binding
        // was never asked to reason about — and would leave the announce
        // suppressed by `announcedPeripherals` after a re-pair.
        advertisedDeviceIds.removeValue(forKey: peripheral.identifier)
        verifiedPeerAddresses.removeValue(forKey: peripheral.identifier)
        verifiedPeerIdentities.removeValue(forKey: peripheral.identifier)
        announcedPeripherals.remove(peripheral.identifier)
        if logThrottler.shouldLog(key: "disconnect_\(peripheral.identifier.uuidString)", interval: 10) {
            let errorDescription = (error as NSError?)?.localizedDescription ?? "none"
            print("[BleManager] Disconnected from \(peripheral.identifier) error=\(errorDescription)")
            emitDiagnostic("warning", "Peripheral disconnected", context: [
                "identifier": peripheral.identifier.uuidString,
                "error": errorDescription,
                "willAttemptReconnect": wasConnected
            ])
        }
        
        // Don't remove from discovered list - keep trying to reconnect
        // Only remove RSSI if it's a permanent error
        if let error = error as? CBError, error.code == .connectionTimeout {
            peripheralRSSI.removeValue(forKey: peripheral.identifier)
        }
        
        // Try to reconnect if we were connected and it's not a permanent error
        if wasConnected {
            if let error = error as? CBError {
                // Don't reconnect on permanent errors
                var isPermanentError = error.code == .connectionTimeout
                if #available(iOS 13.4, *) {
                    isPermanentError = isPermanentError || error.code == .peerRemovedPairingInformation
                }
                if isPermanentError {
                    // Permanent error - notify peer lost
                    if let deviceId = connections.peripheralDeviceId(for: peripheral.identifier) {
                        notifyBlePeerLost(deviceId: deviceId)
                        meshController.registerDisconnection(peerId: deviceId)
                        refreshSelfMetrics()
                        connections.removeConnectionRole(for: deviceId)
                        connections.removePeripheralDeviceId(for: peripheral.identifier)
                        connections.removeCentralDeviceId(for: peripheral.identifier)
                        DispatchQueue.main.async {
                            self.refreshAdvertising(reason: "disconnect")
                        }
                        self.maybeHandleRebalance(reason: "disconnect")
                    }
                    return
                }
            }
            
            // Attempt reconnection with exponential backoff
            let retryCount = (connectionRetryCount[peripheral.identifier] ?? 0) + 1
            connectionRetryCount[peripheral.identifier] = retryCount
            
            // Give up after max retries
            guard retryCount <= MAX_CONNECTION_RETRIES else {
                print("[BleManager] Max retries (\(MAX_CONNECTION_RETRIES)) exceeded for \(peripheral.identifier) on disconnect, giving up")
                connectionRetryCount.removeValue(forKey: peripheral.identifier)
                // Notify peer lost since we're giving up
                if let deviceId = connections.peripheralDeviceId(for: peripheral.identifier) {
                    notifyBlePeerLost(deviceId: deviceId)
                    meshController.registerDisconnection(peerId: deviceId)
                    refreshSelfMetrics()
                    connections.removeConnectionRole(for: deviceId)
                    connections.removePeripheralDeviceId(for: peripheral.identifier)
                    connections.removeCentralDeviceId(for: peripheral.identifier)
                    DispatchQueue.main.async {
                        self.refreshAdvertising(reason: "disconnect_max_retries")
                    }
                    maybeHandleRebalance(reason: "disconnect_max_retries")
                }
                return
            }
            
            let backoffInterval = min(MAX_RECONNECT_INTERVAL, MIN_RECONNECT_INTERVAL * pow(2.0, Double(retryCount - 1)))
            
            DispatchQueue.main.asyncAfter(deadline: .now() + backoffInterval) { [weak self] in
                guard let self = self else { return }
                if self.state == .running && self.discoveredPeripherals[peripheral.identifier] != nil {
                    self.attemptConnection(to: peripheral, reason: "retry_disconnect")
                }
            }
        } else {
            // Wasn't connected, just notify if we had device ID
            if let deviceId = connections.peripheralDeviceId(for: peripheral.identifier) {
                notifyBlePeerLost(deviceId: deviceId)
                meshController.registerDisconnection(peerId: deviceId)
                refreshSelfMetrics()
                connections.removeConnectionRole(for: deviceId)
                DispatchQueue.main.async {
                    self.refreshAdvertising(reason: "disconnect")
                }
                maybeHandleRebalance(reason: "disconnect")
            }
        }
        _ = connections.consumePendingRole(for: peripheral.identifier)
    }
}

// MARK: - CBPeripheralDelegate

extension BleManager: CBPeripheralDelegate {
    
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        if let error = error {
            print("[BleManager] Error discovering services: \(error)")
            emitDiagnostic("error", "Error discovering services", context: ["error": error.localizedDescription])
            // Clean up failed connection to free the slot
            centralManager?.cancelPeripheralConnection(peripheral)
            _ = connections.removePeripheral(peripheral.identifier)
            return
        }
        
        guard let services = peripheral.services else { return }
        
        let hasOurService = services.contains { $0.uuid == SERVICE_UUID }
        
        if hasOurService {
            for service in services where service.uuid == SERVICE_UUID {
                peripheral.discoverCharacteristics([MESSAGE_CHAR_UUID, DEVICE_ID_CHAR_UUID, IDENTITY_CHAR_UUID], for: service)
            }
            emitDiagnostic("info", "Discovered BLE services", context: ["peripheral": peripheral.identifier.uuidString])
        } else {
            // Non-mesh device: disconnect and add to negative cache
            print("[BleManager] Service UUID not found on \(peripheral.identifier). Disconnecting non-mesh device.")
            emitDiagnostic("warning", "Offline protocol service not found", context: [
                "identifier": peripheral.identifier.uuidString,
                "serviceCount": services.count
            ])
            verifiedNonMeshDevices[peripheral.identifier] = Date()
            centralManager?.cancelPeripheralConnection(peripheral)
            _ = connections.removePeripheral(peripheral.identifier)
            discoveredPeripherals.removeValue(forKey: peripheral.identifier)
        }
    }
    
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        if let error = error {
            print("[BleManager] Error discovering characteristics: \(error)")
            emitDiagnostic("error", "Error discovering characteristics", context: ["error": error.localizedDescription])
            return
        }
        
        guard let characteristics = service.characteristics else { return }

        // Both reads are issued here and complete in either order; the peer is
        // announced by whichever one lands second, through
        // `completePeerHandshake`. Neither is sufficient alone — see that
        // method for why the identity is no longer optional.
        for characteristic in characteristics {
            if characteristic.uuid == MESSAGE_CHAR_UUID {
                // Enable notifications for message characteristic
                peripheral.setNotifyValue(true, for: characteristic)
                print("[BleManager] Enabled notifications for message characteristic")
                emitDiagnostic("info", "Enabled notifications for message characteristic", context: ["peripheral": peripheral.identifier.uuidString])
            } else if characteristic.uuid == DEVICE_ID_CHAR_UUID {
                // Read device ID
                peripheral.readValue(for: characteristic)
            } else if characteristic.uuid == IDENTITY_CHAR_UUID {
                // Read identity (public key + signature)
                peripheral.readValue(for: characteristic)
            }
        }

        // A peer missing either characteristic can never complete the
        // handshake: `completePeerHandshake` waits for both halves and no
        // further read will ever be issued for the absent one. Reject here
        // rather than holding a connection open on a join that cannot finish
        // — both arms are required, so both are checked.
        if !characteristics.contains(where: { $0.uuid == DEVICE_ID_CHAR_UUID }) {
            rejectPeerHandshake(
                for: peripheral,
                reason: PeerIdentityBinding.Reason.missingDeviceId,
                detail: "peer exposes no device id characteristic"
            )
        } else if !characteristics.contains(where: { $0.uuid == IDENTITY_CHAR_UUID }) {
            rejectPeerHandshake(
                for: peripheral,
                reason: PeerIdentityBinding.Reason.unverifiedIdentity,
                detail: "peer exposes no identity characteristic"
            )
        }
    }
    
    public func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        if let error = error {
            print("[BleManager] Error reading characteristic: \(error)")
            emitDiagnostic("error", "Error reading characteristic", context: ["error": error.localizedDescription])
            // Either read failing means the handshake can never complete: the
            // device id and the identity are both required, and neither is
            // re-issued. Reject explicitly so the link is dropped and the
            // failure is named, instead of leaving a connected peripheral that
            // is silently never announced.
            if characteristic.uuid == DEVICE_ID_CHAR_UUID {
                rejectPeerHandshake(
                    for: peripheral,
                    reason: PeerIdentityBinding.Reason.missingDeviceId,
                    detail: "device id read failed: \(error.localizedDescription)"
                )
            } else if characteristic.uuid == IDENTITY_CHAR_UUID {
                rejectPeerHandshake(
                    for: peripheral,
                    reason: PeerIdentityBinding.Reason.unverifiedIdentity,
                    detail: "identity read failed: \(error.localizedDescription)"
                )
            }
            return
        }

        guard let data = characteristic.value else { return }

        if characteristic.uuid == DEVICE_ID_CHAR_UUID {
            // Record this half of the handshake. Nothing is announced here —
            // the advertised string is an unproven claim until the identity
            // read binds it to a key. `completePeerHandshake` is the only
            // place that acts on it.
            if let deviceId = String(data: data, encoding: .utf8) {
                print("[BleManager] Read advertised device id for peripheral \(peripheral.identifier): \(deviceId)")
                advertisedDeviceIds[peripheral.identifier] = deviceId
                completePeerHandshake(for: peripheral)
            } else {
                rejectPeerHandshake(
                    for: peripheral,
                    reason: PeerIdentityBinding.Reason.missingDeviceId,
                    detail: "device id characteristic is not valid UTF-8"
                )
            }
        } else if characteristic.uuid == MESSAGE_CHAR_UUID {
            // Handle received message fragment
            handleReceivedData(data, senderId: connections.peripheralDeviceId(for: peripheral.identifier), centralId: peripheral.identifier)
        } else if characteristic.uuid == IDENTITY_CHAR_UUID {
            // Handle received identity data
            handleReceivedIdentity(data, for: peripheral)
        }
    }

    /// Announces a peer, once both halves of its handshake are in and agree.
    ///
    /// This is the only place `blePeerDiscovered` is called from the central
    /// role, and it runs at most once per peripheral. Before it, a connected
    /// peripheral is invisible to the protocol layer: it has no entry in the
    /// Rust `peers` map, no MTU, no mesh registration, and no route.
    ///
    /// # Why the identity is no longer best-effort
    ///
    /// The announced id is load-bearing all the way down — it keys `peers` and
    /// `peer_mtus`, it is what the app puts in `Message.recipient`, and it
    /// comes back as `transport_peer_id` on every frame this peer sends, where
    /// the core matches it against `Message.sender`. Announcing a peer under a
    /// string it merely claimed is what let anyone advertise any name; and
    /// since the sender a peer stamps is its derived address, a claim that is
    /// anything else also makes its own control frames unroutable.
    ///
    /// So a peer that will not or cannot prove its id is not surfaced at all.
    /// That is a real availability cost — an older build advertising its
    /// profile becomes invisible rather than degraded — and it is the intended
    /// cutover: its control frames would be rejected by the core anyway.
    private func completePeerHandshake(for peripheral: CBPeripheral) {
        guard !announcedPeripherals.contains(peripheral.identifier) else { return }

        let advertised = advertisedDeviceIds[peripheral.identifier]
        let derived = verifiedPeerAddresses[peripheral.identifier]

        // Still waiting on the other read — not a failure, just incomplete.
        // Waiting is only ever bounded because every way a read can fail to
        // arrive rejects instead of leaving this pending: a characteristic
        // absent from the service (checked for BOTH uuids in
        // `didDiscoverCharacteristicsFor`), a read that errors, and a device
        // id that is not UTF-8. Adding a third required half means adding its
        // absence check there too, or this guard waits forever.
        guard advertised != nil, derived != nil else { return }

        let outcome = PeerIdentityBinding.resolve(
            advertisedDeviceId: advertised,
            derivedAddress: derived
        )
        guard case let .verified(peerId) = outcome else {
            if case let .rejected(reason) = outcome {
                rejectPeerHandshake(
                    for: peripheral,
                    reason: reason,
                    detail: "advertised=\(advertised ?? "nil") derived=\(derived ?? "nil")"
                )
            }
            return
        }

        announcedPeripherals.insert(peripheral.identifier)
        print("[BleManager] ✅ Verified peer \(peerId) for peripheral \(peripheral.identifier)")

        // `peerId` is the derived address, not the advertised string. They are
        // equal here by construction — the binding rejects anything else — but
        // every downstream key is taken from the proof so a future relaxation
        // of that comparison cannot leak an unproven id into the registry, the
        // MTU map, or the protocol layer.
        connections.setPeripheralDeviceId(peerId, for: peripheral.identifier)
        connections.setCentralDeviceId(peerId, for: peripheral.identifier)
        connectionAttemptTimestamps.removeValue(forKey: peripheral.identifier)

        // ORDERING INVARIANT — DO NOT REORDER.
        //
        // `bleSetPeerMtu` MUST run before `blePeerDiscovered`.
        // This is the entire point of commit ac8cce8: any
        // fragmenting send that lands between announcing the
        // peer to the protocol layer and flushing the MTU
        // into the Rust transport will key-miss `peer_mtus`,
        // fall back to the 185-byte floor, and silently
        // waste ~60% of the negotiated BLE 5 bandwidth for
        // every fragment in that window. The Rust side pins
        // this from its end via
        // `test_ble_golden_path_handshake_never_falls_back`
        // and the `ble_fragment_fallback_count` telemetry
        // counter, which fires the moment a registered peer
        // has to fall back.
        //
        // The identity cross-check moved BOTH of these later — they used to
        // run in the DEVICE_ID read handler, and now run here, once the peer
        // has proved its id. Their order relative to each other is unchanged
        // and must stay that way: the invariant is that no fragmenting send
        // can observe an announced peer that has no MTU on file, and the
        // window between these two calls is still the only place that could
        // happen.
        //
        // CoreBluetooth performs MTU negotiation automatically
        // on connect and exposes the already header-adjusted
        // max-write length as a stable property — we just
        // read it once, here, at the moment the peer becomes real to us.
        let maxPayload = peripheral.maximumWriteValueLength(for: .withoutResponse)
        let rssi = peripheralRSSI[peripheral.identifier] ?? -60

        // Both protocol calls in ONE serial block, off the main thread.
        // Every caller of this method is a CoreBluetooth delegate, so making
        // these inline meant taking the core protocol mutex on the main thread.
        // Keeping them in a single block also makes the ordering invariant
        // below structural rather than a matter of statement order in a method
        // that does plenty of unrelated local bookkeeping.
        onProtocolQueue { [weak self] in
            guard let self = self else { return }
            do {
                try self.protocolInstance.bleSetPeerMtu(peerId: peerId, maxPayload: UInt32(maxPayload))
                self.emitDiagnostic("info", "BLE per-peer MTU flushed to Rust", context: [
                    "deviceId": peerId,
                    "peripheral": peripheral.identifier.uuidString,
                    "maxPayload": maxPayload,
                ])
            } catch {
                // A UniFFI throw here (lock poisoning is the only
                // expected cause) is categorically different from
                // the ordering-invariant regression that
                // `ble_fragment_fallback_count` is designed to
                // surface: the peer will still be announced below
                // and, absent an MTU entry, the first fragmenting
                // send will tick the fallback counter. Tag the
                // diagnostic with `cause: "uniffi_throw"` and emit
                // at error level so dashboards can filter these
                // out of the ordering-invariant alarm.
                self.emitDiagnostic("error", "bleSetPeerMtu failed", context: [
                    "deviceId": peerId,
                    "error": error.localizedDescription,
                    "cause": "uniffi_throw",
                ])
            }

            // ORDERING INVARIANT: this line MUST come AFTER
            // `bleSetPeerMtu` above. See comment block preceding
            // the MTU flush. If you are tempted to move this
            // earlier "for clarity", don't.
            try? self.protocolInstance.blePeerDiscovered(peerId: peerId, rssi: rssi)
        }

        let role = connections.consumePendingRole(for: peripheral.identifier) ?? connections.connectionRole(for: peerId) ?? .member
        meshController.registerConnection(peerId: peerId, role: role)
        connections.setConnectionRole(role, for: peerId)
        meshController.markPeerActive(peerId)
        meshController.markPeerActive(self.deviceId)
        refreshSelfMetrics()
        meshController.updatePeerMetrics(peerId: peerId, metrics: MeshController.PeerMetrics(rssi: Int(rssi)))
        DispatchQueue.main.async {
            self.refreshAdvertising(reason: "membership_change")
        }

        //  Process any pending fragments for this device immediately
        // This is essential for Android → iOS messages that were queued while waiting for device ID
        // When Android writes to iOS, iOS receives the write with Android's central UUID
        // When iOS connects to Android to read device ID, it uses Android's peripheral UUID
        // Both UUIDs should be the same, but we process fragments for both to be safe
        print("[BleManager] 🔄 Processing pending fragments for device ID: \(peerId), peripheral: \(peripheral.identifier)")
        processPendingFragments(for: peripheral.identifier, deviceId: peerId)

        //  Also check all pending fragments and process any that match this device ID
        // This handles the case where Android wrote to iOS before iOS connected to Android
        // The central UUID (from write) might be the same as peripheral UUID, but we check all
        let pendingCentralIds: [UUID] = self.inboundFragments.pendingIds()
        for centralId in pendingCentralIds {
            // Check if this central ID now maps to the device ID we just resolved
            let centralDeviceId = self.connections.centralDeviceId(for: centralId)
            let peripheralDeviceId = self.connections.peripheralDeviceId(for: centralId)
            if centralDeviceId == peerId || peripheralDeviceId == peerId {
                print("[BleManager] 🔄 Processing pending fragments for central \(centralId) (now maps to device \(peerId))")
                processPendingFragments(for: centralId, deviceId: peerId)
            }
        }

        // The recipient -> peripheral mapping and MESSAGE characteristic
        // now exist, so flush any OUTBOUND fragments parked while this
        // connection was still forming. drainAndSendFragments previously
        // ran only on connect / onFragmentsAvailable, so the FIRST message
        // a user sent into a still-connecting peer (already popped out of
        // the Rust queue and parked in pendingOutboundFragments) had
        // nothing to re-trigger it and stalled until a later fragment
        // event or the 30s expiry — the iOS->Android first-message stall.
        self.drainAndSendFragments()
    }

    /// Abandons a handshake that can never complete, and drops the link.
    ///
    /// Called for a failed or absent read on either characteristic, a
    /// non-UTF-8 device id, and — the case this all exists for — an advertised
    /// id that the peer's own key does not derive to. Nothing about the peer
    /// reaches the protocol layer: no announce has happened yet, by
    /// construction, so there is nothing to retract.
    ///
    /// The connection is closed rather than left open unannounced. A live link
    /// we will never use still costs a connection slot on both ends, and
    /// dropping it lets the peer's own reconnect logic retry — which is what
    /// recovers the case where the peer simply had not finished initializing
    /// its identity yet.
    private func rejectPeerHandshake(for peripheral: CBPeripheral, reason: String, detail: String) {
        // Clear the half-state so a reconnect starts a fresh join rather than
        // pairing a stale device id with a newly read identity.
        advertisedDeviceIds.removeValue(forKey: peripheral.identifier)
        verifiedPeerAddresses.removeValue(forKey: peripheral.identifier)
        verifiedPeerIdentities.removeValue(forKey: peripheral.identifier)

        print("[BleManager] ⚠️ Refusing peer \(peripheral.identifier): \(reason) (\(detail))")
        emitDiagnostic("warning", "BLE peer refused: unproven identity", context: [
            "peripheral": peripheral.identifier.uuidString,
            "reason": reason,
            "detail": detail,
        ])

        centralManager?.cancelPeripheralConnection(peripheral)
        _ = connections.removePeripheral(peripheral.identifier)
    }


    /// Verifies a peer's signed identity and records the address it proves.
    ///
    /// This is the half of the handshake that establishes *what the peer can
    /// prove*; `completePeerHandshake` decides whether that matches what it
    /// claimed. Every failure below leaves `verifiedPeerAddresses` unset, which
    /// is what makes the binding refuse the peer — so an unverifiable identity
    /// and an absent one are the same thing to the gate, deliberately.
    private func handleReceivedIdentity(_ data: Data, for peripheral: CBPeripheral) {
        guard let signedIdentity = SignedIdentityData.decode(data) else {
            print("[BleManager] Failed to decode identity data from \(peripheral.identifier)")
            rejectPeerHandshake(
                for: peripheral,
                reason: PeerIdentityBinding.Reason.unverifiedIdentity,
                detail: "identity blob did not decode"
            )
            return
        }

        // Verify the signature
        do {
            let isValid = try protocolInstance.verifySignature(
                publicKey: [UInt8](signedIdentity.publicKey),
                data: [UInt8](signedIdentity.advertisementData),
                signature: [UInt8](signedIdentity.signature)
            )

            if isValid {
                // Store the verified identity
                verifiedPeerIdentities[peripheral.identifier] = signedIdentity

                // Derive the peer's address in Rust — the single implementation
                // shared by every platform. `decode` already guarantees a
                // 32-byte key, so the throw is unreachable here.
                guard let derivedAddress = try? deriveAddress(publicKey: [UInt8](signedIdentity.publicKey)) else {
                    print("[BleManager] Failed to derive address for \(peripheral.identifier)")
                    rejectPeerHandshake(
                        for: peripheral,
                        reason: PeerIdentityBinding.Reason.unverifiedIdentity,
                        detail: "address derivation failed"
                    )
                    return
                }
                print("[BleManager] ✅ Verified peer identity: \(derivedAddress) for \(peripheral.identifier)")
                emitDiagnostic("info", "Verified peer identity", context: [
                    "peripheral": peripheral.identifier.uuidString,
                    "derivedAddress": derivedAddress
                ])

                // Record the proof and join it against the advertised id. The
                // route is seeded there rather than here: learning a route to
                // a peer the protocol layer has not been told about put an
                // address in the routing table that `peers` had no entry for.
                verifiedPeerAddresses[peripheral.identifier] = derivedAddress
                completePeerHandshake(for: peripheral)
            } else {
                print("[BleManager] ⚠️ Invalid signature for peer \(peripheral.identifier)")
                rejectPeerHandshake(
                    for: peripheral,
                    reason: PeerIdentityBinding.Reason.unverifiedIdentity,
                    detail: "identity signature did not verify"
                )
            }
        } catch {
            print("[BleManager] Failed to verify signature: \(error)")
            rejectPeerHandshake(
                for: peripheral,
                reason: PeerIdentityBinding.Reason.unverifiedIdentity,
                detail: "signature verification threw: \(error.localizedDescription)"
            )
        }
    }
    
    public func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        if let error = error {
            print("[BleManager] Error writing characteristic: \(error)")
            emitDiagnostic("error", "Error writing characteristic", context: ["error": error.localizedDescription])
        }
    }
    
    /// Flow-control signal: the BLE write buffer has drained for this peripheral.
    /// Resume sending queued fragments instead of waiting for a timer tick.
    public func peripheralIsReady(toSendWriteWithoutResponse peripheral: CBPeripheral) {
        guard let recipientId = connections.peripheralDeviceId(for: peripheral.identifier) else { return }
        if logThrottler.shouldLog(key: "flow_ready_\(recipientId)", interval: 1.0) {
            emitDiagnostic("debug", "BLE write buffer drained, resuming sends", context: ["recipientId": recipientId])
        }
        drainAndSendFragments()
    }
}

// MARK: - CBPeripheralManagerDelegate

extension BleManager: CBPeripheralManagerDelegate {
    
    /// State restoration for the peripheral (GATT server) side.
    /// Re-registers services if needed after app relaunch.
    public func peripheralManager(_ peripheral: CBPeripheralManager, willRestoreState dict: [String: Any]) {
        print("[BleManager] Restoring peripheral manager state")
        emitDiagnostic("info", "Peripheral manager restoring state", context: [
            "keys": Array(dict.keys)
        ])
        
        // If services were restored, mark GATT as ready
        if let services = dict[CBPeripheralManagerRestoredStateServicesKey] as? [CBMutableService] {
            let hasOurService = services.contains { $0.uuid == SERVICE_UUID }
            if hasOurService {
                isGattServiceReady = true
                print("[BleManager] GATT service restored from state restoration")
                emitDiagnostic("info", "GATT service restored")
            }
        }
    }
    
    public func peripheralManagerDidUpdateState(_ peripheral: CBPeripheralManager) {
        let stateString: String
        var authStatus = "unknown"
        
        if #available(iOS 13.1, *) {
            let authorization = CBPeripheralManager.authorization
            switch authorization {
            case .notDetermined:
                authStatus = "notDetermined"
            case .restricted:
                authStatus = "restricted"
            case .denied:
                authStatus = "denied"
            case .allowedAlways:
                authStatus = "allowedAlways"
            @unknown default:
                authStatus = "unknown"
            }
        }
        
        switch peripheral.state {
        case .unknown:
            stateString = "unknown"
        case .resetting:
            stateString = "resetting"
        case .unsupported:
            stateString = "unsupported"
        case .unauthorized:
            stateString = "unauthorized"
        case .poweredOff:
            stateString = "poweredOff"
        case .poweredOn:
            stateString = "poweredOn"
        @unknown default:
            stateString = "unknown"
        }
        
        print("[BleManager] Peripheral state: \(stateString), authorization: \(authStatus)")
        emitDiagnostic("info", "Peripheral manager state changed", context: [
            "state": stateString,
            "stateRaw": peripheral.state.rawValue,
            "authorization": authStatus
        ])
        
        switch peripheral.state {
        case .poweredOn:
            // Check authorization status on iOS 13.1+
            if #available(iOS 13.1, *) {
                let authorization = CBPeripheralManager.authorization
                switch authorization {
                case .denied, .restricted:
                    print("[BleManager] ⚠️ Bluetooth peripheral permission denied or restricted")
                    emitDiagnostic("error", "Bluetooth peripheral permission denied", context: ["authorization": authStatus])
                    peripheralReady = false
                    updateState(.unavailable)
                    notifyBleStatus(false)
                    return
                    
                case .notDetermined:
                    print("[BleManager] 🔔 Bluetooth peripheral permission not determined yet, waiting for user response...")
                    emitDiagnostic("info", "Waiting for Bluetooth peripheral permission", context: ["authorization": authStatus])
                    // Permission prompt should be showing now
                    // Will get called again when user responds
                    return
                    
                case .allowedAlways:
                    print("[BleManager] ✅ Bluetooth peripheral permission granted")
                    emitDiagnostic("info", "Bluetooth peripheral permission granted", context: ["authorization": authStatus])
                    
                @unknown default:
                    print("[BleManager] ⚠️ Unknown peripheral authorization state")
                    emitDiagnostic("warning", "Unknown peripheral authorization state", context: ["authorization": authStatus])
                }
            }
            
            peripheralReady = true
            startAdvertising(reason: "state_powered_on")
            emitDiagnostic("info", "Peripheral manager powered on and ready")
            
            // If both central and peripheral are ready, mark as running
            if centralReady && state == .starting {
                updateState(.running)
                print("[BleManager] ✅ BLE Manager ready (peripheral) - dispatching bleStatusChanged(true)")
                // See the central-side note: dispatched, not completed.
                notifyBleStatus(true)
                emitDiagnostic("info", "Dispatched protocol.bleStatusChanged(true) from peripheral")
            }
            
        case .poweredOff:
            print("[BleManager] ⚠️ Bluetooth peripheral is powered off")
            peripheralReady = false
            stopAdvertising()
            updateState(.unavailable)
            notifyBleStatus(false)
            emitDiagnostic("warning", "Bluetooth peripheral is powered off", context: ["state": stateString])
            
        case .unauthorized:
            print("[BleManager] ⚠️ Bluetooth peripheral is unauthorized")
            peripheralReady = false
            stopAdvertising()
            updateState(.unavailable)
            notifyBleStatus(false)
            emitDiagnostic("error", "Bluetooth peripheral is unauthorized", context: ["state": stateString, "authorization": authStatus])
            
        case .unsupported:
            print("[BleManager] ⚠️ Bluetooth peripheral is not supported on this device")
            peripheralReady = false
            stopAdvertising()
            updateState(.unavailable)
            notifyBleStatus(false)
            emitDiagnostic("error", "Bluetooth peripheral is not supported", context: ["state": stateString])
            
        case .resetting:
            print("[BleManager] 🔄 Bluetooth peripheral is resetting...")
            emitDiagnostic("info", "Bluetooth peripheral is resetting", context: ["state": stateString])
            
        case .unknown:
            print("[BleManager] ❓ Bluetooth peripheral state is unknown")
            emitDiagnostic("info", "Bluetooth peripheral state is unknown", context: ["state": stateString])
            
        @unknown default:
            print("[BleManager] ❓ Bluetooth peripheral state is unknown (default)")
            emitDiagnostic("warning", "Unknown Bluetooth peripheral state", context: ["state": stateString])
        }
    }
    
    public func peripheralManagerDidStartAdvertising(_ peripheral: CBPeripheralManager, error: Error?) {
        if let error = error {
            print("[BleManager] Error starting advertising: \(error)")
            emitDiagnostic("error", "Error starting BLE advertising", context: ["error": error.localizedDescription])
        } else {
            print("[BleManager] Advertising started successfully")
            emitDiagnostic("info", "BLE advertising started successfully")
        }
    }
    
    public func peripheralManager(_ peripheral: CBPeripheralManager, didAdd service: CBService, error: Error?) {
        if let error = error {
            print("[BleManager] ❌ Error adding GATT service: \(error)")
            emitDiagnostic("error", "Error adding GATT service", context: [
                "error": error.localizedDescription,
                "serviceUUID": service.uuid.uuidString
            ])
            isGattServiceReady = false
            return
        }
        
        print("[BleManager] ✅ GATT service added successfully: \(service.uuid)")
        emitDiagnostic("info", "GATT service registered successfully", context: [
            "serviceUUID": service.uuid.uuidString
        ])
        
        isGattServiceReady = true
        
        // Start advertising now that the service is ready
        if pendingAdvertiseAfterServiceReady {
            pendingAdvertiseAfterServiceReady = false
            print("[BleManager] 📡 Starting deferred advertising after GATT service ready")
            startAdvertising(reason: "gatt_service_ready")
        }
    }
    
    public func peripheralManager(_ peripheral: CBPeripheralManager, didReceiveWrite requests: [CBATTRequest]) {
        for request in requests {
            print("[BleManager] 📨 GATT WRITE REQUEST from \(request.central.identifier), char: \(request.characteristic.uuid), size: \(request.value?.count ?? 0)")
            emitDiagnostic("info", "GATT write request received", context: [
                "centralId": request.central.identifier.uuidString,
                "characteristicUuid": request.characteristic.uuid.uuidString,
                "dataSize": request.value?.count ?? 0
            ])
            
            if request.characteristic.uuid == MESSAGE_CHAR_UUID, let value = request.value {
                print("[BleManager] 📥 MESSAGE CHARACTERISTIC WRITE from \(request.central.identifier), processing...")
                let senderId = connections.centralDeviceId(for: request.central.identifier) ?? connections.peripheralDeviceId(for: request.central.identifier)
                
                //  Ensure device ID resolution happens immediately and fragments are processed
                // When Android sends to iOS, iOS might not have Android's device ID yet
                // We must queue the fragment AND aggressively try to resolve the device ID
                if senderId == nil {
                    if logThrottler.shouldLog(key: "missing_sender_\(request.central.identifier.uuidString)", interval: 10) {
                        print("[BleManager] ⚠️ Received write without known sender for central \(request.central.identifier) - will queue and resolve device ID")
                        emitDiagnostic("warning", "Received BLE fragment without sender ID - resolving", context: [
                            "central": request.central.identifier.uuidString,
                            "length": value.count
                        ])
                    }
                    // Aggressively try to resolve device ID - this is impo for Android → iOS messages
                    ensureDeviceId(for: request.central.identifier)
                    // Queue fragment to be processed once device ID is resolved
                    // handleReceivedData will queue it if senderId is nil
                }
                
                // Process the fragment (will queue if senderId is nil, process immediately if known)
                handleReceivedData(value, senderId: senderId, centralId: request.central.identifier)
            } else {
                print("[BleManager] ❌ Unknown characteristic write: \(request.characteristic.uuid)")
            }
            
            // Respond to write request
            peripheral.respond(to: request, withResult: .success)
            print("[BleManager] ✅ Sent success response to \(request.central.identifier)")
        }
    }
    
    //  When a central subscribes to notifications, try to read its device ID
    // This helps resolve device IDs for Android devices that wrote to iOS before iOS connected to them
    public func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didSubscribeTo characteristic: CBCharacteristic) {
        // If we don't have the device ID for this central yet, try to read it
        if connections.centralDeviceId(for: central.identifier) == nil && 
           connections.peripheralDeviceId(for: central.identifier) == nil {
            print("[BleManager] Central subscribed but device ID unknown - attempting to resolve")
            ensureDeviceId(for: central.identifier)
        }
        // When central subscribes, try to get device ID if we don't have it
        let observation = lastSeenMeshAdvertisements[central.identifier]
        let decision = meshController.shouldAcceptInboundConnection(
            remoteId: connections.peripheralDeviceId(for: central.identifier),
            metadata: observation?.advertisement,
            rssi: observation?.rssi
        )
        if let evictPeerId = decision.evictPeerId {
            self.evictPeer(evictPeerId, reason: "inbound_swap")
        }
        guard decision.intent != .rejected else {
            emitDiagnostic("info", "Rejecting inbound subscription", context: [
                "central": central.identifier.uuidString,
                "reason": decision.reason
            ])
            return
        }
        guard meshController.connectionBudgetAvailable() || decision.evictPeerId != nil else {
            emitDiagnostic("info", "Inbound rejected due to budget", context: [
                "central": central.identifier.uuidString
            ])
            return
        }
        guard currentConnectionCount() < MAX_CONNECTIONS_PER_DEVICE else {
            emitDiagnostic("info", "Inbound rejected due to device connection cap", context: [
                "central": central.identifier.uuidString
            ])
            return
        }
        
        // Retain this central so we can NOTIFY it (peripheral-role egress) and so it
        // counts toward the connection budget. Stored under `notifyLock` because the
        // fragment drain tests notify-reachability off the main queue.
        notifyLock.lock()
        subscribedCentralsById[central.identifier] = central
        notifyLock.unlock()
        print("[BleManager] Central subscribed: \(central.identifier)")
        emitDiagnostic("info", "Central subscribed to characteristic", context: [
            "central": central.identifier.uuidString,
            "totalSubscribed": subscribedCentralCount()
        ])

        if connections.centralDeviceId(for: central.identifier) == nil && connections.peripheralDeviceId(for: central.identifier) == nil {
            ensureDeviceId(for: central.identifier)
        } else if let deviceId = connections.peripheralDeviceId(for: central.identifier) {
            connections.setCentralDeviceId(deviceId, for: central.identifier)
            // Process any pending fragments
            processPendingFragments(for: central.identifier, deviceId: deviceId)
        }
        maybeHandleRebalance(reason: "inbound")
    }
    
    public func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didUnsubscribeFrom characteristic: CBCharacteristic) {
        print("[BleManager] Central unsubscribed from characteristic: \(characteristic.uuid)")
        let deviceId = connections.centralDeviceId(for: central.identifier)
            ?? connections.peripheralDeviceId(for: central.identifier)
        notifyLock.lock()
        subscribedCentralsById.removeValue(forKey: central.identifier)
        notifyLock.unlock()
        emitDiagnostic("info", "Central unsubscribed", context: [
            "central": central.identifier.uuidString,
            "characteristic": characteristic.uuid.uuidString,
            "remainingSubscribed": subscribedCentralCount()
        ])
        connections.removeCentralDeviceId(for: central.identifier)
        // The NOTIFY link to this peer is gone: drop anything queued for it and
        // re-report the per-peer MTU (now only a central-write link, if any, binds).
        if let deviceId = deviceId {
            notifyOutbound.removeValue(forKey: deviceId)
            reflectEgressMtu(forDeviceId: deviceId)
        }
    }

    /// CoreBluetooth signals the peripheral transmit queue has space again after a
    /// prior `updateValue` returned false (transmit-queue-full backpressure). Resume
    /// draining queued NOTIFY fragments. Without this, a multi-fragment NOTIFY (an
    /// MLS Welcome) that hits backpressure would stall until the next unrelated
    /// drain. Mirrors the central-side `peripheralIsReady(toSendWriteWithoutResponse:)`.
    // iOS 26.5 SDK renamed the Swift import of this delegate method to
    // `peripheralManagerIsReady(toUpdateSubscribers:)` and marked the old
    // auto-imported name unavailable, so the old spelling is a hard compile error
    // on new Xcode. The underlying ObjC selector is unchanged, so pin it with an
    // explicit `@objc(...)`: that keeps CoreBluetooth dispatching to this method on
    // older SDKs (where the new Swift name is not the protocol requirement) while
    // satisfying the renamed requirement on 26.5+.
    @objc(peripheralManagerIsReadyToUpdateSubscribers:)
    public func peripheralManagerIsReady(toUpdateSubscribers peripheral: CBPeripheralManager) {
        pumpNotifyOutbound()
    }
}

extension BleManager: @unchecked Sendable {}

// MARK: - Bundle Extension for Display Name
extension Bundle {
    var displayName: String? {
        return object(forInfoDictionaryKey: "CFBundleDisplayName") as? String
            ?? object(forInfoDictionaryKey: "CFBundleName") as? String
    }
}

