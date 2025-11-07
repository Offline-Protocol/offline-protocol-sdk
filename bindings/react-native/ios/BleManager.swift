import Foundation
import CoreBluetooth

/// BLE Manager for Offline Protocol (iOS)
///
/// Handles:
/// - BLE advertising (making device discoverable)
/// - BLE scanning (discovering nearby devices)
/// - GATT peripheral (receiving messages)
/// - GATT central (sending messages)
@objc(BleManager)
class BleManager: NSObject {
    
    // MARK: - Types
    
    enum Status: String {
        case unavailable
        case available
        case scanning
        case advertising
        case connected
        case disconnected
    }
    
    struct DiscoveredPeer {
        let deviceId: String
        var peripheral: CBPeripheral
        var rssi: Int
        var lastSeen: Date
        var connected: Bool = false
    }
    
    // MARK: - Constants
    
    static let serviceUUID = CBUUID(string: "6E400001-B5A3-F393-E0A9-E50E24DCCA9E")
    static let messageCharUUID = CBUUID(string: "6E400002-B5A3-F393-E0A9-E50E24DCCA9E")
    static let deviceIdCharUUID = CBUUID(string: "6E400003-B5A3-F393-E0A9-E50E24DCCA9E")
    
    // MARK: - Properties
    
    private let deviceId: String
    private var centralManager: CBCentralManager?
    private var peripheralManager: CBPeripheralManager?
    
    // Track manager states
    private var centralManagerReady = false
    private var peripheralManagerReady = false
    private var shouldStartOperations = false
    private var gattServiceAdded = false
    
    // Discovered peers
    private var discoveredPeers: [String: DiscoveredPeer] = [:]
    
    // Track RSSI values from scan results (keyed by peripheral UUID)
    private var rssiValues: [UUID: Int] = [:]
    
    // Track device IDs that have already been discovered (for deduplication)
    private var discoveredDeviceIds: Set<String> = []
    
    // Track peripherals being connected (keyed by peripheral UUID) - prevents deallocation
    private var connectingPeripherals: [UUID: CBPeripheral] = [:]
    
    // Connected peripherals for messaging
    private var connectedPeripherals: [String: CBPeripheral] = [:]

    // Peer expiry tracking
    private var peerExpiryTimer: Timer?
    private let peerExpiryCheckInterval: TimeInterval = 5.0
    private let peerLossTimeout: TimeInterval = 60.0
    
    // Connection timeout timers (keyed by peripheral UUID)
    private var connectionTimeouts: [UUID: Timer] = [:]
    private let connectionTimeoutInterval: TimeInterval = 30.0 // 30 seconds (BLE can be slow)
    
    // Service discovery timeout timers (keyed by peripheral UUID)
    private var serviceDiscoveryTimeouts: [UUID: Timer] = [:]
    private let serviceDiscoveryTimeoutInterval: TimeInterval = 10.0 // 10 seconds
    
    // Characteristic discovery timeout timers (keyed by peripheral UUID)
    private var characteristicDiscoveryTimeouts: [UUID: Timer] = [:]
    private let characteristicDiscoveryTimeoutInterval: TimeInterval = 10.0 // 10 seconds
    
    // Characteristics
    private var messageCharacteristic: CBMutableCharacteristic?
    private var deviceIdCharacteristic: CBMutableCharacteristic?
    
    // Callbacks
    var onPeerDiscovered: ((String, String, Int) -> Void)?
    var onPeerLost: ((String) -> Void)?
    var onMessageReceived: ((Data) -> Void)?
    var onStatusChanged: ((Status) -> Void)?
    var onDiagnostic: ((String) -> Void)?
    var onPeerUpdated: ((String, String, Int) -> Void)?
    
    // MARK: - Initialization
    
    @objc init(deviceId: String) {
        self.deviceId = deviceId
        super.init()
    }
    
    // MARK: - Public Methods
    
