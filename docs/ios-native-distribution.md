# Native iOS: blessed distribution path

This page is for a **native Swift app** (no React Native). It is the supported
install path instead of a multi-day hand-built XCFramework workflow.

React Native integrators should use [QUICKSTART.md](../QUICKSTART.md) and
`@offline-protocol/mesh-sdk` instead.

## Choose your path

| You are… | Use |
|----------|-----|
| Shipping an iOS app | **Swift package** below (release or distribution repo) |
| Working on the SDK repo | [bindings/swift/README.md](../bindings/swift/README.md) (assemble + test) |
| Patching bridge sources before a release | [Advanced build from source](ios-integration.md#advanced-build-the-bridge-from-source) in the iOS integration guide |

**Do not** treat `swift build` or `swift test` under
`bindings/react-native/ios` as how end-user apps integrate. That directory is a
**CI harness** for policy tests, not the shipping Swift package
([bridges/swift.md](bridges/swift.md#s7-the-swiftpm-harness-under-bindingsreact-nativeios-is-not-the-sdk)).

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
    .package(url: "https://github.com/Offline-Protocol/offline-protocol-swift.git", from: "0.33.0"),
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

Minimal sequence (details and transports in [ios-integration.md](ios-integration.md)):

1. `import OfflineProtocolSDK`
2. Build `ProtocolConfig` (see [configuration.md](configuration.md))
3. `setEventCallback` **before** `start()`
4. `try OfflineProtocolReferenceStorage.initializeMls(on:appId:profile:)` (built-in stores)
5. `try offlineProtocol.start()`

Lab-only cleartext and `EncryptFailed` hints: [native-development-cleartext.md](native-development-cleartext.md).
Custom protocol-state storage: verify with `StorageConformance.run(on:)` (C11).

Compile-checked sample (depends on the package as an app would):
[bindings/swift/consumer-check](../bindings/swift/consumer-check/).

### What the package contains

| Piece | Notes |
|-------|--------|
| `offline_protocolFFI.xcframework` | Rust `offline-protocol-uniffi`, device + simulator slices |
| Generated UniFFI API | `OfflineProtocol`, `ProtocolConfig`, events, data layer types |
| Bridge transports | BLE, internet, Wi‑Fi Direct, Nostr, Reticulum managers (same as RN pod, minus React-only files) |
| Integrator helpers | `OfflineProtocolReferenceStorage`, `StorageConformance`, lifecycle/diagnostic helpers |

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

- [ios-integration.md](ios-integration.md) — app patterns, plist, transports, groups
- [integrator-map.md](integrator-map.md) — compare RN vs native effort
- [bridges/swift.md](bridges/swift.md) — bridge contract and CI layout
- [UPGRADING.md](UPGRADING.md) — version-specific iOS steps
