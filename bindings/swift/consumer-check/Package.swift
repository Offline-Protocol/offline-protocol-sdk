// swift-tools-version:5.9
//
// An application's view of the Swift package: the dependency the README
// gives, a plain import, and nothing else.
//
// A separate package on purpose. The package's own suites use
// `@testable import`, which reaches what an application cannot, so they say
// nothing about whether the product resolves by the name the README gives,
// whether what an application needs is public, or whether the library links
// into something that is not the package itself.
//
// It depends on the package as assembled at build/offline-protocol-swift, by
// path. The directory is named for the distribution repository, because
// Swift Package Manager names a package after where it came from, and the
// product line below is the one an application writes.

import PackageDescription

let package = Package(
    name: "ConsumerCheck",
    platforms: [
        // This application's own floor. An application chooses it, and one
        // that chose the package's floor has to be able to depend on it.
        .iOS(.v13)
    ],
    dependencies: [
        .package(path: "../../../build/offline-protocol-swift")
    ],
    targets: [
        .target(
            name: "ConsumerCheck",
            dependencies: [
                .product(name: "OfflineProtocolSDK", package: "offline-protocol-swift")
            ]
        ),
        .testTarget(
            name: "ConsumerCheckTests",
            dependencies: ["ConsumerCheck"]
        )
    ]
)