    @objc func start() -> Bool {
        var didScheduleStart = false
        let startBlock = {
            NSLog("[BleManager] Starting BLE operations for device: \(self.deviceId)")
            self.onDiagnostic?("[BLE] Starting BLE operations for device: \(self.deviceId)")

            self.shouldStartOperations = true

            // Initialize central manager (for scanning)
            // IMPORTANT: queue: nil means use main queue. All CB operations must happen
            // on the same queue where the manager was created to avoid missed callbacks.
            if self.centralManager == nil {
                self.onDiagnostic?("[BLE] Initializing Central Manager (scanner)")
                self.centralManager = CBCentralManager(delegate: self, queue: nil)
            }

            // Initialize peripheral manager (for advertising)
            // IMPORTANT: queue: nil means use main queue. Must match central manager queue.
            if self.peripheralManager == nil {
                self.onDiagnostic?("[BLE] Initializing Peripheral Manager (advertiser)")
                self.peripheralManager = CBPeripheralManager(delegate: self, queue: nil)
            }

            // Try to start operations if managers are already ready
            self.startOperationsIfReady()
            didScheduleStart = true

            self.startPeerExpiryTimer()
        }

        if Thread.isMainThread {
            startBlock()
        } else {
            DispatchQueue.main.sync(execute: startBlock)
        }

        return didScheduleStart
    }

    @objc func stop() {
        let stopBlock = {
            NSLog("[BleManager] Stopping BLE operations")

            self.shouldStartOperations = false
            self.gattServiceAdded = false

            self.stopPeerExpiryTimer()

            // Stop scanning
            self.centralManager?.stopScan()

            // Stop advertising
            self.peripheralManager?.stopAdvertising()

            // Cancel all timeout timers
            for (_, timer) in self.connectionTimeouts {
                timer.invalidate()
            }
            self.connectionTimeouts.removeAll()

            for (_, timer) in self.serviceDiscoveryTimeouts {
                timer.invalidate()
            }
            self.serviceDiscoveryTimeouts.removeAll()

            for (_, timer) in self.characteristicDiscoveryTimeouts {
                timer.invalidate()
            }
            self.characteristicDiscoveryTimeouts.removeAll()

            // Disconnect all peripherals
            for (_, peripheral) in self.connectedPeripherals {
                self.centralManager?.cancelPeripheralConnection(peripheral)
            }
            for (_, peripheral) in self.connectingPeripherals {
                self.centralManager?.cancelPeripheralConnection(peripheral)
            }
            self.connectedPeripherals.removeAll()
            self.connectingPeripherals.removeAll()
            self.discoveredPeers.removeAll()
            self.rssiValues.removeAll()
            self.discoveredDeviceIds.removeAll()

            self.onStatusChanged?(.disconnected)
        }

        if Thread.isMainThread {
            stopBlock()
        } else {
            DispatchQueue.main.sync(execute: stopBlock)
        }
    }
    
    @objc func sendMessage(recipientId: String, messageData: Data) -> Bool {
        var sendSucceeded = false

        let sendBlock = {
            guard let peripheral = self.connectedPeripherals[recipientId] else {
                let msg = "[BleManager] No connection to peer: \(recipientId)"
                NSLog(msg)
                self.onDiagnostic?("[BLE] ❌ No active BLE connection to peer \(recipientId)")
                return
            }

            guard peripheral.state == .connected else {
                let msg = "[BleManager] Peripheral for \(recipientId) is not connected (state: \(peripheral.state.rawValue))"
                NSLog(msg)
                self.onDiagnostic?("[BLE] ⚠️ Peripheral for \(recipientId) not connected (state: \(peripheral.state.rawValue))")
                return
            }

            guard let service = peripheral.services?.first(where: { $0.uuid == BleManager.serviceUUID }) else {
                let msg = "[BleManager] Service not discovered yet for peer \(recipientId)"
                NSLog(msg)
                self.onDiagnostic?("[BLE] ℹ️ Service not ready for \(recipientId) – requesting discovery")
                peripheral.discoverServices([BleManager.serviceUUID])
                return
            }

            guard let characteristic = service.characteristics?.first(where: { $0.uuid == BleManager.messageCharUUID }) else {
                let msg = "[BleManager] Message characteristic not available for peer \(recipientId)"
                NSLog(msg)
                self.onDiagnostic?("[BLE] ℹ️ Message characteristic not ready for \(recipientId) – rediscovering")
                peripheral.discoverCharacteristics(nil, for: service)
                return
            }

            peripheral.writeValue(messageData, for: characteristic, type: .withResponse)
            let msg = "[BleManager] Sending message to \(recipientId) (\(messageData.count) bytes)"
            NSLog(msg)
            self.onDiagnostic?("[BLE] 🚀 Sent BLE fragment to \(recipientId) (\(messageData.count) bytes)")
            sendSucceeded = true
        }

        if Thread.isMainThread {
            sendBlock()
        } else {
            DispatchQueue.main.sync(execute: sendBlock)
        }

        return sendSucceeded
    }
    
