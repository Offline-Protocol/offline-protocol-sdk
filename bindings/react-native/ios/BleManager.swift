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
    
    private let FRAGMENT_POLL_INTERVAL: TimeInterval = 0.1 // 100ms
    private let MAX_FRAGMENT_SIZE = 185
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
    
    // Fragment polling
    private var fragmentTimer: Timer?
    private let fragmentQueue = DispatchQueue(label: "com.offlineprotocol.ble.fragments")
    
    // Gradient routing cleanup
    private var routingCleanupTimer: Timer?
    private let ROUTING_CLEANUP_INTERVAL: TimeInterval = 30.0
    
    // Pending fragments waiting for device ID
    private var pendingFragments: [UUID: [(Data, Date)]] = [:]
    private let PENDING_FRAGMENT_TIMEOUT: TimeInterval = 5.0
    private var pendingOutboundFragments: [String: [Data]] = [:]
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
    private var subscribedCentrals: Set<UUID> = []
    private var lastMeshAdvertisement: MeshAdvertisementData?
    
    // Metrics
    private var bytesSent: UInt64 = 0
    private var bytesReceived: UInt64 = 0
    private var fragmentsSent: UInt64 = 0
    private var fragmentsReceived: UInt64 = 0

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
    private var connectionMonitor: DispatchSourceTimer?
    private var connectionAttemptTimestamps: [UUID: Date] = [:]
    private let CONNECTION_MONITOR_INTERVAL: TimeInterval = 5.0
    private let MIN_RECONNECT_INTERVAL: TimeInterval = 5.0
    private var scanRestartCount: Int = 0
    private var lastCentralReset: Date?
    private let MAX_CONSECUTIVE_SCAN_RESTARTS = 3
    private let CENTRAL_RESET_BACKOFF: TimeInterval = 45.0
    private let MINIMUM_RSSI_TO_CONNECT: Int16 = -90

    // MARK: - Thread helpers
    @inline(__always)
    private func performOnMain<T>(_ work: () throws -> T) rethrows -> T {
        if Thread.isMainThread {
            return try work()
        }
        return try DispatchQueue.main.sync(execute: work)
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
        
        // Initialize Central Manager (for scanning)
        // This will trigger permission prompt if not yet determined
        print("[BleManager] 📱 Initializing Central Manager...")
        centralManager = CBCentralManager(
            delegate: self,
            queue: nil,
            options: [CBCentralManagerOptionShowPowerAlertKey: true]
        )
        
        // Initialize Peripheral Manager (for advertising)
        // This will trigger permission prompt if not yet determined
        print("[BleManager] 📡 Initializing Peripheral Manager...")
        peripheralManager = CBPeripheralManager(
            delegate: self,
            queue: nil,
            options: [CBPeripheralManagerOptionShowPowerAlertKey: true]
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
        
        // Stop fragment polling
        stopFragmentPolling()
        
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
        pendingFragments.removeAll()
        pendingOutboundFragments.removeAll()
        lastSeenMeshAdvertisements.removeAll()
        pendingAdvertiseRestart?.cancel()
        pendingAdvertiseRestart = nil
        lastAdvertiseRestartAt = nil
        transportStartAt = nil
        subscribedCentrals.removeAll()
        
        // Clean up managers
        centralManager = nil
        peripheralManager = nil
        
        centralReady = false
        peripheralReady = false
        
        updateState(.stopped)
        emitDiagnostic("info", "BLE transport stopped")
    }
    
    public func pause() {
        performOnMain {
            self.pauseUnsafe()
        }
    }
    
    private func pauseUnsafe() {
        // For iOS background mode
        stopScanning(reason: "pause")
        stopFragmentPolling()
    }
    
    public func resume() {
        performOnMain {
            self.resumeUnsafe()
        }
    }
    
    private func resumeUnsafe() {
        // Resume from background
        if state == .running {
            startScanning(reason: "resume")
            startFragmentPolling()
        }
    }
    
    public func getMetrics() -> [String: Any] {
        return [
            "bytes_sent": bytesSent,
            "bytes_received": bytesReceived,
            "fragments_sent": fragmentsSent,
            "fragments_received": fragmentsReceived,
            "connected_peers": connections.connectedPeripheralCount(),
            "discovered_peers": discoveredPeripherals.count
        ]
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
        
        central.scanForPeripherals(
            withServices: [SERVICE_UUID],
            options: [CBCentralManagerScanOptionAllowDuplicatesKey: true]
        )
        isScanning = true
        scanStartDate = Date()
        lastDiscoveryDate = scanStartDate
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
            if idleDuration >= self.SCAN_RESTART_INTERVAL {
                if self.logThrottler.shouldLog(key: "scan_watchdog", interval: self.SCAN_RESTART_INTERVAL) {
                    print("[BleManager] Restarting scan after \(Int(idleDuration))s of inactivity")
                    self.emitDiagnostic("warning", "Restarting BLE scan due to inactivity", context: ["idle_seconds": Int(idleDuration)])
                }
                self.restartScanningDueToInactivity()
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
            options: [CBCentralManagerOptionShowPowerAlertKey: true]
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
            for centralId in self.pendingFragments.keys {
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
            if self.connections.connectedPeripheral(for: peripheral.identifier) != nil {
                return
            }
            let now = Date()
            if let lastAttempt = self.connectionAttemptTimestamps[peripheral.identifier], now.timeIntervalSince(lastAttempt) < self.MIN_RECONNECT_INTERVAL {
                return
            }
            if let effectiveRSSI = rssi ?? self.peripheralRSSI[peripheral.identifier], effectiveRSSI < self.MINIMUM_RSSI_TO_CONNECT {
                if self.logThrottler.shouldLog(key: "rssi_skip_\(peripheral.identifier.uuidString)", interval: 10) {
                    self.emitDiagnostic("debug", "Skipping BLE connect due to weak RSSI", context: [
                        "rssi": effectiveRSSI,
                        "threshold": self.MINIMUM_RSSI_TO_CONNECT,
                        "reason": reason
                    ])
                }
                return
            }
            if self.currentConnectionCount() >= self.MAX_CONNECTIONS_PER_DEVICE {
                if self.logThrottler.shouldLog(key: "mesh_conn_cap_ios", interval: 10) {
                    print("[BleManager] Connection cap reached, not connecting to \(peripheral.identifier)")
                }
                return
            }
            self.connectionAttemptTimestamps[peripheral.identifier] = now
            if let desiredRole = desiredRole {
                self.connections.setPendingRole(desiredRole, for: peripheral.identifier)
            } else if self.connections.pendingRole(for: peripheral.identifier) == nil {
                self.connections.setPendingRole(.member, for: peripheral.identifier)
            }
            peripheral.delegate = self
            if peripheral.state == .connected {
                self.connections.registerPeripheral(peripheral)
                if let service = peripheral.services?.first(where: { $0.uuid == self.SERVICE_UUID }) {
                    peripheral.discoverCharacteristics([self.MESSAGE_CHAR_UUID, self.DEVICE_ID_CHAR_UUID], for: service)
                } else {
                    peripheral.discoverServices([self.SERVICE_UUID])
                }
                return
            }
            self.centralManager?.connect(peripheral, options: nil)
            if self.logThrottler.shouldLog(key: "connect_attempt_\(peripheral.identifier.uuidString)", interval: 10) {
                print("[BleManager] Attempting connection to \(peripheral.identifier) (reason: \(reason))")
                var context: [String: Any] = [
                    "identifier": peripheral.identifier.uuidString,
                    "reason": reason
                ]
                if let rssi = rssi {
                    context["rssi"] = rssi
                }
                self.emitDiagnostic("info", "Connecting to BLE peripheral", context: context)
            }
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
            var candidates = centralManager.retrievePeripherals(withIdentifiers: [centralId])
            if candidates.isEmpty {
                let connected = centralManager.retrieveConnectedPeripherals(withServices: [self.SERVICE_UUID])
                candidates = connected.filter { $0.identifier == centralId }
            }
            guard let peripheral = candidates.first else {
                if self.logThrottler.shouldLog(key: "missing_peripheral_\(centralId.uuidString)", interval: 15) {
                    print("[BleManager] Unable to retrieve peripheral for central \(centralId)")
                    self.emitDiagnostic("debug", "Unable to retrieve peripheral for central", context: ["central": centralId.uuidString])
                }
                return
            }
            self.discoveredPeripherals[peripheral.identifier] = peripheral
            self.attemptConnection(to: peripheral, reason: "ensure_device_id")
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
        
        setupGattServer()
        
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
    
    private func setupGattServer() {
        guard let peripheral = peripheralManager else { return }
        if messageCharacteristic != nil && deviceIdCharacteristic != nil {
            return
        }
        
        // Create message characteristic (write without response + notify)
        messageCharacteristic = CBMutableCharacteristic(
            type: MESSAGE_CHAR_UUID,
            properties: [.writeWithoutResponse, .notify],
            value: nil,
            permissions: [.writeable]
        )
        
        // Create device ID characteristic (read)
        let deviceIdData = deviceId.data(using: .utf8)
        deviceIdCharacteristic = CBMutableCharacteristic(
            type: DEVICE_ID_CHAR_UUID,
            properties: [.read],
            value: deviceIdData,
            permissions: [.readable]
        )
        
        // Create service
        let service = CBMutableService(type: SERVICE_UUID, primary: true)
        service.characteristics = [messageCharacteristic!, deviceIdCharacteristic!]
        
        // Add service to peripheral manager
        peripheral.add(service)
        print("[BleManager] GATT server configured")
        emitDiagnostic("info", "GATT server configured")
    }
    
    private func startFragmentPolling() {
        stopFragmentPolling()
        
        fragmentTimer = Timer.scheduledTimer(
            withTimeInterval: FRAGMENT_POLL_INTERVAL,
            repeats: true
        ) { [weak self] _ in
            self?.pollAndSendFragments()
        }
        
        RunLoop.current.add(fragmentTimer!, forMode: .common)
        print("[BleManager] Fragment polling started")
        emitDiagnostic("info", "Fragment polling started")
        
        startRoutingCleanup()
    }
    
    private func stopFragmentPolling() {
        fragmentTimer?.invalidate()
        fragmentTimer = nil
        stopRoutingCleanup()
        emitDiagnostic("info", "Fragment polling stopped")
    }
    
    // MARK: - Gradient Routing
    
    private func startRoutingCleanup() {
        stopRoutingCleanup()
        
        routingCleanupTimer = Timer.scheduledTimer(
            withTimeInterval: ROUTING_CLEANUP_INTERVAL,
            repeats: true
        ) { [weak self] _ in
            self?.protocolInstance.cleanupExpiredRoutes()
        }
        
        if let timer = routingCleanupTimer {
            RunLoop.current.add(timer, forMode: .common)
        }
    }
    
    private func stopRoutingCleanup() {
        routingCleanupTimer?.invalidate()
        routingCleanupTimer = nil
    }
    
    /// Computes route quality from RSSI value (0.0 to 1.0)
    private func computeRouteQuality(rssi: Int?) -> Float {
        guard let rssi = rssi else { return 0.5 }
        // Map RSSI from [-100, -20] to [0.0, 1.0]
        let normalized = Float(max(-100, min(-20, rssi)) + 100) / 80.0
        return normalized
    }
    
    /// Learns a route from a received message
    private func learnRouteFromMessage(_ messageJson: String, deliveredBy neighborId: String, neighborUUID: UUID?) {
        guard let data = messageJson.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let sender = json["sender"] as? String,
              let hopCount = json["hop_count"] as? Int else {
            return
        }
        
        // Don't learn route to ourselves
        if sender == deviceId { return }
        
        // Compute quality from RSSI
        let rssi = neighborUUID.flatMap { peripheralRSSI[$0] }.map { Int($0) }
        let quality = computeRouteQuality(rssi: rssi)
        
        // Learn the route: sender can be reached through neighborId
        protocolInstance.learnRoute(
            destination: sender,
            nextHop: neighborId,
            hopCount: UInt8(min(255, hopCount + 1)),
            quality: quality
        )
    }
    
    private func pollAndSendFragments() {
        fragmentQueue.async { [weak self] in
            guard let self = self else { return }
            
            if self.flushPendingOutboundFragments() {
                return
            }
            
            // Poll for next fragment from protocol
            if let fragment = self.protocolInstance.bleGetNextFragment() {
                print("[BleManager] Got fragment for recipient: \(fragment.recipientId), size: \(fragment.data.count)")
                self.emitDiagnostic("debug", "Polling got fragment", context: [
                    "recipientId": fragment.recipientId,
                    "fragmentSize": fragment.data.count
                ])
                self.sendFragment(fragment)
            }
        }
    }
    
    private func sendFragment(_ fragment: BleFragment) {
        let recipientId = fragment.recipientId
        let data = Data(fragment.data)
        
        let sendResult = sendFragmentData(recipientId: recipientId, data: data)
        print("[BleManager] Fragment send result for \(recipientId): \(sendResult)")
        
        if !sendResult {
            print("[BleManager] Failed to send fragment immediately, queuing for retry")
            enqueuePendingOutboundFragment(recipientId: recipientId, data: data)
        } else {
            print("[BleManager] Fragment sent successfully to \(recipientId)")
            emitDiagnostic("debug", "Fragment sent successfully", context: ["recipientId": recipientId])
        }
    }

    private func flushPendingOutboundFragments() -> Bool {
        var hasUnsentFragments = false
        let recipients = Array(pendingOutboundFragments.keys)
        
        for recipientId in recipients {
            guard var queue = pendingOutboundFragments[recipientId] else { continue }
            var sentAllForRecipient = true
            
            while !queue.isEmpty {
                let data = queue.first!
                if sendFragmentData(recipientId: recipientId, data: data) {
                    queue.removeFirst()
                } else {
                    sentAllForRecipient = false
                    break
                }
            }
            
            if queue.isEmpty {
                pendingOutboundFragments.removeValue(forKey: recipientId)
            } else {
                pendingOutboundFragments[recipientId] = queue
                if !sentAllForRecipient {
                    hasUnsentFragments = true
                }
            }
        }
        
        return hasUnsentFragments
    }
    
    private func enqueuePendingOutboundFragment(recipientId: String, data: Data) {
        var queue = pendingOutboundFragments[recipientId] ?? []
        queue.append(data)
        pendingOutboundFragments[recipientId] = queue
    }

    private func currentConnectionCount() -> Int {
        return connections.connectedPeripheralCount() + subscribedCentrals.count
    }

    private func refreshSelfMetrics() {
        let rssiValues = peripheralRSSI.values.map { Int($0) }
        let averageRssi = rssiValues.isEmpty ? nil : Int(Double(rssiValues.reduce(0, +)) / Double(rssiValues.count))
        let signalQuality = averageRssi.map { rssi -> Int in
            let clamped = max(-100, min(-20, rssi))
            let normalized = Double(clamped + 100) / 80.0
            let scaled = Int((normalized * 100.0).rounded())
            return min(100, max(0, scaled))
        }
        let pendingCount = pendingFragments.values.reduce(0) { $0 + $1.count }
        let outboundCount = pendingOutboundFragments.values.reduce(0) { $0 + $1.count }
        let totalPending = pendingCount + outboundCount
        let stability = max(0.0, 1.0 - min(1.0, Double(pendingCount) / 10.0))
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
        pendingFragments.removeValue(forKey: identifier)
        pendingOutboundFragments.removeValue(forKey: deviceId)
        connectionAttemptTimestamps.removeValue(forKey: identifier)
        meshController.registerDisconnection(peerId: deviceId)
        refreshSelfMetrics()
        
        // Clean up routes through this neighbor
        protocolInstance.removeNeighborRoutes(neighborId: deviceId)
        try? protocolInstance.blePeerLost(peerId: deviceId)
        DispatchQueue.main.async {
            self.refreshAdvertising(reason: "evict_\(reason)")
        }
        maybeHandleRebalance(reason: "evict")
    }
    
    private func sendFragmentData(recipientId: String, data: Data) -> Bool {
        guard let peripheral = findPeripheral(for: recipientId) else {
            if logThrottler.shouldLog(key: "missing_peripheral_\(recipientId)") {
                print("[BleManager] No connected peripheral for recipient: \(recipientId)")
                emitDiagnostic("warning", "No connected peripheral for BLE fragment", context: ["recipientId": recipientId])
            }
            return false
        }
        
        guard let (service, characteristic) = findMessageCharacteristic(on: peripheral) else {
            if logThrottler.shouldLog(key: "missing_char_\(recipientId)") {
                print("[BleManager] Message characteristic not found for recipient: \(recipientId)")
                emitDiagnostic("warning", "Message characteristic not found", context: ["recipientId": recipientId])
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
        bytesSent += UInt64(data.count)
        fragmentsSent += 1
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
                // Queue fragment to process later when device ID is available
                if self.pendingFragments[centralId] == nil {
                    self.pendingFragments[centralId] = []
                }
                self.pendingFragments[centralId]?.append((data, Date()))
                
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
                
                // CRITICAL FIX: Immediately check if this completed a message
                if let completedMessage = self.protocolInstance.receiveMessage() {
                    print("[BleManager] 🎉 COMPLETE MESSAGE ASSEMBLED FROM FRAGMENTS!")
                    print("[BleManager] 📬 Received message: \(completedMessage)")
                    self.emitDiagnostic("info", "Complete message assembled from fragments", context: [
                        "senderId": senderId,
                        "messageContent": completedMessage
                    ])
                    
                    // Learn route from the message sender through the delivering neighbor
                    self.learnRouteFromMessage(completedMessage, deliveredBy: senderId, neighborUUID: centralId)
                } else {
                    print("[BleManager] 📦 Fragment processed, waiting for more fragments to complete message")
                }
                
                self.bytesReceived += UInt64(data.count)
                self.fragmentsReceived += 1
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
        fragmentQueue.async { [weak self] in
            guard let self = self else { return }
            guard let fragments = self.pendingFragments.removeValue(forKey: centralId) else { return }
            let role = self.connections.consumePendingRole(for: centralId) ?? self.connections.connectionRole(for: deviceId) ?? .member
            self.meshController.registerConnection(peerId: deviceId, role: role)
            self.connections.setConnectionRole(role, for: deviceId)
            self.meshController.markPeerActive(deviceId)
            self.meshController.markPeerActive(self.deviceId)
            self.refreshSelfMetrics()
            if let rssi = self.peripheralRSSI[centralId] {
                self.meshController.updatePeerMetrics(peerId: deviceId, metrics: MeshController.PeerMetrics(rssi: Int(rssi)))
            }
            DispatchQueue.main.async {
                self.refreshAdvertising(reason: "membership_change")
            }
            
            for (data, _) in fragments {
                let bytes = [UInt8](data)
                do {
                    try self.protocolInstance.bleFragmentReceived(senderId: deviceId, fragment: bytes)
                    self.bytesReceived += UInt64(data.count)
                    self.fragmentsReceived += 1
                    self.meshController.markPeerActive(deviceId)
                    self.meshController.markPeerActive(self.deviceId)
                } catch {
                    print("[BleManager] Error processing pending fragment: \(error)")
                }
            }
        }
    }
    
    private func cleanupPendingFragments() {
        let now = Date()
        for (centralId, fragments) in pendingFragments {
            let validFragments = fragments.filter { now.timeIntervalSince($0.1) < PENDING_FRAGMENT_TIMEOUT }
            if validFragments.isEmpty {
                pendingFragments.removeValue(forKey: centralId)
            } else {
                pendingFragments[centralId] = validFragments
            }
        }
    }

    private func pruneMeshObservations(now: Date = Date()) {
        lastSeenMeshAdvertisements = lastSeenMeshAdvertisements.filter { now.timeIntervalSince($0.value.timestamp) <= MESH_OBSERVATION_TTL }
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
        // Prune old entries
        let oneMinuteAgo = now.addingTimeInterval(-60.0)
        globalConnectionAttempts = globalConnectionAttempts.filter { $0 > oneMinuteAgo }
        peripheralConnectionAttempts = peripheralConnectionAttempts.filter { 
            now.timeIntervalSince($0.value) < ADAPTIVE_COOLDOWN_PER_PERIPHERAL 
        }
        
        // Check per-peripheral cooldown
        if let lastAttempt = peripheralConnectionAttempts[peripheral],
           now.timeIntervalSince(lastAttempt) < ADAPTIVE_COOLDOWN_PER_PERIPHERAL {
            return true
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

    private func identifierForNodeHash(_ nodeHash: UInt64) -> UUID? {
        for (identifier, observation) in lastSeenMeshAdvertisements where observation.advertisement.nodeIdHash == nodeHash {
            return identifier
        }
        return nil
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
                    try? self.protocolInstance.bleStatusChanged(isAvailable: false)
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
            startFragmentPolling()
            emitDiagnostic("info", "Central manager powered on and ready")
            
            // If both central and peripheral are ready, mark as running
            if peripheralReady && state == .starting {
                updateState(.running)
                print("[BleManager] ✅ BLE Manager ready - calling bleStatusChanged(true)")
                emitDiagnostic("info", "About to call protocol.bleStatusChanged(true)")
                try? self.protocolInstance.bleStatusChanged(isAvailable: true)
                print("[BleManager] ✅ Called protocol.bleStatusChanged(true)")
                emitDiagnostic("info", "Successfully called protocol.bleStatusChanged(true)")
            }
            
        case .poweredOff:
            print("[BleManager] ⚠️ Bluetooth is powered off")
            centralReady = false
            stopScanning(reason: "central_powered_off")
            updateState(.unavailable)
            try? self.protocolInstance.bleStatusChanged(isAvailable: false)
            emitDiagnostic("warning", "Bluetooth is powered off", context: ["state": stateString])
            
        case .unauthorized:
            print("[BleManager] ⚠️ Bluetooth is unauthorized")
            centralReady = false
            stopScanning(reason: "central_unauthorized")
            updateState(.unavailable)
            try? self.protocolInstance.bleStatusChanged(isAvailable: false)
            emitDiagnostic("error", "Bluetooth is unauthorized", context: ["state": stateString, "authorization": authStatus])
            
        case .unsupported:
            print("[BleManager] ⚠️ Bluetooth is not supported on this device")
            centralReady = false
            stopScanning(reason: "central_unsupported")
            updateState(.unavailable)
            try? self.protocolInstance.bleStatusChanged(isAvailable: false)
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
        
        let isConnectable: Bool
        if #available(iOS 13.0, *) {
            isConnectable = (advertisementData[CBAdvertisementDataIsConnectable] as? NSNumber)?.boolValue ?? true
        } else {
            isConnectable = true
        }
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

        let decision = meshController.shouldInitiateOutbound(metadata: meshMetadata, rssi: Int(rssiValue))
        guard decision.intent != .rejected else {
            if logThrottler.shouldLog(key: "mesh_skip_\(peripheral.identifier.uuidString)", interval: 15) {
                print("[BleManager] Skipping \(peripheral.identifier) due to \(decision.reason)")
            }
            return
        }
        
        // Adaptive scanning: rate limit connection attempts
        if shouldThrottleConnection(to: peripheral.identifier, now: now) {
            if logThrottler.shouldLog(key: "adaptive_throttle_\(peripheral.identifier.uuidString)", interval: 30) {
                print("[BleManager] Adaptive: throttling connection to \(peripheral.identifier)")
            }
            return
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
        
        // Discover services
        peripheral.discoverServices([SERVICE_UUID])
    }
    
    public func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        print("[BleManager] Failed to connect to peripheral: \(error?.localizedDescription ?? "unknown")")
        emitDiagnostic("error", "Failed to connect to BLE peripheral", context: [
            "identifier": peripheral.identifier.uuidString,
            "error": error?.localizedDescription ?? "unknown"
        ])
        connectionAttemptTimestamps.removeValue(forKey: peripheral.identifier)
        _ = connections.consumePendingRole(for: peripheral.identifier)
        DispatchQueue.main.asyncAfter(deadline: .now() + MIN_RECONNECT_INTERVAL) { [weak self] in
            self?.attemptConnection(to: peripheral, reason: "retry_fail")
        }
    }
    
    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        let wasConnected = connections.connectedPeripheral(for: peripheral.identifier) != nil
        _ = connections.removePeripheral(peripheral.identifier)
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
                        // Clean up routes through this neighbor
                        protocolInstance.removeNeighborRoutes(neighborId: deviceId)
                        try? self.protocolInstance.blePeerLost(peerId: deviceId)
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
            
            // Attempt reconnection after a short delay
            DispatchQueue.main.asyncAfter(deadline: .now() + MIN_RECONNECT_INTERVAL) { [weak self] in
                guard let self = self else { return }
                if self.state == .running && self.discoveredPeripherals[peripheral.identifier] != nil {
                    self.attemptConnection(to: peripheral, reason: "retry_disconnect")
                }
            }
        } else {
            // Wasn't connected, just notify if we had device ID
            if let deviceId = connections.peripheralDeviceId(for: peripheral.identifier) {
                // Clean up routes through this neighbor
                protocolInstance.removeNeighborRoutes(neighborId: deviceId)
                try? self.protocolInstance.blePeerLost(peerId: deviceId)
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
            return
        }
        
        guard let services = peripheral.services else { return }
        
        for service in services where service.uuid == SERVICE_UUID {
            peripheral.discoverCharacteristics([MESSAGE_CHAR_UUID, DEVICE_ID_CHAR_UUID], for: service)
        }
        emitDiagnostic("info", "Discovered BLE services", context: ["peripheral": peripheral.identifier.uuidString])
    }
    
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        if let error = error {
            print("[BleManager] Error discovering characteristics: \(error)")
            emitDiagnostic("error", "Error discovering characteristics", context: ["error": error.localizedDescription])
            return
        }
        
        guard let characteristics = service.characteristics else { return }
        
        for characteristic in characteristics {
            if characteristic.uuid == MESSAGE_CHAR_UUID {
                // Enable notifications for message characteristic
                peripheral.setNotifyValue(true, for: characteristic)
                print("[BleManager] Enabled notifications for message characteristic")
                emitDiagnostic("info", "Enabled notifications for message characteristic", context: ["peripheral": peripheral.identifier.uuidString])
            } else if characteristic.uuid == DEVICE_ID_CHAR_UUID {
                // Read device ID
                peripheral.readValue(for: characteristic)
            }
        }
    }
    
    public func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        if let error = error {
            print("[BleManager] Error reading characteristic: \(error)")
            emitDiagnostic("error", "Error reading characteristic", context: ["error": error.localizedDescription])
            return
        }
        
        guard let data = characteristic.value else { return }
        
        if characteristic.uuid == DEVICE_ID_CHAR_UUID {
            // Store device ID
            if let deviceId = String(data: data, encoding: .utf8) {
                connections.setPeripheralDeviceId(deviceId, for: peripheral.identifier)
                connections.setCentralDeviceId(deviceId, for: peripheral.identifier)
                connectionAttemptTimestamps.removeValue(forKey: peripheral.identifier)

                let rssi = peripheralRSSI[peripheral.identifier] ?? -60
                try? self.protocolInstance.blePeerDiscovered(peerId: deviceId, rssi: rssi)
                let role = connections.consumePendingRole(for: peripheral.identifier) ?? connections.connectionRole(for: deviceId) ?? .member
                meshController.registerConnection(peerId: deviceId, role: role)
                connections.setConnectionRole(role, for: deviceId)
                meshController.markPeerActive(deviceId)
                meshController.markPeerActive(self.deviceId)
                refreshSelfMetrics()
                meshController.updatePeerMetrics(peerId: deviceId, metrics: MeshController.PeerMetrics(rssi: Int(rssi)))
                DispatchQueue.main.async {
                    self.refreshAdvertising(reason: "membership_change")
                }

                // Process any pending fragments for this device
                processPendingFragments(for: peripheral.identifier, deviceId: deviceId)
            }
        } else if characteristic.uuid == MESSAGE_CHAR_UUID {
            // Handle received message fragment
            handleReceivedData(data, senderId: connections.peripheralDeviceId(for: peripheral.identifier), centralId: peripheral.identifier)
        }
    }
    
    public func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        if let error = error {
            print("[BleManager] Error writing characteristic: \(error)")
            emitDiagnostic("error", "Error writing characteristic", context: ["error": error.localizedDescription])
        }
    }
}

// MARK: - CBPeripheralManagerDelegate

extension BleManager: CBPeripheralManagerDelegate {
    
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
                    try? self.protocolInstance.bleStatusChanged(isAvailable: false)
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
                print("[BleManager] ✅ BLE Manager ready (peripheral) - calling bleStatusChanged(true)")
                emitDiagnostic("info", "About to call protocol.bleStatusChanged(true) from peripheral")
                try? self.protocolInstance.bleStatusChanged(isAvailable: true)
                print("[BleManager] ✅ Called protocol.bleStatusChanged(true) from peripheral")
                emitDiagnostic("info", "Successfully called protocol.bleStatusChanged(true) from peripheral")
            }
            
        case .poweredOff:
            print("[BleManager] ⚠️ Bluetooth peripheral is powered off")
            peripheralReady = false
            stopAdvertising()
            updateState(.unavailable)
            try? self.protocolInstance.bleStatusChanged(isAvailable: false)
            emitDiagnostic("warning", "Bluetooth peripheral is powered off", context: ["state": stateString])
            
        case .unauthorized:
            print("[BleManager] ⚠️ Bluetooth peripheral is unauthorized")
            peripheralReady = false
            stopAdvertising()
            updateState(.unavailable)
            try? self.protocolInstance.bleStatusChanged(isAvailable: false)
            emitDiagnostic("error", "Bluetooth peripheral is unauthorized", context: ["state": stateString, "authorization": authStatus])
            
        case .unsupported:
            print("[BleManager] ⚠️ Bluetooth peripheral is not supported on this device")
            peripheralReady = false
            stopAdvertising()
            updateState(.unavailable)
            try? self.protocolInstance.bleStatusChanged(isAvailable: false)
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
                if senderId == nil && logThrottler.shouldLog(key: "missing_sender_\(request.central.identifier.uuidString)", interval: 10) {
                    print("[BleManager] Received write without known sender for central \(request.central.identifier)")
                    emitDiagnostic("warning", "Received BLE fragment without sender ID", context: [
                        "central": request.central.identifier.uuidString,
                        "length": value.count
                    ])
                }
                if senderId == nil {
                    ensureDeviceId(for: request.central.identifier)
                }
                handleReceivedData(value, senderId: senderId, centralId: request.central.identifier)
            } else {
                print("[BleManager] ❌ Unknown characteristic write: \(request.characteristic.uuid)")
            }
            
            // Respond to write request
            peripheral.respond(to: request, withResult: .success)
            print("[BleManager] ✅ Sent success response to \(request.central.identifier)")
        }
    }
    
    public func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didSubscribeTo characteristic: CBCharacteristic) {
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
        emitDiagnostic("info", "Central unsubscribed", context: [
            "central": central.identifier.uuidString,
            "characteristic": characteristic.uuid.uuidString
        ])
        connections.removeCentralDeviceId(for: central.identifier)
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

