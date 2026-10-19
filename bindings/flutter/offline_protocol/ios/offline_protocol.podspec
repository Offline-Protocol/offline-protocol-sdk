require 'json'

react_native_ios = File.expand_path('../../../react-native/ios', __dir__)

# PR2: engine lifecycle + Internet transport + shared relay helpers.
# BLE / Wi‑Fi / Nostr / Reticulum land in PR3. Keep in sync with
# android/build.gradle `flutterPr2ExcludedKotlin` / react-only list.
pr2_bridge_swift = [
  'ProtocolConfigParser.swift',
  'InitialRuntimeConfigApplier.swift',
  'InternetTransportBootstrap.swift',
  'EncryptionConfigReader.swift',
  'MeshRelayConfigReader.swift',
  'CustodyConfigReader.swift',
  'MlsSecureStorage.swift',
  'ProtocolStateStorage.swift',
  'StorageNamespace.swift',
  'LegacyStoreAdoption.swift',
  'TransportManager.swift',
  'InternetManager.swift',
  'RelayControlOpTranslator.swift',
  'RelayAnswerPrefixes.swift',
  'RelayGroupSnapshotBridge.swift',
  'RelayRateLimiter.swift',
  'RelayTimestamps.swift',
  'LegacyRelayMessage.swift',
  'AddressDeclarationPolicy.swift',
  'ForcedPresenceCheckQueue.swift',
  'RecipientInFlightTracker.swift',
  'SupersededLatchPolicy.swift',
  'GatewayAttachPolicy.swift',
  'GatewayVerdictTracker.swift',
  'PeerIdentityBinding.swift',
  'MonotonicClock.swift',
  'SocketGenerationTracker.swift',
  'WriteStallWatchdog.swift',
  'ProtocolErrorBridge.swift',
].map { |name| File.join(react_native_ios, name) }

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

  s.source_files = [
    'Classes/**/*.swift',
    File.join(react_native_ios, 'Generated/offline_protocol.swift'),
  ] + pr2_bridge_swift + [
    File.join(react_native_ios, 'Generated/*.h'),
  ]

  s.resource_bundles = {
    'OfflineProtocolPrivacy' => [File.join(react_native_ios, 'PrivacyInfo.xcprivacy')],
  }

  s.preserve_paths = [
    File.join(react_native_ios, 'Generated/offline_protocolFFI.modulemap'),
  ]

  s.vendored_frameworks = File.join(react_native_ios, 'libs/offline_protocol_uniffi.xcframework')

  s.public_header_files = [
    File.join(react_native_ios, 'Generated/offline_protocolFFI.h'),
  ]

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_INCLUDE_PATHS' => "$(PODS_TARGET_SRCROOT)/../../../react-native/ios/Generated",
    'HEADER_SEARCH_PATHS' => "$(inherited) $(PODS_TARGET_SRCROOT)/../../../react-native/ios/Generated",
  }

  s.frameworks = 'Foundation', 'Security', 'Network'
  s.dependency 'Flutter'
end