    @objc func getDiscoveredPeers() -> [[String: Any]] {
        return discoveredPeers.values.map { peer in
            return [
                "deviceId": peer.deviceId,
                "rssi": peer.rssi,
                "connected": peer.connected
            ]
        }
    }

    // MARK: - Peer Expiry Management

    private func startPeerExpiryTimer() {
        guard peerExpiryTimer == nil else { return }

        let timer = Timer(timeInterval: peerExpiryCheckInterval, repeats: true) { [weak self] _ in
            self?.checkForExpiredPeers()
        }
        RunLoop.main.add(timer, forMode: .common)
        peerExpiryTimer = timer
    }

    private func stopPeerExpiryTimer() {
        peerExpiryTimer?.invalidate()
        peerExpiryTimer = nil
    }

    private func checkForExpiredPeers() {
        let now = Date()
        var expired: [String] = []

        for (deviceId, peer) in discoveredPeers {
            if peer.connected {
                continue
            }
            if now.timeIntervalSince(peer.lastSeen) > peerLossTimeout {
                expired.append(deviceId)
            }
        }

        guard !expired.isEmpty else { return }

        for deviceId in expired {
            guard let peer = discoveredPeers.removeValue(forKey: deviceId) else { continue }

            let msg = "[BLE] ⏳ Peer \(deviceId) expired after \(peerLossTimeout)s without advertisements"
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)

            connectedPeripherals.removeValue(forKey: deviceId)
            discoveredDeviceIds.remove(deviceId)
            rssiValues.removeValue(forKey: peer.peripheral.identifier)
            connectingPeripherals.removeValue(forKey: peer.peripheral.identifier)

            onPeerLost?(deviceId)
        }
    }
    
    // MARK: - Private Methods
    
    private func startScanning() {
        guard let centralManager = centralManager,
              centralManager.state == .poweredOn else {
            let msg = "[BLE] Cannot start scanning - Bluetooth not ready"
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)
            return
        }
        
        let msg = "[BLE] 🔍 Starting BLE scanning for service UUID: \(BleManager.serviceUUID.uuidString)"
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        centralManager.scanForPeripherals(
            withServices: [BleManager.serviceUUID],
            options: [CBCentralManagerScanOptionAllowDuplicatesKey: true]  // CRITICAL: Without this, iOS throttles/suppresses duplicate advertisements
                                                                            // causing false "peer lost" events when iOS doesn't deliver scans for 3-5s
        )
        onStatusChanged?(.scanning)
    }
    
    private func startAdvertising() {
        guard let peripheralManager = peripheralManager,
              peripheralManager.state == .poweredOn else {
            let msg = "[BLE] Cannot start advertising - Bluetooth not ready"
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)
            return
        }
        
        // Setup GATT service if not already added
        if !gattServiceAdded {
            let msg = "[BLE] ⚙️ Setting up GATT service (will advertise when ready)..."
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)
            setupGattService()
            // Advertising will start in peripheralManager(_:didAdd:error:) callback
            return
        }
        
        // Start advertising (service is already set up)
        let msg = "[BLE] 📡 Starting BLE advertising with service UUID: \(BleManager.serviceUUID.uuidString)"
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        peripheralManager.startAdvertising([
            CBAdvertisementDataServiceUUIDsKey: [BleManager.serviceUUID],
            CBAdvertisementDataLocalNameKey: "OfflineProtocol"
        ])
    }
    
    private func setupGattService() {
        guard let peripheralManager = peripheralManager else { return }
        
        let msg = "[BLE] 🔧 Creating GATT service with 2 characteristics..."
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        // Message characteristic (write, notify)
        messageCharacteristic = CBMutableCharacteristic(
            type: BleManager.messageCharUUID,
            properties: [.write, .notify],
            value: nil,
            permissions: [.writeable]
        )
        
        // Device ID characteristic (read)
        let deviceIdData = deviceId.data(using: .utf8)
        deviceIdCharacteristic = CBMutableCharacteristic(
            type: BleManager.deviceIdCharUUID,
            properties: [.read],
            value: deviceIdData,
            permissions: [.readable]
        )
        
        let msg2 = "[BLE] 📝 Device ID characteristic value: '\(deviceId)' (\(deviceIdData?.count ?? 0) bytes)"
        NSLog("[BleManager] \(msg2)")
        onDiagnostic?(msg2)
        
        // Create service
        let service = CBMutableService(type: BleManager.serviceUUID, primary: true)
        service.characteristics = [messageCharacteristic!, deviceIdCharacteristic!]
        
        // Add service (async - will trigger peripheralManager(_:didAdd:error:) callback)
        let msg3 = "[BLE] ➕ Adding service to peripheral manager (async)..."
        NSLog("[BleManager] \(msg3)")
        onDiagnostic?(msg3)
        peripheralManager.add(service)
    }
    
    private func handleDiscoveredPeripheral(_ peripheral: CBPeripheral, rssi: NSNumber) {
        let work = {
            // Update RSSI value
            self.rssiValues[peripheral.identifier] = rssi.intValue

            // Check if we already discovered this peripheral
            if let existingPeer = self.discoveredPeers.values.first(where: { $0.peripheral.identifier == peripheral.identifier }) {
                // Update existing peer's RSSI, lastSeen, and peripheral reference
                var updatedPeer = existingPeer
                updatedPeer.peripheral = peripheral
                updatedPeer.rssi = rssi.intValue
                updatedPeer.lastSeen = Date()
                updatedPeer.connected = false
                self.discoveredPeers[existingPeer.deviceId] = updatedPeer

                if updatedPeer.connected {
                    self.connectedPeripherals[existingPeer.deviceId] = peripheral
                } else {
                    self.connectedPeripherals.removeValue(forKey: existingPeer.deviceId)
                }

                self.onPeerUpdated?(existingPeer.deviceId, peripheral.identifier.uuidString, rssi.intValue)
                return // Don't reconnect - peer is already known
            }

            // Skip if already connecting
            if self.connectingPeripherals[peripheral.identifier] != nil {
                return // Already trying to connect
            }

            let msg = "[BLE] 🔗 Connecting to peripheral \(peripheral.identifier.uuidString) to read device ID..."
            NSLog("[BleManager] \(msg)")
            self.onDiagnostic?(msg)

            // IMPORTANT: Store peripheral to prevent deallocation during connection
            self.connectingPeripherals[peripheral.identifier] = peripheral

            // Ensure delegate is set before connecting so callbacks route correctly
            peripheral.delegate = self

            // Set up connection timeout on main run loop
            let timeout = Timer(timeInterval: self.connectionTimeoutInterval, repeats: false) { [weak self, weak peripheral] _ in
                guard let peripheral = peripheral else { return }
                self?.handleConnectionTimeout(for: peripheral)
            }
            RunLoop.main.add(timeout, forMode: .common)
            self.connectionTimeouts[peripheral.identifier] = timeout

            // Connect to read device ID with connection options
            let options: [String: Any] = [
                CBConnectPeripheralOptionNotifyOnConnectionKey: true,
                CBConnectPeripheralOptionNotifyOnDisconnectionKey: true,
                CBConnectPeripheralOptionNotifyOnNotificationKey: true
            ]

            if Thread.isMainThread {
                self.centralManager?.connect(peripheral, options: options)
            } else {
                DispatchQueue.main.async {
                    self.centralManager?.connect(peripheral, options: options)
                }
            }
        }

        if Thread.isMainThread {
            work()
        } else {
            DispatchQueue.main.async(execute: work)
        }
    }
    
    private func handleConnectionTimeout(for peripheral: CBPeripheral) {
        let msg = "[BLE] ⏱️ Connection timeout for peripheral \(peripheral.identifier.uuidString) - cancelling..."
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        // Cancel the connection attempt
        if Thread.isMainThread {
            centralManager?.cancelPeripheralConnection(peripheral)
        } else {
            DispatchQueue.main.async {
                self.centralManager?.cancelPeripheralConnection(peripheral)
            }
        }
        
        // Clean up
        connectionTimeouts.removeValue(forKey: peripheral.identifier)
        connectingPeripherals.removeValue(forKey: peripheral.identifier)
        rssiValues.removeValue(forKey: peripheral.identifier)
    }
    
    private func startOperationsIfReady() {
        // Only start if:
        // 1. We've been told to start (shouldStartOperations is true)
        // 2. Both managers are ready
        guard shouldStartOperations,
              centralManagerReady,
              peripheralManagerReady else {
            let msg = "[BLE] Not ready yet - shouldStart: \(shouldStartOperations), central: \(centralManagerReady), peripheral: \(peripheralManagerReady)"
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)
            return
        }
        
        let msg = "[BLE] Both managers ready - starting scanning and advertising"
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        startScanning()
        startAdvertising()
    }
}

