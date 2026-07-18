// swift-tools-version:5.9
//
// CI/test harness ONLY — this package is NOT how the SDK ships. The pod
// (MeshSdk.podspec) owns distribution and lists its sources explicitly, so
// it never picks this manifest up. The package exists so `swift test` can
// run the standalone policy-class suites (the "mirrors android/, keep in
// sync" contract) without an Xcode project: the library target compiles
// exactly the Foundation-only helpers those suites cover — nothing that
// imports React, CoreBluetooth, or the Generated UniFFI module.
//
// The library target is named OfflineProtocol so the suites' existing
// `@testable import OfflineProtocol` keeps matching the pod module name.
//
// Run locally:  swift test --package-path bindings/react-native/ios
//

import PackageDescription

let package = Package(
    name: "OfflineProtocol",
    products: [
        .library(name: "OfflineProtocol", targets: ["OfflineProtocol"])
    ],
    targets: [
        .target(
            name: "OfflineProtocol",
            path: ".",
            // Everything the pod compiles but this harness must not (React /
            // CoreBluetooth / Generated-UniFFI dependents), excluded so
            // SwiftPM doesn't warn about unhandled files.
            exclude: [
                "BRIDGE_MAINTENANCE.md",
                "BleManager.swift",
                "Generated",
                "InternetManager.swift",
                "MeshSdk.podspec",
                "MlsSecureStorage.swift",
                "NostrManager.swift",
                "OfflineProtocolModule.m",
                "OfflineProtocolModule.swift",
                "ProtocolErrorBridge.swift",
                "ReticulumManager.swift",
                "TransportManager.swift",
                "WifiDirectManager.swift",
                "ble",
                "libs",
                "mesh",
                "tests"
            ],
            sources: [
                "EncryptionConfigReader.swift",
                "ForcedPresenceCheckQueue.swift",
                "LegacyRelayMessage.swift",
                "PresenceWatchPolicy.swift",
                "RecipientInFlightTracker.swift",
                "RelayControlOpTranslator.swift",
                "RelayGroupSnapshotBridge.swift",
                "RelayRateLimiter.swift",
                "RelayTimestamps.swift"
            ]
        ),
        .testTarget(
            name: "OfflineProtocolTests",
            dependencies: ["OfflineProtocol"],
            path: "tests",
            // The remaining suites cover classes that drag in the Generated
            // UniFFI module or platform frameworks; they still ride the app
            // build until they get a harness of their own.
            exclude: [
                "BleDiscoveryBootstrapPolicyTests.swift",
                "MeshControllerTests.swift",
                "ProtocolErrorBridgeTests.swift"
            ],
            sources: [
                "EncryptionConfigReaderTests.swift",
                "ForcedPresenceCheckQueueTests.swift",
                "LegacyRelayMessageTests.swift",
                "PresenceWatchPolicyTests.swift",
                "RecipientInFlightTrackerTests.swift",
                "RelayControlOpTranslatorTests.swift",
                "RelayGroupSnapshotBridgeTests.swift",
                "RelayRateLimiterTests.swift",
                "RelayTimestampsTests.swift"
            ]
        )
    ]
)
