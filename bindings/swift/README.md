# The Swift package

The sources of the Swift package are not in this directory. The package is
assembled from the bridge sources in `bindings/react-native/ios`, which stay
where the pod compiles them and where the Rust guards read them
([ADR 0025](../../docs/adr/0025-native-packages-are-assembled-in-place.md)).
What lives here is what exists only for the package.

| File | What it is |
|------|------------|
| `Package.swift.template` | The manifest. `scripts/assemble-swift-package.sh` renders the binary target and the deployment target into it |
| `PACKAGE_README.md.template` | The README a consumer reads, rendered with the version and the deployment target |
| `Tests/` | Suites that need the linked library, which the test harness in `bindings/react-native/ios` cannot supply |
| `consumer-check/` | A separate package that depends on the assembled one as an application would, with a plain import |

## Build and test it

On a Mac with Xcode and the Rust iOS targets. On an Intel Mac the simulator
target is `x86_64-apple-ios`.

```bash
# 1. The Rust library, for the simulator, built for the pod's deployment
#    target.
IPHONEOS_DEPLOYMENT_TARGET="$(bash scripts/ios-deployment-target.sh)" \
  cargo build --release --target aarch64-apple-ios-sim --package offline-protocol-uniffi

# 2. The XCFramework the package links, with the header inside it.
bash scripts/package-swiftpm-xcframework.sh build/swiftpm \
  target/aarch64-apple-ios-sim/release/liboffline_protocol_uniffi.a

# 3. The package itself. The output directory must not exist or must be empty.
rm -rf build/offline-protocol-swift
bash scripts/assemble-swift-package.sh \
  --output build/offline-protocol-swift --version 0.0.0 \
  --xcframework-path build/swiftpm/offline_protocolFFI.xcframework \
  --bridge-tests

# 4. Every suite, on a simulator.
(cd build/offline-protocol-swift && xcodebuild test -scheme OfflineProtocolSDK \
  -destination "platform=iOS Simulator,id=$(bash ../../scripts/pick-ios-simulator.sh)")

# 5. The package from an application's side.
(cd bindings/swift/consumer-check && xcodebuild test -scheme ConsumerCheck-Package \
  -destination "platform=iOS Simulator,id=$(bash ../../../scripts/pick-ios-simulator.sh)")
```

`swift build` and `swift test` do not work here. They build for the Mac,
where the bridge does not compile and the XCFramework has no slice.

The package is assembled under `build/` and not under `target/`. CI caches
`target/`, and its cache keeps the directories of what it does not know and
deletes the files, so a package assembled there comes back as an empty tree.

`--bridge-tests` adds the suites from `bindings/react-native/ios/tests`. Three
of them run nowhere else: the mesh controller, the BLE discovery bootstrap
policy and the error mapping are excluded from the test harness, which cannot
compile the sources they cover. They find their vectors by walking up to the
repository root, so the output has to be inside the repository.

The directory is named for the distribution repository, `offline-protocol-swift`,
because Swift Package Manager names a package after where it came from, and
the consumer check refers to it by that name.

## Adding a source

A new file at the top level of `bindings/react-native/ios`, or under `ble/`
or `mesh/`, is in the package without being registered anywhere: the assemble
script takes what is there and leaves out a short list. A new directory has
to be named, in the script and in the podspec. The five registration points
in
[S2](../../docs/bridges/swift.md#s2-five-registration-points-per-new-swift-file)
still apply to the pod and the harness.

A file that imports React cannot be in the package. The assemble script
refuses it by name, and says what to do.