// MARK: - CBCentralManagerDelegate

extension BleManager: CBCentralManagerDelegate {
    
    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        let stateStr: String
        switch central.state {
        case .unknown: stateStr = "unknown"
        case .resetting: stateStr = "resetting"
        case .unsupported: stateStr = "unsupported"
        case .unauthorized: stateStr = "unauthorized"
        case .poweredOff: stateStr = "poweredOff"
        case .poweredOn: stateStr = "poweredOn"
        @unknown default: stateStr = "unknown(\(central.state.rawValue))"
        }
        
        let msg = "[BLE] Central Manager state: \(stateStr)"
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        switch central.state {
        case .poweredOn:
            centralManagerReady = true
            onStatusChanged?(.available)
            startOperationsIfReady()
        case .poweredOff, .unauthorized, .unsupported:
            centralManagerReady = false
            onStatusChanged?(.unavailable)
        default:
            centralManagerReady = false
            break
        }
    }
    
    func centralManager(_ central: CBCentralManager,
                       didDiscover peripheral: CBPeripheral,
                       advertisementData: [String : Any],
                       rssi RSSI: NSNumber) {
        let msg = "[BLE] 🎯 Discovered peripheral: \(peripheral.identifier.uuidString) RSSI: \(RSSI)"
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        handleDiscoveredPeripheral(peripheral, rssi: RSSI)
    }
    
    func centralManager(_ central: CBCentralManager,
                       didConnect peripheral: CBPeripheral) {
        let msg = "[BLE] ✅ Connected to peripheral \(peripheral.identifier.uuidString) - discovering services..."
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        // Cancel connection timeout timer
        if let timer = connectionTimeouts[peripheral.identifier] {
            timer.invalidate()
            connectionTimeouts.removeValue(forKey: peripheral.identifier)
        }
        
        // Set up service discovery timeout
        let timeout = Timer.scheduledTimer(withTimeInterval: serviceDiscoveryTimeoutInterval, repeats: false) { [weak self] _ in
            self?.handleServiceDiscoveryTimeout(for: peripheral)
        }
        serviceDiscoveryTimeouts[peripheral.identifier] = timeout
        
        peripheral.delegate = self
        // Discover ALL services to debug - we'll filter in the callback
        peripheral.discoverServices(nil)
    }
    
    private func handleServiceDiscoveryTimeout(for peripheral: CBPeripheral) {
        let msg = "[BLE] ⏱️ Service discovery timeout for peripheral \(peripheral.identifier.uuidString) - disconnecting..."
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        // Disconnect
        centralManager?.cancelPeripheralConnection(peripheral)
        
        // Clean up
        serviceDiscoveryTimeouts.removeValue(forKey: peripheral.identifier)
        connectingPeripherals.removeValue(forKey: peripheral.identifier)
    }
    
    func centralManager(_ central: CBCentralManager,
                       didDisconnectPeripheral peripheral: CBPeripheral,
                       error: Error?) {
        let msg: String
        if let error = error {
            msg = "[BLE] ⚠️ Disconnected from peripheral \(peripheral.identifier.uuidString): \(error.localizedDescription)"
        } else {
            msg = "[BLE] Disconnected from peripheral \(peripheral.identifier.uuidString)"
        }
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        //  Don't mark peer as lost on disconnect!
        // We intentionally disconnect after reading device ID and reconnect on-demand.
        // Peers are only truly "lost" when they stop advertising (handled by scan timeout logic if implemented).
        
        // Just update connection status and clean up connection tracking
        if let peer = discoveredPeers.first(where: { $0.value.peripheral.identifier == peripheral.identifier }) {
            let deviceId = peer.key
            var updatedPeer = peer.value
            updatedPeer.connected = false
            discoveredPeers[deviceId] = updatedPeer
            connectedPeripherals.removeValue(forKey: deviceId)
            
            NSLog("[BleManager] Peer \(deviceId) disconnected but still in discovered list")
        }
        
        connectingPeripherals.removeValue(forKey: peripheral.identifier)
    }
    
    func centralManager(_ central: CBCentralManager,
                       didFailToConnect peripheral: CBPeripheral,
                       error: Error?) {
        let msg: String
        if let error = error {
            msg = "[BLE] ❌ Failed to connect to peripheral \(peripheral.identifier.uuidString): \(error.localizedDescription)"
        } else {
            msg = "[BLE] ❌ Failed to connect to peripheral \(peripheral.identifier.uuidString): Unknown error"
        }
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        // Cancel connection timeout timer
        if let timer = connectionTimeouts[peripheral.identifier] {
            timer.invalidate()
            connectionTimeouts.removeValue(forKey: peripheral.identifier)
        }
        
        // Clean up stored data for this peripheral
        connectingPeripherals.removeValue(forKey: peripheral.identifier)
        rssiValues.removeValue(forKey: peripheral.identifier)
    }
}

