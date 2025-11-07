//
//  BleManager.swift
//  OfflineProtocol
//
//  BLE transport implementation using CoreBluetooth
//  Supports iOS ↔ Android cross-platform communication
//

import Foundation
import CoreBluetooth

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
    
    // Peripheral (advertiser/server) components
    private var peripheralManager: CBPeripheralManager?
    private var messageCharacteristic: CBMutableCharacteristic?
    private var deviceIdCharacteristic: CBMutableCharacteristic?
    
    // Fragment polling
    private var fragmentTimer: Timer?
    private let fragmentQueue = DispatchQueue(label: "com.offlineprotocol.ble.fragments")
    
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
        updateState(.starting)
        
        // Initialize Central Manager (for scanning)
        centralManager = CBCentralManager(
            delegate: self,
            queue: DispatchQueue.global(qos: .userInitiated),
            options: [CBCentralManagerOptionShowPowerAlertKey: true]
        )
        
        // Initialize Peripheral Manager (for advertising)
        peripheralManager = CBPeripheralManager(
            delegate: self,
            queue: DispatchQueue.global(qos: .userInitiated),
            options: [CBPeripheralManagerOptionShowPowerAlertKey: true]
        )
        
        print("[BleManager] Waiting for Bluetooth to power on...")
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
        stopScanning()
        
        // Stop advertising
        stopAdvertising()
        
        // Disconnect all peripherals
        for peripheral in connectedPeripherals.values {
            centralManager?.cancelPeripheralConnection(peripheral)
        }
        connectedPeripherals.removeAll()
        discoveredPeripherals.removeAll()
        peripheralDeviceIds.removeAll()
        
        // Clean up managers
        centralManager = nil
        peripheralManager = nil
        
        centralReady = false
        peripheralReady = false
        
        updateState(.stopped)
    }
    
    public func pause() {
        // For iOS background mode
        stopScanning()
        stopFragmentPolling()
    }
    
    public func resume() {
        // Resume from background
        if state == .running {
            startScanning()
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
    
    private func startScanning() {
        guard let central = centralManager, central.state == .poweredOn, !isScanning else {
            return
        }
        
        print("[BleManager] Starting scan for service: \(SERVICE_UUID)")
        central.scanForPeripherals(
            withServices: [SERVICE_UUID],
            options: [CBCentralManagerScanOptionAllowDuplicatesKey: true]
        )
        isScanning = true
    }
    
    private func stopScanning() {
        guard isScanning else { return }
        centralManager?.stopScan()
        isScanning = false
        print("[BleManager] Stopped scanning")
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
        
        print("[BleManager] Starting advertising with service: \(SERVICE_UUID)")
        peripheral.startAdvertising(advertisementData)
        isAdvertising = true
    }
    
    private func stopAdvertising() {
        guard isAdvertising else { return }
        peripheralManager?.stopAdvertising()
        isAdvertising = false
        print("[BleManager] Stopped advertising")
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
    }
    
    private func stopFragmentPolling() {
        fragmentTimer?.invalidate()
        fragmentTimer = nil
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
            return
        }
        
        // Find message characteristic
        guard let service = peripheral.services?.first(where: { $0.uuid == SERVICE_UUID }),
              let characteristic = service.characteristics?.first(where: { $0.uuid == MESSAGE_CHAR_UUID }) else {
            print("[BleManager] Message characteristic not found")
            return
        }
        
        // Write data without response
        peripheral.writeValue(data, for: characteristic, type: .withoutResponse)
        
        bytesSent += UInt64(data.count)
        fragmentsSent += 1
        
        print("[BleManager] Sent fragment to \(recipientId): \(data.count) bytes")
    }
    
    private func handleReceivedData(_ data: Data, from peripheral: CBPeripheral) {
        fragmentQueue.async { [weak self] in
            guard let self = self else { return }
            
            // Get sender device ID
            guard let senderId = self.peripheralDeviceIds[peripheral.identifier] else {
                print("[BleManager] Unknown sender peripheral")
                return
            }
            
            // Convert to byte array
            let bytes = [UInt8](data)
            
            // Pass to protocol
            do {
                try self.protocolInstance.bleFragmentReceived(senderId: senderId, fragment: bytes)
                
                self.bytesReceived += UInt64(data.count)
                self.fragmentsReceived += 1
                
                print("[BleManager] Received fragment from \(senderId): \(data.count) bytes")
            } catch {
                print("[BleManager] Error processing received fragment: \(error)")
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
        
        switch central.state {
        case .poweredOn:
            centralReady = true
            startScanning()
            startFragmentPolling()
            
            // If both central and peripheral are ready, mark as running
            if peripheralReady && state == .starting {
                updateState(.running)
                try? self.protocolInstance.bleStatusChanged(isAvailable: true)
            }
            
        case .poweredOff, .unauthorized, .unsupported:
            centralReady = false
            stopScanning()
            updateState(.unavailable)
            try? self.protocolInstance.bleStatusChanged(isAvailable: false)
            
        default:
            break
        }
    }
    
    public func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral, advertisementData: [String: Any], rssi RSSI: NSNumber) {
        let rssiValue = RSSI.int16Value
        
        // Store discovered peripheral
        discoveredPeripherals[peripheral.identifier] = peripheral
        
        print("[BleManager] Discovered peripheral: \(peripheral.identifier) RSSI: \(rssiValue)")
        
        // Connect to peripheral if not already connected
        if connectedPeripherals[peripheral.identifier] == nil {
            peripheral.delegate = self
            central.connect(peripheral, options: nil)
        }
    }
    
    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        print("[BleManager] Connected to peripheral: \(peripheral.identifier)")
        
        connectedPeripherals[peripheral.identifier] = peripheral
        
        // Discover services
        peripheral.discoverServices([SERVICE_UUID])
    }
    
    public func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        print("[BleManager] Failed to connect to peripheral: \(error?.localizedDescription ?? "unknown")")
        discoveredPeripherals.removeValue(forKey: peripheral.identifier)
    }
    
    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        print("[BleManager] Disconnected from peripheral: \(peripheral.identifier)")
        
        connectedPeripherals.removeValue(forKey: peripheral.identifier)
        
        // Notify protocol of peer loss
        if let deviceId = peripheralDeviceIds[peripheral.identifier] {
            try? self.protocolInstance.blePeerLost(peerId: deviceId)
            peripheralDeviceIds.removeValue(forKey: peripheral.identifier)
        }
    }
}

