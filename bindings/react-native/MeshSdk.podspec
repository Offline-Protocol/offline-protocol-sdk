require "json"

# This podspec lives at the package root on purpose: React Native's autolinking
# resolves a dependency's podspec by globbing "*.podspec" in the package root
# only, non-recursively. A podspec under ios/ is invisible to it, which is why
# iOS autolinking used to be disabled for this package and every consumer had to
# add `pod 'MeshSdk'` by hand. Do not move this file back into ios/.
package = JSON.parse(File.read(File.join(__dir__, "package.json")))

Pod::Spec.new do |s|
  s.name         = "MeshSdk"
  s.version      = package["version"]
  s.summary      = package["description"]
  s.homepage     = package["homepage"]
  s.license      = package["license"]
  s.authors      = package["author"]

  s.platforms    = { :ios => "13.0" }
  s.source       = { :git => "https://github.com/Offline-Protocol/offline-protocol-sdk.git", :tag => "#{s.version}" }

  # Source files (paths are relative to this file, i.e. the package root)
  s.source_files = [
    "ios/OfflineProtocolModule.{m,swift}",
    "ios/EncryptionConfigReader.swift",
    "ios/ProtocolErrorBridge.swift",
    "ios/TransportManager.swift",
    "ios/BleManager.swift",
    "ios/InternetManager.swift",
    "ios/ForcedPresenceCheckQueue.swift",
    "ios/RelayControlOpTranslator.swift",
    "ios/RelayGroupSnapshotBridge.swift",
    "ios/RelayRateLimiter.swift",
    "ios/LegacyRelayMessage.swift",
    "ios/LegacyStoreAdoption.swift",
    "ios/ForegroundReconnectPolicy.swift",
    "ios/MonotonicClock.swift",
    "ios/PresenceWatchPolicy.swift",
    "ios/RecipientInFlightTracker.swift",
    "ios/RelayTimestamps.swift",
    "ios/SocketGenerationTracker.swift",
    "ios/SupersededLatchPolicy.swift",
    "ios/WriteStallWatchdog.swift",
    "ios/WifiDirectManager.swift",
    "ios/NostrManager.swift",
    "ios/ReticulumManager.swift",
    "ios/MlsSecureStorage.swift",
    "ios/ProtocolStateStorage.swift",
    "ios/StorageNamespace.swift",
    "ios/ble/**/*.swift",
    "ios/mesh/**/*.swift",
    "ios/Generated/offline_protocol.swift",  # UniFFI generated Swift file
    "ios/Generated/*.h"  # Include headers (but not module maps)
  ]

  # Preserve the FFI module map for Swift imports
  s.preserve_paths = [
    "ios/Generated/offline_protocolFFI.modulemap"
  ]

  # Native library. An XCFramework, not loose per-platform archives: CocoaPods
  # copies the slice matching the build SDK and points the search paths at it,
  # so device and simulator builds both link the right binary with no
  # sdk-conditional linker flags anywhere. A flat directory of archives cannot
  # do this — CocoaPods emits an unconditional -l for every archive it finds,
  # and the only xcconfig that could gate them is the app target's, which a
  # podspec has no way to reach.
  s.vendored_frameworks = "ios/libs/offline_protocol_uniffi.xcframework"

  # Public headers
  s.public_header_files = [
    "ios/Generated/offline_protocolFFI.h"
  ]

  # Header/module-map search paths for the generated UniFFI FFI layer. Linking
  # is handled entirely by the vendored XCFramework above.
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_INCLUDE_PATHS' => '$(PODS_TARGET_SRCROOT)/ios/Generated',
    'HEADER_SEARCH_PATHS' => '$(inherited) $(PODS_TARGET_SRCROOT)/ios/Generated'
  }

  # System libraries and frameworks
  s.frameworks = "Foundation", "CoreBluetooth", "MultipeerConnectivity"

  # React Native dependency
  s.dependency "React-Core"

  # Swift version
  s.swift_version = "5.5"
end