// MARK: - CBPeripheralDelegate

extension BleManager: CBPeripheralDelegate {
    
    func peripheral(_ peripheral: CBPeripheral,
                   didDiscoverServices error: Error?) {
        // Cancel service discovery timeout
        if let timer = serviceDiscoveryTimeouts[peripheral.identifier] {
            timer.invalidate()
            serviceDiscoveryTimeouts.removeValue(forKey: peripheral.identifier)
        }
        
        guard error == nil,
              let services = peripheral.services else {
            let msg = "[BLE] ❌ Failed to discover services on \(peripheral.identifier.uuidString): \(error?.localizedDescription ?? "unknown")"
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)
            centralManager?.cancelPeripheralConnection(peripheral)
            serviceDiscoveryTimeouts.removeValue(forKey: peripheral.identifier)
            connectingPeripherals.removeValue(forKey: peripheral.identifier)
            return
        }
        
        let msg = "[BLE] ✅ Discovered \(services.count) service(s) on \(peripheral.identifier.uuidString)"
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        // Log all discovered service UUIDs for debugging
        let serviceUUIDs = services.map { $0.uuid.uuidString }.joined(separator: ", ")
        let msg1 = "[BLE] 📋 Service UUIDs: [\(serviceUUIDs)]"
        NSLog("[BleManager] \(msg1)")
        onDiagnostic?(msg1)
        