// MARK: - CBPeripheralDelegate

extension BleManager: CBPeripheralDelegate {
    
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        if let error = error {
            print("[BleManager] Error discovering services: \(error)")
            return
        }
        
        guard let services = peripheral.services else { return }
        
        for service in services where service.uuid == SERVICE_UUID {
            peripheral.discoverCharacteristics([MESSAGE_CHAR_UUID, DEVICE_ID_CHAR_UUID], for: service)
        }
    }
    
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        if let error = error {
            print("[BleManager] Error discovering characteristics: \(error)")
            return
        }
        
        guard let characteristics = service.characteristics else { return }
        
        for characteristic in characteristics {
            if characteristic.uuid == MESSAGE_CHAR_UUID {
                // Enable notifications for message characteristic
                peripheral.setNotifyValue(true, for: characteristic)
                print("[BleManager] Enabled notifications for message characteristic")
            } else if characteristic.uuid == DEVICE_ID_CHAR_UUID {
                // Read device ID
                peripheral.readValue(for: characteristic)
            }
        }
    }
    
    public func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        if let error = error {
            print("[BleManager] Error reading characteristic: \(error)")
            return
        }
        
        guard let data = characteristic.value else { return }
        
        if characteristic.uuid == DEVICE_ID_CHAR_UUID {
            // Store device ID
            if let deviceId = String(data: data, encoding: .utf8) {
                peripheralDeviceIds[peripheral.identifier] = deviceId
                    
                    // Notify protocol of peer discovery
                    let rssi: Int16 = -50 // Default RSSI (will be updated from scan)
                    try? self.protocolInstance.blePeerDiscovered(peerId: deviceId, rssi: rssi)
                    
                    print("[BleManager] Peer discovered: \(deviceId)")
            }
        } else if characteristic.uuid == MESSAGE_CHAR_UUID {
            // Handle received message fragment
            handleReceivedData(data, from: peripheral)
        }
    }
    
    public func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        if let error = error {
            print("[BleManager] Error writing characteristic: \(error)")
        }
    }
}

// MARK: - CBPeripheralManagerDelegate

extension BleManager: CBPeripheralManagerDelegate {
    
    public func peripheralManagerDidUpdateState(_ peripheral: CBPeripheralManager) {
        print("[BleManager] Peripheral state: \(peripheral.state.rawValue)")
        
        switch peripheral.state {
        case .poweredOn:
            peripheralReady = true
            startAdvertising()
            
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
            
        default:
            break
        }
    }
    
    public func peripheralManagerDidStartAdvertising(_ peripheral: CBPeripheralManager, error: Error?) {
        if let error = error {
            print("[BleManager] Error starting advertising: \(error)")
        } else {
            print("[BleManager] Advertising started successfully")
        }
    }
    
    public func peripheralManager(_ peripheral: CBPeripheralManager, didReceiveWrite requests: [CBATTRequest]) {
        for request in requests {
            if request.characteristic.uuid == MESSAGE_CHAR_UUID, let value = request.value {
                // Handle incoming fragment
                // Note: We don't have the central's device ID here directly
                // In a real implementation, we'd need to track central connections
                handleReceivedData(value, from: request.central as! CBPeripheral)
            }
            
            // Respond to write request
            peripheral.respond(to: request, withResult: .success)
        }
    }
    
    public func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didSubscribeTo characteristic: CBCharacteristic) {
        print("[BleManager] Central subscribed to characteristic: \(characteristic.uuid)")
    }
    
    public func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didUnsubscribeFrom characteristic: CBCharacteristic) {
        print("[BleManager] Central unsubscribed from characteristic: \(characteristic.uuid)")
    }
}

