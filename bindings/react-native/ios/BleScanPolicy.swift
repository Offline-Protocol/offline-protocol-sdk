//
// BleScanPolicy.swift
// OfflineProtocol
//
// What a Bluetooth LE scan asks iOS for, and what its silence means, by
// application state.
//
// In the background iOS runs a scan only for the services it names, and
// delivers nothing to one that names none ("0 advertisements delivered", in
// bluetoothd's words). The bridge scanned with no filter and kept that scan
// when the app went to the background, so a backgrounded iPhone discovered
// nobody, and the scan watchdog read the silence as a stalled stack. Three
// restarts in, it replaced the CBCentralManager. A second manager under the
// same restore identifier, made while the first one's session is still
// alive, reports `.unsupported` and never leaves it; the bridge took that as
// a device with no Bluetooth, reported the radio unavailable, the core
// dropped every Bluetooth neighbour at once, and the fragment drain stopped
// sending on every link, foreground included, until the app was killed.
// Inbound kept arriving, so each peer saw a link that took its writes and
// never answered. Measured on 2026-10-30 about two minutes after
// backgrounding, beside a peer whose writes kept waking the app (an app with
// nothing waking it is suspended, and its watchdog never runs).
//
// So a backgrounded scan names the mesh service, and its silence is never a
// stall: quiet is what iOS delivers in the background, by design. The
// manager replacement itself is gone; see `BleManager.restartScanningDueToInactivity`.
//
// Pure functions of their arguments, pinned by `swift test`.

enum BleScanPolicy {
    /// The services a scan names: none in the foreground, where the bridge
    /// filters advertisements itself, and the mesh service in the
    /// background, the only scan iOS runs for a backgrounded app. Android
    /// advertises the service in its primary packet, which is what a
    /// background scan matches.
    static func services<Service>(_ service: Service, inBackground: Bool) -> [Service]? {
        inBackground ? [service] : nil
    }

    /// Whether a scan that has delivered nothing for a while may be read as
    /// stalled and restarted, or refreshed on a timer. Never in the
    /// background: iOS throttles a background scan and coalesces its
    /// duplicates, so a quiet one is working as designed, and every restart
    /// there only costs the app its next wake-up.
    static func mayRestartQuietScan(inBackground: Bool) -> Bool {
        !inBackground
    }
}