        var foundService = false
        for service in services {
            if service.uuid == BleManager.serviceUUID {
                foundService = true
                let msg2 = "[BLE] 🔍 Found Offline Protocol service - discovering characteristics..."
                NSLog("[BleManager] \(msg2)")
                onDiagnostic?(msg2)
                
                // Set up characteristic discovery timeout
                let timeout = Timer.scheduledTimer(withTimeInterval: characteristicDiscoveryTimeoutInterval, repeats: false) { [weak self] _ in
                    self?.handleCharacteristicDiscoveryTimeout(for: peripheral)
                }
                characteristicDiscoveryTimeouts[peripheral.identifier] = timeout
                
                // Discover ALL characteristics to debug
                peripheral.discoverCharacteristics(nil, for: service)
            }
        }
        
        if !foundService {
            let msg2 = "[BLE] ❌ Offline Protocol service NOT found in discovered services"
            NSLog("[BleManager] \(msg2)")
            onDiagnostic?(msg2)
            centralManager?.cancelPeripheralConnection(peripheral)
            connectingPeripherals.removeValue(forKey: peripheral.identifier)
        }
    }
    
    private func handleCharacteristicDiscoveryTimeout(for peripheral: CBPeripheral) {
        let msg = "[BLE] ⏱️ Characteristic discovery timeout for peripheral \(peripheral.identifier.uuidString) - disconnecting..."
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        // Disconnect
        centralManager?.cancelPeripheralConnection(peripheral)
        
        // Clean up
        characteristicDiscoveryTimeouts.removeValue(forKey: peripheral.identifier)
        connectingPeripherals.removeValue(forKey: peripheral.identifier)
    }
    
    func peripheral(_ peripheral: CBPeripheral,
                   didDiscoverCharacteristicsFor service: CBService,
                   error: Error?) {
        // Cancel characteristic discovery timeout
        if let timer = characteristicDiscoveryTimeouts[peripheral.identifier] {
            timer.invalidate()
            characteristicDiscoveryTimeouts.removeValue(forKey: peripheral.identifier)
        }
        
        guard error == nil,
              let characteristics = service.characteristics else {
            let msg = "[BLE] ❌ Failed to discover characteristics: \(error?.localizedDescription ?? "unknown")"
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)
            centralManager?.cancelPeripheralConnection(peripheral)
            connectingPeripherals.removeValue(forKey: peripheral.identifier)
            return
        }
        
        let msg = "[BLE] ✅ Discovered \(characteristics.count) characteristic(s)"
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        // Log all characteristic UUIDs for debugging
        let charUUIDs = characteristics.map { $0.uuid.uuidString }.joined(separator: ", ")
        let msg1 = "[BLE] 📋 Characteristic UUIDs: [\(charUUIDs)]"
        NSLog("[BleManager] \(msg1)")
        onDiagnostic?(msg1)
        
        // Check if we already know this peripheral's device ID (reconnecting for messaging)
        let existingPeer = discoveredPeers.values.first(where: { $0.peripheral.identifier == peripheral.identifier })
        
        var foundDeviceId = false
        for characteristic in characteristics {
            if characteristic.uuid == BleManager.deviceIdCharUUID {
                foundDeviceId = true
                
                if let knownPeer = existingPeer {
                    // We already know this peer - skip reading device ID, just mark as connected
                    NSLog("[BleManager] 📡 Reconnected to known peer \(knownPeer.deviceId) for messaging")
                    onDiagnostic?("[BLE] Reconnected to \(knownPeer.deviceId)")
                    
                    var updatedPeer = knownPeer
                    updatedPeer.connected = true
                    updatedPeer.lastSeen = Date()
                    discoveredPeers[knownPeer.deviceId] = updatedPeer
                    connectedPeripherals[knownPeer.deviceId] = peripheral
                    connectingPeripherals.removeValue(forKey: peripheral.identifier)
                } else {
                    // New peer - read device ID
                    let msg2 = "[BLE] 📖 Reading device ID characteristic from \(peripheral.identifier.uuidString)..."
                    NSLog("[BleManager] \(msg2)")
                    onDiagnostic?(msg2)
                    peripheral.readValue(for: characteristic)
                }
            } else if characteristic.uuid == BleManager.messageCharUUID {
                // Subscribe to notifications
                NSLog("[BleManager] Subscribing to message notifications...")
                peripheral.setNotifyValue(true, for: characteristic)
            }
        }
        
        if !foundDeviceId {
            let msg2 = "[BLE] ❌ Device ID characteristic NOT found in service"
            NSLog("[BleManager] \(msg2)")
            onDiagnostic?(msg2)
            centralManager?.cancelPeripheralConnection(peripheral)
            connectingPeripherals.removeValue(forKey: peripheral.identifier)
        }
    }
    
    func peripheral(_ peripheral: CBPeripheral,
                   didUpdateValueFor characteristic: CBCharacteristic,
                   error: Error?) {
        guard error == nil,
              let value = characteristic.value else {
            if let error = error {
                let msg = "[BLE] ❌ Failed to read characteristic value: \(error.localizedDescription)"
                NSLog("[BleManager] \(msg)")
                onDiagnostic?(msg)
            }
            return
        }
        
        if characteristic.uuid == BleManager.deviceIdCharUUID {
            let msg = "[BLE] 📥 Received device ID data: \(value.count) bytes"
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)
            
            if let remoteDeviceId = String(data: value, encoding: .utf8) {
                // Get stored RSSI value or use default
                let rssi = rssiValues[peripheral.identifier] ?? -60

                if discoveredDeviceIds.contains(remoteDeviceId) {
                    if var peer = discoveredPeers[remoteDeviceId] {
                        peer.peripheral = peripheral
                        peer.rssi = rssi
                        peer.lastSeen = Date()
                        peer.connected = true
                        discoveredPeers[remoteDeviceId] = peer
                    }
                    NSLog("[BleManager] Updated existing peer: \(remoteDeviceId) (RSSI: \(rssi))")
                    connectedPeripherals[remoteDeviceId] = peripheral
                } else {
                    let msg = "[BLE] 🎉 Discovered NEW peer device: \(remoteDeviceId) (RSSI: \(rssi))"
                    NSLog("[BleManager] \(msg)")
                    onDiagnostic?(msg)

                    discoveredDeviceIds.insert(remoteDeviceId)
                    discoveredPeers[remoteDeviceId] = DiscoveredPeer(
                        deviceId: remoteDeviceId,
                        peripheral: peripheral,
                        rssi: rssi,
                        lastSeen: Date(),
                        connected: false
                    )

                    onPeerDiscovered?(remoteDeviceId, peripheral.identifier.uuidString, rssi)
                }

                if var peer = discoveredPeers[remoteDeviceId] {
                    peer.connected = false
                    discoveredPeers[remoteDeviceId] = peer
                }
                connectedPeripherals.removeValue(forKey: remoteDeviceId)
                centralManager?.cancelPeripheralConnection(peripheral)
            } else {
                let msg = "[BLE] ❌ Failed to decode device ID from characteristic value"
                NSLog("[BleManager] \(msg)")
                onDiagnostic?(msg)

                if let entry = self.discoveredPeers.first(where: { $0.value.peripheral.identifier == peripheral.identifier }) {
                    let deviceId = entry.key
                    var peer = entry.value
                    peer.connected = false
                    self.discoveredPeers[deviceId] = peer
                }
                for (deviceId, storedPeripheral) in self.connectedPeripherals where storedPeripheral.identifier == peripheral.identifier {
                    self.connectedPeripherals.removeValue(forKey: deviceId)
                }
                centralManager?.cancelPeripheralConnection(peripheral)
            }
            
            connectingPeripherals.removeValue(forKey: peripheral.identifier)
        } else if characteristic.uuid == BleManager.messageCharUUID {
            // Received message
            NSLog("[BleManager] Received message: \(value.count) bytes")
            onMessageReceived?(value)
        }
    }
    
    func peripheral(_ peripheral: CBPeripheral,
                   didWriteValueFor characteristic: CBCharacteristic,
                   error: Error?) {
        if let error = error {
            NSLog("[BleManager] Failed to write characteristic: \(error.localizedDescription)")
        } else {
            NSLog("[BleManager] Successfully wrote to characteristic")
        }
    }
}

