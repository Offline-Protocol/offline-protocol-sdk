# Native iOS: the Swift package

This page is for a **native Swift app** (no React Native). It is the single
recommended install path instead of a multi-day hand-built XCFramework workflow.

React Native integrators should use [QUICKSTART.md](../QUICKSTART.md) and
`@offline-protocol/mesh-sdk` instead.

> **Stuck?** Ask in the [developer Discord](https://discord.gg/9TQGrxaPJk), where the team and other integrators answer questions.

## Choose your path

| You are… | Use |
|----------|-----|
| Shipping an iOS app | **Swift package** below (release or distribution repo) |
| Working on the SDK repo | [bindings/swift/README.md](../bindings/swift/README.md) (assemble + test) |
| Patching bridge sources before a release | [Advanced build from source](ios-integration.md#advanced-build-the-bridge-from-source) in the iOS integration guide |

**Do not** treat `swift build` or `swift test` under
`bindings/react-native/ios` as how end-user apps integrate. That directory is a
**CI harness** for policy tests, not the shipping Swift package
([bridges/swift.md](bridges/swift.md#the-swift-package) and
[S2](bridges/swift.md#s2-five-registration-points-per-new-swift-file) in the Swift
bridge contract).

## Recommended: Swift package (`OfflineProtocolSDK`)

From 0.28.0 onward, releases publish a Swift package assembled from the same
bridge sources as the React Native iOS module ([ADR 0025](adr/0025-native-packages-are-assembled-in-place.md)).

### Install (released version)

1. In Xcode: **File → Add Package Dependencies…**
2. URL: `https://github.com/Offline-Protocol/offline-protocol-swift.git`
3. Pick the SDK version that matches your release (same `X.Y.Z` as
   `offline-protocol-sdk`).

Or in `Package.swift`:

```swift
dependencies: [
    .package(url: "https://github.com/Offline-Protocol/offline-protocol-swift.git", from: "0.34.0"),
],
targets: [
    .target(
        name: "YourApp",
        dependencies: [
            .product(name: "OfflineProtocolSDK", package: "offline-protocol-swift"),
        ]
    ),
]
```

Requirements: **iOS only**, deployment target from the package manifest (see
[bindings/swift](../bindings/swift/README.md); it tracks the React Native pod).

### Wire the engine (product code)

Import **`OfflineProtocolSDK`** only. You do not add
`bindings/react-native/ios/Generated/offline_protocol.swift` by hand; the
package links the Rust library and generated bindings together.

Minimal sequence (details and transports in [ios-integration.md](ios-integration.md)).
It needs 0.34.0 or later: `OfflineProtocolReferenceStorage` and
`StorageConformance` are not in 0.33.0 and earlier.

1. `import OfflineProtocolSDK`
2. Build `ProtocolConfig` (see [configuration.md](configuration.md))
3. `let engine = try OfflineProtocol(config: config)`
4. `engine.setEventCallback(callback: …)` **before** `start()` (events emitted
   during startup are lost if the callback is registered late)
5. `let adoption = try OfflineProtocolReferenceStorage.initializeMls(on: engine, appLabel: config.appLabel, profile: config.profile)`
   opens the built-in stores. Report `adoption.warning` when it is set: it is
   the only signal that this account started from a fresh MLS identity.
6. `try engine.start()`

Lab-only cleartext and `EncryptFailed` hints: [native-development-cleartext.md](native-development-cleartext.md).

Custom protocol-state storage: run `StorageConformance.run(on:)` on a scratch
backend before you wire it in. Green on the
[storage conformance suite](bridges/README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified)
is what makes a protocol-state backend supported.

Compile-checked sample (package dependency, same call order as above):
[bindings/swift/consumer-check](../bindings/swift/consumer-check/) (`startWithReferenceStorage` in
`Use.swift`).

### What the package contains

| Piece | Notes |
|-------|--------|
| `offline_protocolFFI.xcframework` | Rust `offline-protocol-uniffi`, device + simulator slices |
| Generated UniFFI API | `OfflineProtocol`, `ProtocolConfig`, events, data layer types |
| Bridge transports | `BleManager`, `InternetManager`, `WifiDirectManager` (the peer-stream slot: TCP over a LAN or AWDL, not Wi-Fi Direct), `NostrManager`, `ReticulumManager`; the React Native pod's sources minus the two that need React |
| Integrator helpers | `OfflineProtocolReferenceStorage`, `StorageConformance`, `OfflineProtocolDevelopmentCleartextConfig`, `OfflineProtocolEncryptFailedDiagnostics` |

Transports still need app-side wiring (permissions, background modes, starting
managers). See [ios-integration.md](ios-integration.md) and
[mobile-permissions.md](mobile-permissions.md).

## Release artifacts (alternative to the distribution repo)

GitHub releases attach:

- `offline-protocol-X.Y.Z-swiftpm-xcframework.zip`
- `offline-protocol-X.Y.Z-swift-package.tar.gz` (assembled tree with manifest +
  checksum for the XCFramework URL)

Corporate mirrors may vendor the tarball instead of cloning
`offline-protocol-swift`. Do not edit generated trees by hand; they are
overwritten each release ([distribution README template](../bindings/swift/PACKAGE_README.md.template)).

## What we discourage for app teams

- Building `liboffline_protocol_uniffi.a` locally and stitching an XCFramework
  **as the default** onboarding path (error-prone, slow, and easy to desync from
  generated bindings).
- Running `swift build` in the repository expecting a consumable SDK artifact.
- CocoaPods for the native Swift package (not published; React Native uses the
  in-repo `MeshSdk` pod).

SDK maintainers and contributors still use the scripts in
[ios-integration.md#advanced-build-the-bridge-from-source](ios-integration.md#advanced-build-the-bridge-from-source)
and [bindings/swift/README.md](../bindings/swift/README.md) to validate changes.

## Upgrading

On a new SDK version, bump the Swift package dependency to the same `X.Y.Z`
(release train). You do not rebuild Rust locally unless you fork from source.
Details: [UPGRADING.md §12.3](UPGRADING.md#123-native-ios-bump-the-swift-package-not-a-hand-built-xcframework).

## See also

- [ios-integration.md](ios-integration.md): app patterns, plist, transports, groups
- [integrator-map.md](integrator-map.md): compare RN vs native effort
- [bridges/swift.md](bridges/swift.md): bridge contract and CI layout
- [UPGRADING.md](UPGRADING.md): version-specific iOS steps
