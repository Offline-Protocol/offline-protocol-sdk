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
        let peripheral: CBPeripheral
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
    
    // Discovered peers
    private var discoveredPeers: [String: DiscoveredPeer] = [:]
    
    // Connected peripherals for messaging
    private var connectedPeripherals: [String: CBPeripheral] = [:]
    
    // Characteristics
    private var messageCharacteristic: CBMutableCharacteristic?
    private var deviceIdCharacteristic: CBMutableCharacteristic?
    
    // Callbacks
    var onPeerDiscovered: ((String, String, Int) -> Void)?
    var onPeerLost: ((String) -> Void)?
    var onMessageReceived: ((Data) -> Void)?
    var onStatusChanged: ((Status) -> Void)?
    
    // MARK: - Initialization
    
    @objc init(deviceId: String) {
        self.deviceId = deviceId
        super.init()
    }
    
    // MARK: - Public Methods
    
    @objc func start() -> Bool {
        NSLog("[BleManager] Starting BLE operations for device: \(deviceId)")
        
        shouldStartOperations = true
        
        // Initialize central manager (for scanning)
        if centralManager == nil {
            centralManager = CBCentralManager(delegate: self, queue: nil)
        }
        
        // Initialize peripheral manager (for advertising)
        if peripheralManager == nil {
            peripheralManager = CBPeripheralManager(delegate: self, queue: nil)
        }
        
        // Try to start operations if managers are already ready
        startOperationsIfReady()
        
        return true
    }
    
    @objc func stop() {
        NSLog("[BleManager] Stopping BLE operations")
        
        shouldStartOperations = false
        
        // Stop scanning
        centralManager?.stopScan()
        
        // Stop advertising
        peripheralManager?.stopAdvertising()
        
        // Disconnect all peripherals
        for (_, peripheral) in connectedPeripherals {
            centralManager?.cancelPeripheralConnection(peripheral)
        }
        connectedPeripherals.removeAll()
        discoveredPeers.removeAll()
        
        onStatusChanged?(.disconnected)
    }
    
    @objc func sendMessage(recipientId: String, messageData: Data) -> Bool {
        guard let peripheral = connectedPeripherals[recipientId] else {
            NSLog("[BleManager] No connection to peer: \(recipientId)")
            return false
        }
        
        guard let service = peripheral.services?.first(where: { $0.uuid == BleManager.serviceUUID }),
              let characteristic = service.characteristics?.first(where: { $0.uuid == BleManager.messageCharUUID }) else {
            NSLog("[BleManager] Message characteristic not found")
            return false
        }
        
        peripheral.writeValue(messageData, for: characteristic, type: .withResponse)
        NSLog("[BleManager] Sending message to \(recipientId)")
        return true
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
    
    // MARK: - Private Methods
    
    private func startScanning() {
        guard let centralManager = centralManager,
              centralManager.state == .poweredOn else {
            NSLog("[BleManager] Cannot start scanning, Bluetooth not ready")
            return
        }
        
        NSLog("[BleManager] Starting BLE scanning...")
        centralManager.scanForPeripherals(
            withServices: [BleManager.serviceUUID],
            options: [CBCentralManagerScanOptionAllowDuplicatesKey: false]
        )
        onStatusChanged?(.scanning)
    }
    
    private func startAdvertising() {
        guard let peripheralManager = peripheralManager,
              peripheralManager.state == .poweredOn else {
            NSLog("[BleManager] Cannot start advertising, Bluetooth not ready")
            return
        }
        
        // Setup GATT service
        setupGattService()
        
        // Start advertising
        NSLog("[BleManager] Starting BLE advertising...")
        peripheralManager.startAdvertising([
            CBAdvertisementDataServiceUUIDsKey: [BleManager.serviceUUID],
            CBAdvertisementDataLocalNameKey: "OfflineProtocol"
        ])
    }
    
    private func setupGattService() {
        guard let peripheralManager = peripheralManager else { return }
        
        // Message characteristic (write, notify)
        messageCharacteristic = CBMutableCharacteristic(
            type: BleManager.messageCharUUID,
            properties: [.write, .notify],
            value: nil,
            permissions: [.writeable]
        )
        
        // Device ID characteristic (read)
        deviceIdCharacteristic = CBMutableCharacteristic(
            type: BleManager.deviceIdCharUUID,
            properties: [.read],
            value: deviceId.data(using: .utf8),
            permissions: [.readable]
        )
        
        // Create service
        let service = CBMutableService(type: BleManager.serviceUUID, primary: true)
        service.characteristics = [messageCharacteristic!, deviceIdCharacteristic!]
        
        // Add service
        peripheralManager.add(service)
        NSLog("[BleManager] GATT service setup complete")
    }
    
    private func handleDiscoveredPeripheral(_ peripheral: CBPeripheral, rssi: NSNumber) {
        NSLog("[BleManager] Discovered peripheral: \(peripheral.identifier)")
        
        // Connect to read device ID
        centralManager?.connect(peripheral, options: nil)
    }
    
    private func startOperationsIfReady() {
        // Only start if:
        // 1. We've been told to start (shouldStartOperations is true)
        // 2. Both managers are ready
        guard shouldStartOperations,
              centralManagerReady,
              peripheralManagerReady else {
            NSLog("[BleManager] Not ready to start operations yet. shouldStart: \(shouldStartOperations), central: \(centralManagerReady), peripheral: \(peripheralManagerReady)")
            return
        }
        
        NSLog("[BleManager] Both managers ready - starting scanning and advertising")
        startScanning()
        startAdvertising()
    }
}