// MARK: - CBPeripheralManagerDelegate

extension BleManager: CBPeripheralManagerDelegate {
    
    func peripheralManagerDidUpdateState(_ peripheral: CBPeripheralManager) {
        let stateStr: String
        switch peripheral.state {
        case .unknown: stateStr = "unknown"
        case .resetting: stateStr = "resetting"
        case .unsupported: stateStr = "unsupported"
        case .unauthorized: stateStr = "unauthorized"
        case .poweredOff: stateStr = "poweredOff"
        case .poweredOn: stateStr = "poweredOn"
        @unknown default: stateStr = "unknown(\(peripheral.state.rawValue))"
        }
        
        let msg = "[BLE] Peripheral Manager state: \(stateStr)"
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        
        switch peripheral.state {
        case .poweredOn:
            peripheralManagerReady = true
            onStatusChanged?(.available)
            startOperationsIfReady()
        case .poweredOff, .unauthorized, .unsupported:
            peripheralManagerReady = false
            onStatusChanged?(.unavailable)
        default:
            peripheralManagerReady = false
            break
        }
    }
    
    func peripheralManager(_ peripheral: CBPeripheralManager,
                          didAdd service: CBService,
                          error: Error?) {
        if let error = error {
            let msg = "[BLE] ❌ Failed to add GATT service: \(error.localizedDescription)"
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)
            gattServiceAdded = false
            return
        }
        
