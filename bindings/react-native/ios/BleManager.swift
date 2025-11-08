//
//  BleManager.swift
//  OfflineProtocol
//
//  BLE transport implementation using CoreBluetooth
//  Supports iOS ↔ Android cross-platform communication
//

import Foundation
import CoreBluetooth

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
    
    // MARK: - Properties
    
    private let protocolInstance: OfflineProtocol
    private let deviceId: String
    
    // Central (scanner/client) components
    private var centralManager: CBCentralManager?
    private var discoveredPeripherals: [UUID: CBPeripheral] = [:]
    private var connectedPeripherals: [UUID: CBPeripheral] = [:]
    private var peripheralDeviceIds: [UUID: String] = [:]
    private var peripheralRSSI: [UUID: Int16] = [:]
    private var centralDeviceIds: [UUID: String] = [:]
    
    // Peripheral (advertiser/server) components
    private var peripheralManager: CBPeripheralManager?
    private var messageCharacteristic: CBMutableCharacteristic?
    private var deviceIdCharacteristic: CBMutableCharacteristic?
    
    // Fragment polling
    private var fragmentTimer: Timer?
    private let fragmentQueue = DispatchQueue(label: "com.offlineprotocol.ble.fragments")
    
    // Pending fragments waiting for device ID
    private var pendingFragments: [UUID: [(Data, Date)]] = [:]
    private let PENDING_FRAGMENT_TIMEOUT: TimeInterval = 5.0
    
    // State tracking
    private var isScanning = false
    private var isAdvertising = false
    private var centralReady = false
    private var peripheralReady = false
    
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

    // MARK: - Diagnostics
    private func emitDiagnostic(_ level: String, _ message: String, context: [String: Any] = [:]) {
        delegate?.transportManager(self, didEmitDiagnostic: level, message: message, context: context)
    }
    
    // MARK: - Initialization
    
    public init(protocol protocolInstance: OfflineProtocol, deviceId: String) {
        self.protocolInstance = protocolInstance
        self.deviceId = deviceId
        super.init()
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
        guard state != .running else {
            throw TransportError.alreadyRunning
        }
        
        guard isAvailable() else {
            throw TransportError.notAvailable("BLE not available on this device")
        }
        
        print("[BleManager] Starting BLE transport for device: \(deviceId)")
        emitDiagnostic("info", "Starting BLE transport", context: ["deviceId": deviceId])
        updateState(.starting)
        
        // Initialize Central Manager (for scanning)
        centralManager = CBCentralManager(
            delegate: self,
            queue: nil,
            options: [CBCentralManagerOptionShowPowerAlertKey: true]
        )
        
        // Initialize Peripheral Manager (for advertising)
        peripheralManager = CBPeripheralManager(
            delegate: self,
            queue: nil,
            options: [CBPeripheralManagerOptionShowPowerAlertKey: true]
        )
        
        print("[BleManager] Waiting for Bluetooth to power on...")
        emitDiagnostic("info", "Waiting for Bluetooth to power on")
        // Note: Actual start happens in delegate callbacks when ready
    }
    
    public func stop() {
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
        for peripheral in connectedPeripherals.values {
            centralManager?.cancelPeripheralConnection(peripheral)
        }
        connectedPeripherals.removeAll()
        discoveredPeripherals.removeAll()
        peripheralDeviceIds.removeAll()
        peripheralRSSI.removeAll()
        centralDeviceIds.removeAll()
        pendingFragments.removeAll()
        
        // Clean up managers
        centralManager = nil
        peripheralManager = nil
        
        centralReady = false
        peripheralReady = false
        
        updateState(.stopped)
        emitDiagnostic("info", "BLE transport stopped")
    }
    
    public func pause() {
        // For iOS background mode
        stopScanning(reason: "pause")
        stopFragmentPolling()
    }
    
    public func resume() {
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
            "connected_peers": connectedPeripherals.count,
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
                if self.connectedPeripherals[peripheral.identifier] == nil {
                    self.attemptConnection(to: peripheral, reason: "monitor")
                }
            }
            for centralId in self.pendingFragments.keys {
                if self.centralDeviceIds[centralId] == nil && self.peripheralDeviceIds[centralId] == nil {
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
    
    private func attemptConnection(to peripheral: CBPeripheral, reason: String, rssi: Int16? = nil) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            if self.connectedPeripherals[peripheral.identifier] != nil {
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
            self.connectionAttemptTimestamps[peripheral.identifier] = now
            peripheral.delegate = self
            if peripheral.state == .connected {
                self.connectedPeripherals[peripheral.identifier] = peripheral
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
            if self.centralDeviceIds[centralId] != nil || self.peripheralDeviceIds[centralId] != nil {
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
    
    private func startAdvertising() {
        guard let peripheral = peripheralManager, peripheral.state == .poweredOn, !isAdvertising else {
            return
        }
        
        // Create GATT service
        setupGattServer()
        
        // Start advertising
        let advertisementData: [String: Any] = [
            CBAdvertisementDataServiceUUIDsKey: [SERVICE_UUID],
            CBAdvertisementDataLocalNameKey: "OfflineProtocol"
        ]
        
        // Reduced logging
        peripheral.startAdvertising(advertisementData)
        isAdvertising = true
    }
    
    private func stopAdvertising() {
        guard isAdvertising else { return }
        peripheralManager?.stopAdvertising()
        isAdvertising = false
        print("[BleManager] Stopped advertising")
        emitDiagnostic("info", "Stopped BLE advertising")
    }
    
    private func setupGattServer() {
        guard let peripheral = peripheralManager else { return }
        
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
    }
    
    private func stopFragmentPolling() {
        fragmentTimer?.invalidate()
        fragmentTimer = nil
        emitDiagnostic("info", "Fragment polling stopped")
    }
    
    private func pollAndSendFragments() {
        fragmentQueue.async { [weak self] in
            guard let self = self else { return }
            
            // Poll for next fragment from protocol
            if let fragment = self.protocolInstance.bleGetNextFragment() {
                self.sendFragment(fragment)
            }
        }
    }
    
    private func sendFragment(_ fragment: BleFragment) {
        let recipientId = fragment.recipientId
        let data = Data(fragment.data)
        
        // Find peripheral with matching device ID
        var targetPeripheral: CBPeripheral?
        for (uuid, deviceId) in peripheralDeviceIds {
            if deviceId == recipientId, let peripheral = connectedPeripherals[uuid] {
                targetPeripheral = peripheral
                break
            }
        }
        
        guard let peripheral = targetPeripheral else {
            print("[BleManager] No connected peripheral for recipient: \(recipientId)")
            emitDiagnostic("warning", "No connected peripheral for BLE fragment", context: ["recipientId": recipientId])
            return
        }
        
        // Find message characteristic
        guard let service = peripheral.services?.first(where: { $0.uuid == SERVICE_UUID }),
              let characteristic = service.characteristics?.first(where: { $0.uuid == MESSAGE_CHAR_UUID }) else {
            print("[BleManager] Message characteristic not found")
            emitDiagnostic("warning", "Message characteristic not found", context: ["recipientId": recipientId])
            return
        }
        
        // Write data without response
        peripheral.writeValue(data, for: characteristic, type: .withoutResponse)
        
        bytesSent += UInt64(data.count)
        fragmentsSent += 1
        
        // Reduced logging - only log errors
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
                if self.centralDeviceIds[centralId] == nil && self.peripheralDeviceIds[centralId] == nil {
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

            do {
                try self.protocolInstance.bleFragmentReceived(senderId: senderId, fragment: bytes)
                self.bytesReceived += UInt64(data.count)
                self.fragmentsReceived += 1
            } catch {
                print("[BleManager] Error processing received fragment: \(error)")
                self.emitDiagnostic("error", "Error processing received fragment", context: ["error": error.localizedDescription])
            }
        }
    }
    
    private func processPendingFragments(for centralId: UUID, deviceId: String) {
        fragmentQueue.async { [weak self] in
            guard let self = self else { return }
            guard let fragments = self.pendingFragments.removeValue(forKey: centralId) else { return }
            
            for (data, _) in fragments {
                let bytes = [UInt8](data)
                do {
                    try self.protocolInstance.bleFragmentReceived(senderId: deviceId, fragment: bytes)
                    self.bytesReceived += UInt64(data.count)
                    self.fragmentsReceived += 1
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
        print("[BleManager] Central state: \(central.state.rawValue)")
        emitDiagnostic("info", "Central manager state changed", context: ["state": central.state.rawValue])
        
        switch central.state {
        case .poweredOn:
            centralReady = true
            startScanning(reason: "central_powered_on")
            startFragmentPolling()
            emitDiagnostic("info", "Central manager powered on")
            
            // If both central and peripheral are ready, mark as running
            if peripheralReady && state == .starting {
                updateState(.running)
                try? self.protocolInstance.bleStatusChanged(isAvailable: true)
            }
            
        case .poweredOff, .unauthorized, .unsupported:
            centralReady = false
            stopScanning(reason: "central_state_\(central.state.rawValue)")
            updateState(.unavailable)
            try? self.protocolInstance.bleStatusChanged(isAvailable: false)
            emitDiagnostic("warning", "Central manager unavailable", context: ["state": central.state.rawValue])
            
        default:
            break
        }
    }
    
    public func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral, advertisementData: [String: Any], rssi RSSI: NSNumber) {
        let rssiValue = RSSI.int16Value
        markDiscoveryEvent()
        
        // Store discovered peripheral
        discoveredPeripherals[peripheral.identifier] = peripheral
        peripheralRSSI[peripheral.identifier] = rssiValue
        
        let now = Date()
        let isConnectable: Bool
        if #available(iOS 13.0, *) {
            isConnectable = (advertisementData[CBAdvertisementDataIsConnectable] as? NSNumber)?.boolValue ?? true
        } else {
            isConnectable = true
        }
        if discoveryLogTimestamps[peripheral.identifier] == nil || (now.timeIntervalSince(discoveryLogTimestamps[peripheral.identifier]!) > 30) {
            discoveryLogTimestamps[peripheral.identifier] = now
            print("[BleManager] Discovered peripheral: \(peripheral.identifier) RSSI=\(rssiValue)")
            emitDiagnostic("info", "Discovered BLE peripheral", context: [
                "identifier": peripheral.identifier.uuidString,
                "rssi": rssiValue,
                "connectable": isConnectable
            ])
        }
        
        attemptConnection(to: peripheral, reason: "discovery", rssi: rssiValue)
    }
    
    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        print("[BleManager] Connected to peripheral: \(peripheral.identifier)")
        emitDiagnostic("info", "Connected to BLE peripheral", context: ["identifier": peripheral.identifier.uuidString])
        
        connectedPeripherals[peripheral.identifier] = peripheral
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
        DispatchQueue.main.asyncAfter(deadline: .now() + MIN_RECONNECT_INTERVAL) { [weak self] in
            self?.attemptConnection(to: peripheral, reason: "retry_fail")
        }
    }
    
    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        let wasConnected = connectedPeripherals[peripheral.identifier] != nil
        connectedPeripherals.removeValue(forKey: peripheral.identifier)
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
                if error.code == .connectionTimeout || error.code == .peerRemovedPairingInformation {
                    // Permanent error - notify peer lost
                    if let deviceId = peripheralDeviceIds[peripheral.identifier] {
                        try? self.protocolInstance.blePeerLost(peerId: deviceId)
                        peripheralDeviceIds.removeValue(forKey: peripheral.identifier)
                        centralDeviceIds.removeValue(forKey: peripheral.identifier)
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
            if let deviceId = peripheralDeviceIds[peripheral.identifier] {
                try? self.protocolInstance.blePeerLost(peerId: deviceId)
            }
        }
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
                peripheralDeviceIds[peripheral.identifier] = deviceId
                centralDeviceIds[peripheral.identifier] = deviceId
                connectionAttemptTimestamps.removeValue(forKey: peripheral.identifier)

                let rssi = peripheralRSSI[peripheral.identifier] ?? -60
                try? self.protocolInstance.blePeerDiscovered(peerId: deviceId, rssi: rssi)

                // Process any pending fragments for this device
                processPendingFragments(for: peripheral.identifier, deviceId: deviceId)
            }
        } else if characteristic.uuid == MESSAGE_CHAR_UUID {
            // Handle received message fragment
            handleReceivedData(data, senderId: peripheralDeviceIds[peripheral.identifier], centralId: peripheral.identifier)
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
        print("[BleManager] Peripheral state: \(peripheral.state.rawValue)")
        emitDiagnostic("info", "Peripheral manager state changed", context: ["state": peripheral.state.rawValue])
        
        switch peripheral.state {
        case .poweredOn:
            peripheralReady = true
            startAdvertising()
            emitDiagnostic("info", "Peripheral manager powered on")
            
            // If both central and peripheral are ready, mark as running
            if centralReady && state == .starting {
                updateState(.running)
                try? self.protocolInstance.bleStatusChanged(isAvailable: true)
            }
            
        case .poweredOff, .unauthorized, .unsupported:
            peripheralReady = false
            stopAdvertising()
            updateState(.unavailable)
            try? self.protocolInstance.bleStatusChanged(isAvailable: false)
            emitDiagnostic("warning", "Peripheral manager unavailable", context: ["state": peripheral.state.rawValue])
            
        default:
            break
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
            if request.characteristic.uuid == MESSAGE_CHAR_UUID, let value = request.value {
                let senderId = centralDeviceIds[request.central.identifier] ?? peripheralDeviceIds[request.central.identifier]
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
            }
            
            // Respond to write request
            peripheral.respond(to: request, withResult: .success)
        }
    }
    
    public func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didSubscribeTo characteristic: CBCharacteristic) {
        // When central subscribes, try to get device ID if we don't have it
        if centralDeviceIds[central.identifier] == nil && peripheralDeviceIds[central.identifier] == nil {
            ensureDeviceId(for: central.identifier)
        } else if let deviceId = peripheralDeviceIds[central.identifier] {
            centralDeviceIds[central.identifier] = deviceId
            // Process any pending fragments
            processPendingFragments(for: central.identifier, deviceId: deviceId)
        }
    }
    
    public func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didUnsubscribeFrom characteristic: CBCharacteristic) {
        print("[BleManager] Central unsubscribed from characteristic: \(characteristic.uuid)")
        emitDiagnostic("info", "Central unsubscribed", context: [
            "central": central.identifier.uuidString,
            "characteristic": characteristic.uuid.uuidString
        ])
        centralDeviceIds.removeValue(forKey: central.identifier)
    }
}

