require "json"

package = JSON.parse(File.read(File.join(__dir__, "../package.json")))

Pod::Spec.new do |s|
  s.name         = "MeshSdk"
  s.version      = package["version"]
  s.summary      = package["description"]
  s.homepage     = package["homepage"]
  s.license      = package["license"]
  s.authors      = package["author"]

  s.platforms    = { :ios => "13.0" }
  s.source       = { :git => "https://github.com/Offline-Protocol/offline-protocol-sdk.git", :tag => "#{s.version}" }

  # Source files
  s.source_files = [
    "OfflineProtocolModule.{m,swift}",
    "ProtocolErrorBridge.swift",
    "TransportManager.swift",
    "BleManager.swift",
    "InternetManager.swift",
    "RelayControlOpTranslator.swift",
    "RelayRateLimiter.swift",
    "LegacyRelayMessage.swift",
    "PresenceWatchPolicy.swift",
    "RecipientInFlightTracker.swift",
    "RelayTimestamps.swift",
    "WifiDirectManager.swift",
    "NostrManager.swift",
    "ReticulumManager.swift",
    "MlsSecureStorage.swift",
    "ble/**/*.swift",
    "mesh/**/*.swift",
    "Generated/offline_protocol.swift",  # UniFFI generated Swift file
    "Generated/*.h"  # Include headers (but not module maps)
  ]
  
  # Preserve the FFI module map for Swift imports
  s.preserve_paths = [
    "Generated/offline_protocolFFI.modulemap"
  ]

  # Native library - use vendored_libraries with specific per-platform linking
  s.vendored_libraries = "libs/*.a"

  # Public headers
  s.public_header_files = [
    "Generated/offline_protocolFFI.h"
  ]

  # Configure library search paths and linking
  s.pod_target_xcconfig = {
    'LIBRARY_SEARCH_PATHS' => '$(PODS_TARGET_SRCROOT)/libs',
    'OTHER_LDFLAGS[sdk=iphonesimulator*]' => '-loffline_protocol_uniffi_sim',
    'OTHER_LDFLAGS[sdk=iphoneos*]' => '-loffline_protocol_uniffi_device',
    'SWIFT_INCLUDE_PATHS' => '$(PODS_TARGET_SRCROOT)/Generated',
    'HEADER_SEARCH_PATHS' => '$(inherited) $(PODS_TARGET_SRCROOT)/Generated'
  }

  # System libraries and frameworks
  s.frameworks = "Foundation", "CoreBluetooth", "MultipeerConnectivity"

  # React Native dependency
  s.dependency "React-Core"

  # Swift version
  s.swift_version = "5.5"
end