        let msg = "[BLE] ✅ GATT service added successfully - now starting advertising..."
        NSLog("[BleManager] \(msg)")
        onDiagnostic?(msg)
        gattServiceAdded = true
        
        // Now that service is ready, start advertising
        startAdvertising()
    }
    
    func peripheralManagerDidStartAdvertising(_ peripheral: CBPeripheralManager,
                                             error: Error?) {
        if let error = error {
            let msg = "[BLE] ❌ Failed to start advertising: \(error.localizedDescription)"
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)
            onStatusChanged?(.unavailable)
        } else {
            let msg = "[BLE] ✅ Advertising started successfully - device is now discoverable"
            NSLog("[BleManager] \(msg)")
            onDiagnostic?(msg)
            onStatusChanged?(.advertising)
        }
    }
    
    func peripheralManager(_ peripheral: CBPeripheralManager,
                          didReceiveWrite requests: [CBATTRequest]) {
        NSLog("[BleManager] Received \(requests.count) write request(s)")
        
        for request in requests {
            if request.characteristic.uuid == BleManager.messageCharUUID,
               let value = request.value {
                NSLog("[BleManager] Received message: \(value.count) bytes")
                onMessageReceived?(value)
                peripheral.respond(to: request, withResult: .success)
            } else {
                peripheral.respond(to: request, withResult: .requestNotSupported)
            }
        }
    }
    
    func peripheralManager(_ peripheral: CBPeripheralManager,
                          didReceiveRead request: CBATTRequest) {
        NSLog("[BleManager] Received read request for \(request.characteristic.uuid)")
        
        if request.characteristic.uuid == BleManager.deviceIdCharUUID {
            request.value = deviceId.data(using: .utf8)
            peripheral.respond(to: request, withResult: .success)
        } else {
            peripheral.respond(to: request, withResult: .requestNotSupported)
        }
    }
}

