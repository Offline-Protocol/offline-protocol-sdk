// swift-tools-version:5.9
//
// CI/test harness ONLY — this package is NOT how the SDK ships. The pod
// (../MeshSdk.podspec, at the package root so React Native autolinking can
// find it) owns distribution and lists its sources explicitly, so it never
// picks this manifest up. The package exists so `swift test` can
// run the standalone policy-class suites (the "mirrors android/, keep in
// sync" contract) without an Xcode project: the library target compiles
// exactly the Foundation-only helpers those suites cover — nothing that
// imports React, CoreBluetooth, or the Generated UniFFI module.
//
// The storage providers are included for compile coverage: under SWIFT_PACKAGE
// they bind to the local protocol shims in ProtocolStateStorage.swift instead
// of the Generated ones. Only the file-backed state store is exercised at
// runtime — the Keychain-backed one would touch the developer's login keychain,
// so its policy lives in LegacyStoreAdoption, which is tested directly.
//
// The library target is named OfflineProtocol so the suites' existing
// `@testable import OfflineProtocol` keeps matching the pod module name.
//
// Run locally:  swift test --package-path bindings/react-native/ios
//

import PackageDescription

let package = Package(
    name: "OfflineProtocol",
    platforms: [
        .macOS(.v10_15),
        .iOS(.v13)
    ],
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
                "AddressDeclarationPolicy.swift",
                "EncryptionConfigReader.swift",
                "ForcedPresenceCheckQueue.swift",
                "ForegroundReconnectPolicy.swift",
                "InboundFragmentBuffer.swift",
                "LegacyRelayMessage.swift",
                "LegacyStoreAdoption.swift",
                "MeshRelayConfigReader.swift",
                "MlsSecureStorage.swift",
                "MonotonicClock.swift",
                "NostrQueryTracker.swift",
                "OutboundFragmentQueue.swift",
                "PeerIdentityBinding.swift",
                "GatewayAttachPolicy.swift",
                "GatewayVerdictTracker.swift",
                "PeripheralRestorationAgeOutPolicy.swift",
                "PresenceWatchPolicy.swift",
                "ProtocolStateStorage.swift",
                "RecipientInFlightTracker.swift",
                "RelayAnswerPrefixes.swift",
                "RelayControlOpTranslator.swift",
                "RelayGroupSnapshotBridge.swift",
                "RelayRateLimiter.swift",
                "RelayTimestamps.swift",
                "SocketGenerationTracker.swift",
                "StorageNamespace.swift",
                "SupersededLatchPolicy.swift",
                "WriteStallWatchdog.swift"
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
                "AddressDeclarationPolicyTests.swift",
                "EncryptionConfigReaderTests.swift",
                "ForcedPresenceCheckQueueTests.swift",
                "GatewayAttachPolicyTests.swift",
                "GatewayVerdictTrackerTests.swift",
                "ForegroundReconnectPolicyTests.swift",
                "InboundFragmentBufferTests.swift",
                "LegacyRelayMessageTests.swift",
                "LegacyStoreAdoptionTests.swift",
                "MeshRelayConfigReaderTests.swift",
                "NostrQueryTrackerTests.swift",
                "OutboundFragmentQueueTests.swift",
                "PeerIdentityBindingTests.swift",
                "PeripheralRestorationAgeOutPolicyTests.swift",
                "PresenceWatchPolicyTests.swift",
                "ProtocolStateStorageTests.swift",
                "RecipientInFlightTrackerTests.swift",
                "RelayAnswerPrefixesTests.swift",
                "RelayControlOpTranslatorTests.swift",
                "RelayGroupSnapshotBridgeTests.swift",
                "RelayRateLimiterTests.swift",
                "RelayTimestampsTests.swift",
                "SocketGenerationTrackerTests.swift",
                "StorageNamespaceTests.swift",
                "SupersededLatchPolicyTests.swift",
                "WriteStallWatchdogTests.swift"
            ]
        )
    ]
)
