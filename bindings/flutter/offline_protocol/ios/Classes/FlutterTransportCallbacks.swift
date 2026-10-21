import Foundation

final class FlutterBleTransportCallback: BleTransportCallback, @unchecked Sendable {
    weak var bleManager: BleManager?

    init(bleManager: BleManager) {
        self.bleManager = bleManager
    }

    func onFragmentsAvailable() {
        bleManager?.onFragmentsAvailable()
    }
}

final class FlutterWifiDirectTransportCallback: WifiDirectTransportCallback, @unchecked Sendable {
    weak var wifiDirectManager: WifiDirectManager?

    init(wifiDirectManager: WifiDirectManager) {
        self.wifiDirectManager = wifiDirectManager
    }

    func onMessagesAvailable() {
        wifiDirectManager?.onMessagesAvailable()
    }
}
