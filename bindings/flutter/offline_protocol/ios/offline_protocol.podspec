require 'json'

react_native_ios = File.expand_path('../../../react-native/ios', __dir__)

bridge_swift = Dir.glob("#{react_native_ios}/**/*.swift").reject do |path|
  rel = path.delete_prefix("#{react_native_ios}/")
  rel == 'OfflineProtocolModule.swift' ||
    rel.start_with?('tests/') ||
    rel == 'Package.swift'
end

Pod::Spec.new do |s|
  s.name             = 'offline_protocol'
  s.version          = '0.29.0'
  s.summary          = 'Flutter bindings for the Offline Protocol SDK'
  s.description      = 'Offline-first mesh messaging with MLS encryption (Flutter plugin).'
  s.homepage         = 'https://github.com/Offline-Protocol/offline-protocol-sdk'
  s.license          = { :type => 'AGPL-3.0-only' }
  s.author           = { 'Offline Protocol, Inc.' => 'support@offlineprotocol.com' }
  s.source           = { :path => '.' }
  s.platform         = :ios, '13.0'
  s.swift_version    = '5.5'

  s.source_files = ['Classes/**/*.swift'] + bridge_swift + [
    "#{react_native_ios}/Generated/*.h",
  ]

  s.resource_bundles = {
    'OfflineProtocolPrivacy' => ["#{react_native_ios}/PrivacyInfo.xcprivacy"],
  }

  s.preserve_paths = [
    "#{react_native_ios}/Generated/offline_protocolFFI.modulemap",
  ]

  s.vendored_frameworks = "#{react_native_ios}/libs/offline_protocol_uniffi.xcframework"

  s.public_header_files = [
    "#{react_native_ios}/Generated/offline_protocolFFI.h",
  ]

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_INCLUDE_PATHS' => "$(PODS_TARGET_SRCROOT)/../../../react-native/ios/Generated",
    'HEADER_SEARCH_PATHS' => "$(inherited) $(PODS_TARGET_SRCROOT)/../../../react-native/ios/Generated",
  }

  s.frameworks = 'Foundation', 'CoreBluetooth', 'Network'
  s.dependency 'Flutter'
end