// MARK: - CBCentralManagerDelegate

extension BleManager: CBCentralManagerDelegate {
    
    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        NSLog("[BleManager] Central manager state: \(central.state.rawValue)")
        
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
        handleDiscoveredPeripheral(peripheral, rssi: RSSI)
    }
    
    func centralManager(_ central: CBCentralManager,
                       didConnect peripheral: CBPeripheral) {
        NSLog("[BleManager] Connected to peripheral: \(peripheral.identifier)")
        peripheral.delegate = self
        peripheral.discoverServices([BleManager.serviceUUID])
    }
    
    func centralManager(_ central: CBCentralManager,
                       didDisconnectPeripheral peripheral: CBPeripheral,
                       error: Error?) {
        NSLog("[BleManager] Disconnected from peripheral: \(peripheral.identifier)")
        
        // Find and remove peer
        if let peer = discoveredPeers.first(where: { $0.value.peripheral.identifier == peripheral.identifier }) {
            let deviceId = peer.key
            discoveredPeers.removeValue(forKey: deviceId)
            connectedPeripherals.removeValue(forKey: deviceId)
            onPeerLost?(deviceId)
        }
    }
}

// MARK: - CBPeripheralDelegate

extension BleManager: CBPeripheralDelegate {
    
    func peripheral(_ peripheral: CBPeripheral,
                   didDiscoverServices error: Error?) {
        guard error == nil,
              let services = peripheral.services else {
            NSLog("[BleManager] Failed to discover services: \(error?.localizedDescription ?? "unknown")")
            return
        }
        
        for service in services {
            if service.uuid == BleManager.serviceUUID {
                peripheral.discoverCharacteristics(
                    [BleManager.messageCharUUID, BleManager.deviceIdCharUUID],
                    for: service
                )
            }
        }
    }
    
    func peripheral(_ peripheral: CBPeripheral,
                   didDiscoverCharacteristicsFor service: CBService,
                   error: Error?) {
        guard error == nil,
              let characteristics = service.characteristics else {
            NSLog("[BleManager] Failed to discover characteristics: \(error?.localizedDescription ?? "unknown")")
            return
        }
        
        for characteristic in characteristics {
            if characteristic.uuid == BleManager.deviceIdCharUUID {
                // Read device ID
                peripheral.readValue(for: characteristic)
            } else if characteristic.uuid == BleManager.messageCharUUID {
                // Subscribe to notifications
                peripheral.setNotifyValue(true, for: characteristic)
            }
        }
    }
    
    func peripheral(_ peripheral: CBPeripheral,
                   didUpdateValueFor characteristic: CBCharacteristic,
                   error: Error?) {
        guard error == nil,
              let value = characteristic.value else {
            return
        }
        
        if characteristic.uuid == BleManager.deviceIdCharUUID {
            // Got device ID
            if let remoteDeviceId = String(data: value, encoding: .utf8) {
                NSLog("[BleManager] Discovered peer device: \(remoteDeviceId)")
                
                // Store peer
                let peer = DiscoveredPeer(
                    deviceId: remoteDeviceId,
                    peripheral: peripheral,
                    rssi: -60, // TODO: Store actual RSSI
                    lastSeen: Date(),
                    connected: true
                )
                discoveredPeers[remoteDeviceId] = peer
                connectedPeripherals[remoteDeviceId] = peripheral
                
                // Notify discovery
                onPeerDiscovered?(remoteDeviceId, peripheral.identifier.uuidString, peer.rssi)
            }
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
        NSLog("[BleManager] Peripheral manager state: \(peripheral.state.rawValue)")
        
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
    
    func peripheralManagerDidStartAdvertising(_ peripheral: CBPeripheralManager,
                                             error: Error?) {
        if let error = error {
            NSLog("[BleManager] Failed to start advertising: \(error.localizedDescription)")
            onStatusChanged?(.unavailable)
        } else {
            NSLog("[BleManager] Advertising started successfully")
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

