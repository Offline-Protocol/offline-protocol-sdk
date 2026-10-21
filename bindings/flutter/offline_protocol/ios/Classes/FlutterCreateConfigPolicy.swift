import Foundation

/// Preview policy: shared bridge sources include Reticulum/Nostr managers, but
/// the Flutter plugin does not host them yet. Validated before create teardown.
enum FlutterCreateConfigPolicy {
    static let unsupportedTransportMessage =
        "Reticulum and Nostr are not hosted in the Flutter plugin yet. " +
        "Disable reticulumEnabled and nostrEnabled, or use React Native."

    static func requireCreateConfigSupported(config: ProtocolConfig) throws {
        if config.reticulumEnabled || config.nostrEnabled {
            throw HostError.invalidArgument(unsupportedTransportMessage)
        }
    }

    static func requireTransportTypeSupported(_ type: String) throws {
        switch type.lowercased() {
        case "reticulum", "nostr":
            throw HostError.invalidArgument(unsupportedTransportMessage)
        default:
            return
        }
    }

    static func requireTransportTypeSupported(arguments: Any?) throws {
        guard let map = arguments as? [String: Any] else { return }
        let type = (map["type"] as? String) ?? (map["transportType"] as? String)
        guard let type else { return }
        try requireTransportTypeSupported(type)
    }
}
